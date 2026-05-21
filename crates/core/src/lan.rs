use std::collections::HashMap;
use std::error::Error as StdError;
use std::io::ErrorKind;
use std::mem::ManuallyDrop;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use flume::RecvTimeoutError;
use iroh::EndpointId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo, TxtProperties};
use rand::seq::SliceRandom;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::protocol::DeviceType;
/// DNS-SD type for wisp receivers on the LAN (`wisp` ≤ 15 bytes per RFC 6763).
pub const WISP_MDNS_SERVICE_TYPE: &str = "_wisp._udp.local.";

/// TXT `ver` value for this wire format (ticket chunks `t0`… + `tc`).
pub const WISP_MDNS_TXT_VER: &str = "1";

/// UDP port for presence ping/pong (SRV port in mDNS). Not the iroh data plane.
pub const WISP_LAN_PRESENCE_PORT: u16 = 47_474;

/// UDP port for broadcast discovery (works on Android hotspot where mDNS multicast is blocked).
pub const WISP_LAN_DISCOVERY_PORT: u16 = 47_475;

const TICKET_CHUNK_LEN: usize = 200;

const PRESENCE_MAGIC: &[u8; 4] = b"WSPP";
const PRESENCE_VER: u16 = 1;
const OP_PING: u8 = 1;
const OP_PONG: u8 = 2;
const PRESENCE_PKT_LEN: usize = 16;

// Broadcast discovery protocol (WSPD = Wisp Discovery).
// Query:  16 bytes fixed  — sender broadcasts to 255.255.255.255:WISP_LAN_DISCOVERY_PORT
// Reply:  variable length — receiver unicasts back ticket + label
const DISCOVERY_MAGIC: &[u8; 4] = b"WSPD";
const DISCOVERY_VER: u16 = 1;
const OP_QUERY: u8 = 1;
const OP_DREPLY: u8 = 2;
const DISCOVERY_QUERY_LEN: usize = 16;

/// How long a cached endpoint IP stays valid between scans.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// Per-process IP cache — maps iroh ticket → last-seen endpoint + receiver metadata.
/// Allows the next scan to immediately presence-ping known IPs without waiting for a
/// fresh mDNS re-announce (SRV TTL = 120 s).
struct CachedEntry {
    ip: Ipv4Addr,
    /// Presence-ping port (always WISP_LAN_PRESENCE_PORT for confirmed peers).
    port: u16,
    receiver: NearbyReceiver,
    seen_at: Instant,
}

static PEER_CACHE: OnceLock<Mutex<HashMap<String, CachedEntry>>> = OnceLock::new();

fn peer_cache() -> &'static Mutex<HashMap<String, CachedEntry>> {
    PEER_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Error)]
pub enum LanError {
    #[error("could not determine a usable IPv4 address for LAN discovery")]
    NoUsableIpv4Address,
    #[error("{context}")]
    Mdns {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("{context}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("spawn presence thread")]
    SpawnPresenceThread {
        #[source]
        source: std::io::Error,
    },
    #[error("presence ping reply from unexpected address")]
    PresenceUnexpectedReply,
    #[error("presence ping invalid pong")]
    PresenceInvalidPong,
}

/// Collects all non-loopback IPv4 addresses across every local interface.
///
/// VPN / tunnel interfaces are included intentionally: the advertiser publishes
/// all of them, and the scanner pings each one — the real Wi-Fi IP answers
/// while VPN IPs silently time out, so the device is still discovered.
fn all_local_ipv4_addrs() -> Vec<Ipv4Addr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.addr.ip() {
            IpAddr::V4(ip) => Some(ip),
            _ => None,
        })
        .collect()
}

fn chunk_ascii(s: &str, max: usize) -> Vec<String> {
    s.as_bytes()
        .chunks(max)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect()
}

/// Random `recv-xxxx` instance name (does not embed the rendezvous pairing code).
fn random_mdns_instance_name() -> String {
    let mut rng = rand::thread_rng();
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let suffix: String = (0..10)
        .map(|_| *CHARS.choose(&mut rng).unwrap() as char)
        .collect();
    format!("recv-{suffix}")
}

fn ticket_from_txt(txt: &TxtProperties) -> Option<String> {
    let n = txt.get_property_val_str("tc")?.parse::<usize>().ok()?;
    let mut out = String::new();
    for i in 0..n {
        let piece = txt.get_property_val_str(&format!("t{i}"))?;
        out.push_str(piece);
    }
    Some(out)
}

fn device_type_from_txt(txt: &TxtProperties) -> DeviceType {
    match txt.get_property_val_str("dt") {
        Some("phone") => DeviceType::Phone,
        Some("laptop") | None | Some(_) => DeviceType::Laptop,
    }
}

fn device_type_to_txt(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::Phone => "phone",
        DeviceType::Laptop => "laptop",
    }
}

fn build_presence_packet(op: u8, nonce: u64) -> [u8; PRESENCE_PKT_LEN] {
    let mut b = [0u8; PRESENCE_PKT_LEN];
    b[0..4].copy_from_slice(PRESENCE_MAGIC);
    b[4..6].copy_from_slice(&PRESENCE_VER.to_be_bytes());
    b[6] = op;
    b[7] = 0;
    b[8..16].copy_from_slice(&nonce.to_be_bytes());
    b
}

fn parse_presence_pong(buf: &[u8], expected_nonce: u64) -> bool {
    if buf.len() != PRESENCE_PKT_LEN {
        return false;
    }
    if &buf[0..4] != PRESENCE_MAGIC {
        return false;
    }
    if u16::from_be_bytes([buf[4], buf[5]]) != PRESENCE_VER {
        return false;
    }
    if buf[6] != OP_PONG {
        return false;
    }
    u64::from_be_bytes(buf[8..16].try_into().unwrap()) == expected_nonce
}

/// Returns true if the peer echoed our nonce over UDP within `timeout`.
pub fn presence_ping(target: SocketAddr, timeout: Duration) -> std::result::Result<(), LanError> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|source| LanError::Io {
        context: "presence ping bind",
        source,
    })?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|source| LanError::Io {
            context: "presence ping set_read_timeout",
            source,
        })?;

    let nonce: u64 = rand::random();
    let pkt = build_presence_packet(OP_PING, nonce);
    socket
        .send_to(&pkt, target)
        .map_err(|source| LanError::Io {
            context: "presence ping send_to",
            source,
        })?;

    let mut buf = [0u8; PRESENCE_PKT_LEN];
    let (n, from) = socket.recv_from(&mut buf).map_err(|source| LanError::Io {
        context: "presence ping recv_from",
        source,
    })?;
    if from != target {
        return Err(LanError::PresenceUnexpectedReply);
    }
    if !parse_presence_pong(&buf[..n], nonce) {
        return Err(LanError::PresenceInvalidPong);
    }
    Ok(())
}

/// Tries each IPv4 until one answers the presence ping; returns the responding address.
fn verify_presence(info: &mdns_sd::ResolvedService) -> Option<Ipv4Addr> {
    if info.get_port() != WISP_LAN_PRESENCE_PORT {
        warn!(
            service = %info.get_fullname(),
            port = info.get_port(),
            expected = WISP_LAN_PRESENCE_PORT,
            "lan_scan.verify_presence: wrong port — skipping",
        );
        return None;
    }
    let addrs: Vec<_> = info.get_addresses_v4().into_iter().collect();
    if addrs.is_empty() {
        warn!(service = %info.get_fullname(), "lan_scan.verify_presence: no IPv4 addresses in record");
        return None;
    }
    let timeout = Duration::from_millis(400);
    for ip in &addrs {
        let target = SocketAddr::new(IpAddr::V4(*ip), info.get_port());
        match presence_ping(target, timeout) {
            Ok(()) => {
                debug!(
                    service = %info.get_fullname(),
                    %target,
                    "lan_scan.presence_ping: OK",
                );
                return Some(*ip);
            }
            Err(ref e) => {
                debug!(
                    service = %info.get_fullname(),
                    %target,
                    error = %e,
                    "lan_scan.presence_ping: failed",
                );
            }
        }
    }
    warn!(
        service = %info.get_fullname(),
        ips = ?addrs,
        "lan_scan.verify_presence: all IPs failed ping — device not reachable",
    );
    None
}

/// Answers [`WISP_LAN_PRESENCE_PORT`] UDP datagrams while alive.
pub struct PresenceResponder {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl PresenceResponder {
    pub fn bind(port: u16) -> std::result::Result<Self, LanError> {
        let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))).map_err(|source| {
            LanError::Io {
                context: "binding presence UDP",
                source,
            }
        })?;
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|source| LanError::Io {
                context: "presence responder set_read_timeout",
                source,
            })?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("wisp-lan-presence".into())
            .spawn(move || run_presence_loop(socket, stop_t))
            .map_err(|source| LanError::SpawnPresenceThread { source })?;

        Ok(Self {
            stop,
            join: Some(join),
        })
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for PresenceResponder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_presence_loop(socket: UdpSocket, stop: Arc<AtomicBool>) {
    let mut buf = [0u8; 256];
    while !stop.load(Ordering::SeqCst) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if n != PRESENCE_PKT_LEN {
                    continue;
                }
                let p = &buf[..n];
                if &p[0..4] != PRESENCE_MAGIC {
                    continue;
                }
                if u16::from_be_bytes([p[4], p[5]]) != PRESENCE_VER {
                    continue;
                }
                if p[6] != OP_PING {
                    continue;
                }
                let nonce = u64::from_be_bytes(p[8..16].try_into().unwrap());
                let pong = build_presence_packet(OP_PONG, nonce);
                let _ = socket.send_to(&pong, from);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Broadcast discovery responder (receiver side)
// ---------------------------------------------------------------------------

/// Listens on [`WISP_LAN_DISCOVERY_PORT`] for UDP broadcast QUERY packets and
/// replies unicast with the device's ticket + label.  Works on Android hotspot
/// where mDNS multicast is blocked by the SoftAP driver.
pub struct BroadcastDiscoveryResponder {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl BroadcastDiscoveryResponder {
    pub fn bind(
        port: u16,
        ticket: String,
        label: String,
        device_type: DeviceType,
    ) -> std::result::Result<Self, LanError> {
        let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))).map_err(|source| {
            LanError::Io {
                context: "binding broadcast discovery UDP",
                source,
            }
        })?;
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|source| LanError::Io {
                context: "broadcast discovery set_read_timeout",
                source,
            })?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("wisp-lan-discovery".into())
            .spawn(move || run_discovery_responder_loop(socket, stop_t, ticket, label, device_type))
            .map_err(|source| LanError::SpawnPresenceThread { source })?;

        Ok(Self {
            stop,
            join: Some(join),
        })
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for BroadcastDiscoveryResponder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_discovery_responder_loop(
    socket: UdpSocket,
    stop: Arc<AtomicBool>,
    ticket: String,
    label: String,
    device_type: DeviceType,
) {
    let dt_byte: u8 = match device_type {
        DeviceType::Phone => 1,
        DeviceType::Laptop => 0,
    };
    let label_bytes = label.as_bytes();
    let ticket_bytes = ticket.as_bytes();

    let mut buf = [0u8; 256];
    while !stop.load(Ordering::SeqCst) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if n < DISCOVERY_QUERY_LEN {
                    continue;
                }
                let p = &buf[..n];
                if &p[0..4] != DISCOVERY_MAGIC {
                    continue;
                }
                if u16::from_be_bytes([p[4], p[5]]) != DISCOVERY_VER {
                    continue;
                }
                if p[6] != OP_QUERY {
                    continue;
                }
                let nonce = u64::from_be_bytes(p[8..16].try_into().unwrap());

                // Build variable-length reply: header + label + ticket
                let mut reply = Vec::with_capacity(
                    DISCOVERY_QUERY_LEN + 4 + label_bytes.len() + ticket_bytes.len(),
                );
                reply.extend_from_slice(DISCOVERY_MAGIC);
                reply.extend_from_slice(&DISCOVERY_VER.to_be_bytes());
                reply.push(OP_DREPLY);
                reply.push(dt_byte);
                reply.extend_from_slice(&nonce.to_be_bytes());
                reply.extend_from_slice(&(label_bytes.len() as u16).to_be_bytes());
                reply.extend_from_slice(label_bytes);
                reply.extend_from_slice(&(ticket_bytes.len() as u16).to_be_bytes());
                reply.extend_from_slice(ticket_bytes);

                debug!(%from, "lan_discovery.query_received - sending reply");
                let _ = socket.send_to(&reply, from);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Broadcast discovery scanner (sender side)
// ---------------------------------------------------------------------------

fn parse_discovery_reply(buf: &[u8], expected_nonce: u64) -> Option<NearbyReceiver> {
    if buf.len() < DISCOVERY_QUERY_LEN + 4 {
        return None;
    }
    if &buf[0..4] != DISCOVERY_MAGIC {
        return None;
    }
    if u16::from_be_bytes([buf[4], buf[5]]) != DISCOVERY_VER {
        return None;
    }
    if buf[6] != OP_DREPLY {
        return None;
    }
    let device_type = if buf[7] == 1 {
        DeviceType::Phone
    } else {
        DeviceType::Laptop
    };
    let nonce = u64::from_be_bytes(buf[8..16].try_into().ok()?);
    if nonce != expected_nonce {
        return None;
    }

    let mut pos = 16usize;
    let label_len = u16::from_be_bytes([*buf.get(pos)?, *buf.get(pos + 1)?]) as usize;
    pos += 2;
    let label = String::from_utf8(buf.get(pos..pos + label_len)?.to_vec()).ok()?;
    pos += label_len;
    let ticket_len = u16::from_be_bytes([*buf.get(pos)?, *buf.get(pos + 1)?]) as usize;
    pos += 2;
    let ticket = String::from_utf8(buf.get(pos..pos + ticket_len)?.to_vec()).ok()?;

    Some(NearbyReceiver {
        fullname: format!("broadcast-{}", &ticket[..ticket.len().min(12)]),
        label,
        device_type,
        code: String::new(),
        ticket,
    })
}

/// Sends a UDP broadcast query and collects replies for `timeout`.
/// Works on networks where mDNS multicast is blocked (e.g. Android hotspot).
/// Returns each found receiver together with the unicast source IP of its reply.
fn broadcast_scan(timeout: Duration, nonce: u64) -> Vec<(NearbyReceiver, Ipv4Addr)> {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "lan_scan.broadcast_scan: bind failed");
            return vec![];
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        warn!(error = %e, "lan_scan.broadcast_scan: set_broadcast failed");
        return vec![];
    }
    let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));

    let mut query = [0u8; DISCOVERY_QUERY_LEN];
    query[0..4].copy_from_slice(DISCOVERY_MAGIC);
    query[4..6].copy_from_slice(&DISCOVERY_VER.to_be_bytes());
    query[6] = OP_QUERY;
    query[8..16].copy_from_slice(&nonce.to_be_bytes());

    // Send to global broadcast + per-interface /24 subnet broadcasts.
    let targets: Vec<SocketAddr> = {
        let mut t = vec![SocketAddr::from((
            [255, 255, 255, 255],
            WISP_LAN_DISCOVERY_PORT,
        ))];
        for ip in all_local_ipv4_addrs() {
            let o = ip.octets();
            let subnet_broadcast = Ipv4Addr::new(o[0], o[1], o[2], 255);
            t.push(SocketAddr::new(
                IpAddr::V4(subnet_broadcast),
                WISP_LAN_DISCOVERY_PORT,
            ));
        }
        t.dedup();
        t
    };
    for target in &targets {
        let _ = socket.send_to(&query, target);
    }
    info!(targets = ?targets, "lan_scan.broadcast_query_sent");

    let deadline = Instant::now() + timeout;
    let mut results: HashMap<String, (NearbyReceiver, Ipv4Addr)> = HashMap::new();
    let mut buf = vec![0u8; 4096];

    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(receiver) = parse_discovery_reply(&buf[..n], nonce) {
                    if let IpAddr::V4(ip) = from.ip() {
                        debug!(%from, label = %receiver.label, "lan_scan.broadcast_reply_received");
                        results.insert(receiver.ticket.clone(), (receiver, ip));
                    }
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }

    info!(found = results.len(), "lan_scan.broadcast_scan_done");
    results.into_values().collect()
}

// ---------------------------------------------------------------------------

/// Holds mDNS registration for `receive`; unregister on drop.
pub struct LanReceiveAdvertisement {
    fullname: String,
    daemon: ManuallyDrop<ServiceDaemon>,
    presence: ManuallyDrop<PresenceResponder>,
    discovery: ManuallyDrop<BroadcastDiscoveryResponder>,
}

impl LanReceiveAdvertisement {
    /// Publishes the given iroh `ticket` (same string as rendezvous) on the LAN.
    ///
    /// Returns `Ok(None)` when no IPv4 interface is available.
    ///
    /// All non-loopback IPv4 addresses (including VPN tunnel IPs) are included
    /// in the mDNS record via [`ServiceInfo::enable_addr_auto`].  The scanner
    /// pings every advertised address; whichever one responds (the real Wi-Fi
    /// IP) determines whether the device is shown.
    pub fn start(
        ticket: &str,
        device_label: &str,
        device_type: DeviceType,
    ) -> std::result::Result<Option<Self>, LanError> {
        let all_ips = all_local_ipv4_addrs();
        let seed_ip = match all_ips.first().copied() {
            Some(ip) => ip,
            None => {
                info!("lan_advertisement.no_ipv4_interface — skipping");
                return Ok(None);
            }
        };
        info!(advertised_ips = ?all_ips, %device_label, "lan_advertisement.starting");

        let presence = PresenceResponder::bind(WISP_LAN_PRESENCE_PORT)
            .map_err(|source| LanError::mdns("starting LAN presence responder", source))?;

        let discovery = BroadcastDiscoveryResponder::bind(
            WISP_LAN_DISCOVERY_PORT,
            ticket.to_owned(),
            device_label.to_owned(),
            device_type,
        )
        .map_err(|source| LanError::mdns("starting LAN broadcast discovery responder", source))?;

        // Use the seed IP only as a required constructor argument; enable_addr_auto()
        // replaces it with all local addresses so every interface is represented.
        let host_name = format!("{seed_ip}.local.");
        let instance = random_mdns_instance_name();

        let chunks = chunk_ascii(ticket, TICKET_CHUNK_LEN);
        let mut properties: Vec<(String, String)> = vec![
            ("ver".into(), WISP_MDNS_TXT_VER.into()),
            ("label".into(), device_label.to_owned()),
            ("dt".into(), device_type_to_txt(device_type).into()),
            ("tc".into(), chunks.len().to_string()),
        ];
        for (i, c) in chunks.iter().enumerate() {
            properties.push((format!("t{i}"), c.clone()));
        }

        let txt: Vec<(&str, &str)> = properties
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let service = ServiceInfo::new(
            WISP_MDNS_SERVICE_TYPE,
            &instance,
            &host_name,
            IpAddr::V4(seed_ip),
            WISP_LAN_PRESENCE_PORT,
            txt.as_slice(),
        )
        .map_err(|source| LanError::mdns("building mDNS service info", source))?
        .enable_addr_auto();

        let fullname = service.get_fullname().to_owned();
        let daemon = ServiceDaemon::new()
            .map_err(|source| LanError::mdns("creating mDNS daemon", source))?;
        if let Err(e) = daemon.register(service) {
            return Err(LanError::mdns("registering mDNS wisp receive service", e));
        }

        Ok(Some(Self {
            fullname,
            daemon: ManuallyDrop::new(daemon),
            presence: ManuallyDrop::new(presence),
            discovery: ManuallyDrop::new(discovery),
        }))
    }
}

impl Drop for LanReceiveAdvertisement {
    fn drop(&mut self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
        std::thread::sleep(Duration::from_millis(100));
        unsafe {
            ManuallyDrop::drop(&mut self.presence);
            ManuallyDrop::drop(&mut self.discovery);
        }
        if let Ok(rx) = self.daemon.shutdown() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
        unsafe {
            ManuallyDrop::drop(&mut self.daemon);
        }
    }
}

/// One resolved nearby receiver from mDNS.
#[derive(Debug, Clone)]
pub struct NearbyReceiver {
    pub fullname: String,
    pub label: String,
    pub device_type: DeviceType,
    /// Always empty for current advertisers (pairing code is not published on LAN).
    pub code: String,
    pub ticket: String,
}

/// Browse for `scan` duration and return the latest snapshot of matching receivers.
///
/// Only includes services that answer the UDP presence protocol on [`WISP_LAN_PRESENCE_PORT`].
///
/// When `exclude_endpoint_id` is set, drops entries whose mDNS ticket decodes to that iroh
/// endpoint id (same process advertising while browsing, e.g. Flutter idle receive + send UI).
pub fn browse_nearby_receivers(
    scan: Duration,
    exclude_endpoint_id: Option<EndpointId>,
) -> std::result::Result<Vec<NearbyReceiver>, LanError> {
    let local_ips = all_local_ipv4_addrs();
    info!(ips = ?local_ips, scan_secs = scan.as_secs(), "lan_scan.browse_start");

    // Run UDP broadcast scan in a parallel thread so it overlaps with mDNS.
    let nonce: u64 = rand::random();
    let broadcast_handle = std::thread::spawn(move || broadcast_scan(scan, nonce));

    // Snapshot the IP cache and start pinging known IPs in parallel threads.
    // Each ping has a 300 ms deadline; since they run concurrently the total
    // wait is bounded by one ping round-trip regardless of how many entries exist.
    let cache_snapshot: Vec<(String, Ipv4Addr, u16, NearbyReceiver)> = {
        let cache = peer_cache().lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        cache
            .iter()
            .filter(|(_, e)| now.duration_since(e.seen_at) < CACHE_TTL)
            .map(|(t, e)| (t.clone(), e.ip, e.port, e.receiver.clone()))
            .collect()
    };
    let cache_ping_handles: Vec<_> = cache_snapshot
        .into_iter()
        .map(|(ticket, ip, port, receiver)| {
            std::thread::spawn(move || {
                let target = SocketAddr::new(IpAddr::V4(ip), port);
                match presence_ping(target, Duration::from_millis(300)) {
                    Ok(()) => {
                        debug!(%ip, %port, label = %receiver.label, "lan_scan.cache_hit");
                        Some((ticket, ip, port, receiver))
                    }
                    Err(_) => None,
                }
            })
        })
        .collect();

    let daemon =
        ServiceDaemon::new().map_err(|source| LanError::mdns("creating mDNS daemon", source))?;
    let browse_rx = daemon
        .browse(WISP_MDNS_SERVICE_TYPE)
        .map_err(|source| LanError::mdns("starting mDNS browse", source))?;

    // Collect cache hits (≤300 ms, overlaps with mDNS daemon startup).
    // Both maps are keyed by *ticket* so mDNS and broadcast results dedup naturally.
    let mut peers: HashMap<String, NearbyReceiver> = HashMap::new();
    let mut peer_ips: HashMap<String, (Ipv4Addr, u16)> = HashMap::new();
    for handle in cache_ping_handles {
        if let Ok(Some((ticket, ip, port, receiver))) = handle.join() {
            peer_ips.entry(ticket.clone()).or_insert((ip, port));
            peers.entry(ticket).or_insert(receiver);
        }
    }
    if !peers.is_empty() {
        info!(seeded = peers.len(), "lan_scan.cache_seeded");
    }

    let deadline = Instant::now() + scan;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = Duration::from_millis(250).min(remaining);
        if wait.is_zero() {
            break;
        }

        match browse_rx.recv_timeout(wait) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let addrs: Vec<_> = info.get_addresses_v4().into_iter().collect();
                debug!(
                    service = %info.get_fullname(),
                    ips = ?addrs,
                    port = info.get_port(),
                    "lan_scan.service_resolved",
                );

                if !info.is_valid() {
                    warn!(service = %info.get_fullname(), "lan_scan.filter: invalid service");
                    continue;
                }
                let ver = info.get_properties().get_property_val_str("ver");
                if ver != Some(WISP_MDNS_TXT_VER) {
                    warn!(
                        service = %info.get_fullname(),
                        got = ?ver,
                        expected = WISP_MDNS_TXT_VER,
                        "lan_scan.filter: ver mismatch",
                    );
                    continue;
                }
                let Some(ticket) = ticket_from_txt(info.get_properties()) else {
                    warn!(service = %info.get_fullname(), "lan_scan.filter: missing ticket TXT");
                    continue;
                };
                let Some(ip) = verify_presence(&info) else {
                    continue;
                };
                let label = info
                    .get_properties()
                    .get_property_val_str("label")
                    .unwrap_or("Wisp receiver")
                    .to_owned();
                info!(service = %info.get_fullname(), %label, "lan_scan.peer_added");
                // mDNS is authoritative — always overwrite cache-seeded entry.
                peer_ips.insert(ticket.clone(), (ip, WISP_LAN_PRESENCE_PORT));
                peers.insert(
                    ticket.clone(),
                    NearbyReceiver {
                        fullname: info.get_fullname().to_owned(),
                        label,
                        device_type: device_type_from_txt(info.get_properties()),
                        code: String::new(),
                        ticket,
                    },
                );
            }
            Ok(ServiceEvent::ServiceRemoved(_ty_domain, fullname)) => {
                // Peers are now keyed by ticket, so we can't remove by fullname directly.
                // Flutter-side staleness handling takes care of eventually evicting gone peers.
                debug!(service = %fullname, "lan_scan.service_removed (ignored — keyed by ticket)");
            }
            Ok(ev) => {
                debug!(event = ?ev, "lan_scan.mdns_event");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    info!(mdns_found = peers.len(), "lan_scan.browse_done");

    if let Ok(rx) = daemon.shutdown() {
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }

    // Merge broadcast results — dedup by ticket (prefer mDNS entry when both found).
    if let Ok(broadcast_results) = broadcast_handle.join() {
        for (peer, ip) in broadcast_results {
            let ticket = peer.ticket.clone();
            // Record IP for cache update (don't overwrite if mDNS already has it).
            peer_ips
                .entry(ticket.clone())
                .or_insert((ip, WISP_LAN_PRESENCE_PORT));
            // Only insert if this ticket wasn't already seen via mDNS.
            peers.entry(ticket).or_insert(peer);
        }
    }
    info!(total = peers.len(), "lan_scan.merged");

    // Persist discovered endpoints to the cross-scan IP cache.
    {
        let mut cache = peer_cache().lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        for (ticket, receiver) in &peers {
            if let Some(&(ip, port)) = peer_ips.get(ticket) {
                cache.insert(
                    ticket.clone(),
                    CachedEntry {
                        ip,
                        port,
                        receiver: receiver.clone(),
                        seen_at: now,
                    },
                );
            }
        }
        // Evict entries older than CACHE_TTL.
        cache.retain(|_, e| now.duration_since(e.seen_at) < CACHE_TTL);
        debug!(cache_size = cache.len(), "lan_scan.cache_updated");
    }

    let mut list: Vec<NearbyReceiver> = peers.into_values().collect();
    if let Some(exclude) = exclude_endpoint_id {
        list.retain(|r| {
            crate::util::decode_ticket(r.ticket.trim())
                .map(|addr| addr.id != exclude)
                .unwrap_or(true)
        });
    }
    list.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.fullname.cmp(&b.fullname))
    });
    Ok(list)
}

impl LanError {
    fn mdns(context: &'static str, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::Mdns {
            context,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use mdns_sd::IntoTxtProperties;

    use super::*;

    #[test]
    fn ticket_roundtrip_txt_chunks() {
        let ticket = "a".repeat(450);
        let chunks = chunk_ascii(&ticket, TICKET_CHUNK_LEN);
        assert_eq!(chunks.len(), 3);

        let mut m: HashMap<String, String> = HashMap::new();
        m.insert("ver".into(), WISP_MDNS_TXT_VER.into());
        m.insert("dt".into(), "phone".into());
        m.insert("tc".into(), chunks.len().to_string());
        for (i, c) in chunks.iter().enumerate() {
            m.insert(format!("t{i}"), c.clone());
        }
        let txt = m.into_txt_properties();
        let got = ticket_from_txt(&txt).expect("reassembled");
        assert_eq!(got, ticket);
        assert_eq!(device_type_from_txt(&txt), DeviceType::Phone);
    }

    #[test]
    fn missing_device_type_defaults_to_laptop() {
        let txt = HashMap::<String, String>::new().into_txt_properties();
        assert_eq!(device_type_from_txt(&txt), DeviceType::Laptop);
    }

    #[test]
    fn presence_ping_pong_localhost() {
        let socket = match UdpSocket::bind("127.0.0.1:0") {
            Ok(socket) => socket,
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(1) =>
            {
                return;
            }
            Err(error) => panic!("bind: {error}"),
        };
        let port = socket.local_addr().unwrap().port();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = std::thread::spawn(move || run_presence_loop(socket, stop_t));

        let target = SocketAddr::from(([127, 0, 0, 1], port));
        presence_ping(target, Duration::from_secs(1)).expect("ping");

        stop.store(true, Ordering::SeqCst);
        join.join().unwrap();
    }
}
