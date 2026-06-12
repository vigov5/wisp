mod device_name;

pub use device_name::{normalize_hostname_label, process_display_device_name, random_device_name};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::endpoint::TransportAddrUsage;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransferTicket {
    node_id: String,
    addrs: Vec<EncodedTransportAddr>,
}

/// JSON envelope used by QR pairing. Carries the original ticket plus
/// device info so the sender can show "From <name>" on its tile before
/// dialing.  Wire-format separate from `TransferTicket` (bincode) so the
/// LAN broadcast stays binary-compatible with older wisp binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct QrPayload {
    /// The same string that `make_ticket` / `make_ticket_offline` produce —
    /// base64 of bincode `TransferTicket`.
    ticket: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    device_type: String,
}

/// Magic prefix on a QR-encoded pairing payload so the scanner can
/// distinguish it from a raw ticket string. Short enough to keep the
/// QR small while still being self-describing.
const QR_PAYLOAD_PREFIX: &str = "wisp-pair:";

/// Public read-only view of a decoded ticket. Returned by [`decode_ticket_info`]
/// so callers (e.g., Flutter QR scan) can pre-populate UI before dialing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTicketInfo {
    pub endpoint_addr: EndpointAddr,
    /// Empty string when the ticket didn't carry a name (older format).
    pub device_name: String,
    pub device_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum EncodedTransportAddr {
    Relay(String),
    Ip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPathKind {
    Direct,
    Relay,
    Unknown,
}

/// Snapshot of the connection path between the local endpoint and a remote peer,
/// plus the relay URL when the path is relay-only.
///
/// Derivation: we iterate `RemoteInfo::addrs()` and only consider entries whose
/// `usage() == Active`. "Direct" means iroh is actively using a direct UDP
/// address; "Relay" means it's actively using a relay. Inactive (advertised but
/// not in use) candidates are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPath {
    pub kind: ConnectionPathKind,
    pub relay_url: Option<String>,
    /// Active direct UDP socket address ("ip:port" form) when `kind == Direct`,
    /// otherwise `None`. Useful for debugging / showing peer reachability info.
    pub direct_addr: Option<String>,
}

impl ConnectionPath {
    pub fn unknown() -> Self {
        Self {
            kind: ConnectionPathKind::Unknown,
            relay_url: None,
            direct_addr: None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            ConnectionPathKind::Direct => "p2p",
            ConnectionPathKind::Relay => "relay",
            ConnectionPathKind::Unknown => "unknown",
        }
    }
}

impl Default for ConnectionPath {
    fn default() -> Self {
        Self::unknown()
    }
}

/// Pure helper: classify a sequence of `TransportAddr`s into a `ConnectionPath`.
/// Extracted for unit testing without an iroh `Endpoint`. The async wrapper
/// `snapshot_connection_path` inlines this logic to keep its borrow lifetimes
/// trivially correct.
#[cfg(test)]
fn classify_addrs<'a, I>(addrs: I) -> ConnectionPath
where
    I: IntoIterator<Item = &'a TransportAddr>,
{
    let mut first_direct: Option<String> = None;
    let mut first_relay: Option<String> = None;
    for addr in addrs {
        match addr {
            TransportAddr::Ip(socket) => {
                if first_direct.is_none() {
                    first_direct = Some(socket.to_string());
                }
            }
            TransportAddr::Relay(url) => {
                if first_relay.is_none() {
                    first_relay = Some(url.to_string());
                }
            }
            _ => {}
        }
    }

    let kind = if first_direct.is_some() {
        ConnectionPathKind::Direct
    } else if first_relay.is_some() {
        ConnectionPathKind::Relay
    } else {
        ConnectionPathKind::Unknown
    };
    let relay_url = match kind {
        ConnectionPathKind::Relay => first_relay,
        _ => None,
    };
    let direct_addr = match kind {
        ConnectionPathKind::Direct => first_direct,
        _ => None,
    };
    ConnectionPath {
        kind,
        relay_url,
        direct_addr,
    }
}

#[derive(Debug, Error)]
pub enum TicketError {
    #[error("serializing transfer ticket")]
    Serialize {
        #[source]
        source: Box<bincode::ErrorKind>,
    },
    #[error("decoding ticket from base64")]
    DecodeBase64 {
        #[source]
        source: base64::DecodeError,
    },
    #[error("ticket payload is not a supported wisp ticket")]
    InvalidPayload,
    #[error("parsing node id {value}")]
    ParseNodeId {
        value: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("parsing relay url {value}")]
    ParseRelayUrl {
        value: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("parsing socket addr {value}")]
    ParseSocketAddr {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },
}

#[derive(Debug, Error)]
pub enum ConfirmAcceptError {
    #[error("flushing prompt")]
    FlushPrompt {
        #[source]
        source: io::Error,
    },
    #[error("reading confirmation")]
    ReadConfirmation {
        #[source]
        source: io::Error,
    },
}

/// Snapshot the current connection path to `remote_id`, including relay URL
/// when the path is relay-only. See [`ConnectionPath`] for semantics.
///
/// We filter by `TransportAddrInfo::usage() == Active` so we report what iroh
/// is *actually* using, not just every candidate it discovered. Without this,
/// cross-NAT scenarios (e.g. WiFi ↔ 4G) report "Direct" even when the QUIC
/// connection is flowing through the relay, because iroh still advertises the
/// STUN-discovered direct candidate.
pub async fn snapshot_connection_path(
    endpoint: &iroh::Endpoint,
    remote_id: iroh::EndpointId,
) -> ConnectionPath {
    let Some(info) = endpoint.remote_info(remote_id).await else {
        return ConnectionPath::unknown();
    };

    let mut first_direct: Option<String> = None;
    let mut first_relay: Option<String> = None;
    for addr in info.addrs() {
        if !matches!(addr.usage(), TransportAddrUsage::Active) {
            continue;
        }
        match addr.addr() {
            TransportAddr::Ip(socket) => {
                if first_direct.is_none() {
                    first_direct = Some(socket.to_string());
                }
            }
            TransportAddr::Relay(url) => {
                if first_relay.is_none() {
                    first_relay = Some(url.to_string());
                }
            }
            _ => {}
        }
    }

    let kind = if first_direct.is_some() {
        ConnectionPathKind::Direct
    } else if first_relay.is_some() {
        ConnectionPathKind::Relay
    } else {
        ConnectionPathKind::Unknown
    };
    let relay_url = match kind {
        ConnectionPathKind::Relay => first_relay,
        _ => None,
    };
    let direct_addr = match kind {
        ConnectionPathKind::Direct => first_direct,
        _ => None,
    };
    ConnectionPath {
        kind,
        relay_url,
        direct_addr,
    }
}

pub async fn classify_connection_path(
    endpoint: &iroh::Endpoint,
    remote_id: iroh::EndpointId,
) -> ConnectionPathKind {
    snapshot_connection_path(endpoint, remote_id).await.kind
}

/// Return the peer's relay URL if iroh knows one, **regardless of whether the
/// relay is the actively-used path**.
///
/// [`snapshot_connection_path`] only reports `relay_url` when the relay is the
/// *active* path, so a direct (LAN) transfer yields `relay_url: None` even
/// though iroh still tracks the peer's home relay as a fallback candidate. That
/// is the right call for the connection-path badge (we show what's in use), but
/// the wrong input for a persisted "send back" ticket: dropping the relay means
/// that when the peer later changes networks (e.g. Wi-Fi → 4G) the saved ticket
/// carries only an unreachable direct IP, and iroh has nothing to fall back to.
///
/// This helper scans every known candidate (any `usage()`) and returns the
/// first relay, so the send-back ticket can always carry a relay fallback.
pub async fn peer_relay_url(
    endpoint: &iroh::Endpoint,
    remote_id: iroh::EndpointId,
) -> Option<String> {
    let info = endpoint.remote_info(remote_id).await?;
    info.addrs().find_map(|addr| match addr.addr() {
        TransportAddr::Relay(url) => Some(url.to_string()),
        _ => None,
    })
}

pub fn confirm_accept() -> std::result::Result<bool, ConfirmAcceptError> {
    print!("Accept? [y/N]: ");
    io::stdout()
        .flush()
        .map_err(|source| ConfirmAcceptError::FlushPrompt { source })?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|source| ConfirmAcceptError::ReadConfirmation { source })?;

    let response = input.trim().to_ascii_lowercase();
    Ok(matches!(response.as_str(), "y" | "yes"))
}

pub fn describe_remote(
    remote_id: iroh::EndpointId,
    remote: Option<&iroh::endpoint::RemoteInfo>,
) -> String {
    let relay = remote
        .and_then(|info| {
            info.addrs().find_map(|addr| match addr.addr() {
                TransportAddr::Relay(url) => Some(format!(" via relay {url}")),
                TransportAddr::Ip(_) => None,
                _ => None,
            })
        })
        .unwrap_or_default();
    format!("{remote_id}{relay}")
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_code_label(code: &str) -> String {
    let normalized = code.trim().to_ascii_uppercase();
    let chars: Vec<char> = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if chars.len() != 6 {
        return "Code".to_owned();
    }

    format!(
        "Code {}{}{} {}{}{}",
        chars[0], chars[1], chars[2], chars[3], chars[4], chars[5]
    )
}

pub async fn make_ticket(endpoint: &Endpoint) -> std::result::Result<String, TicketError> {
    endpoint.online().await;
    make_ticket_from_addr(endpoint.addr())
}

/// Build a ticket from whatever addresses are known *right now*, without
/// waiting on `endpoint.online()` (which pends until a relay handshake
/// completes — fails forever on offline-LAN networks).
pub fn make_ticket_offline(endpoint: &Endpoint) -> std::result::Result<String, TicketError> {
    let mut addr = endpoint.addr();
    let lan = lan_direct_addrs(endpoint);
    for socket in lan {
        addr.addrs.insert(TransportAddr::Ip(socket));
    }
    make_ticket_from_addr(addr)
}

/// Build a QR pairing payload — wraps an offline ticket together with
/// device name + type so the scanning sender can render an informative
/// tile before dialing. Format: `"wisp-pair:" + base64url(json)`.
pub fn make_qr_payload(
    endpoint: &Endpoint,
    device_name: &str,
    device_type: &str,
) -> std::result::Result<String, TicketError> {
    let ticket = make_ticket_offline(endpoint)?;
    let payload = QrPayload {
        ticket,
        device_name: device_name.to_owned(),
        device_type: device_type.to_owned(),
    };
    let json = serde_json::to_vec(&payload).map_err(|_| TicketError::InvalidPayload)?;
    Ok(format!(
        "{QR_PAYLOAD_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(json)
    ))
}

/// Returns the LAN-routable direct socket addresses for this endpoint.
///
/// Strategy:
/// 1. Read the bound UDP port from `endpoint.bound_sockets()` (returns
///    immediately, doesn't depend on discovery).
/// 2. Enumerate local network interfaces via `if_addrs`.
/// 3. Pair each non-loopback, non-link-local IPv4/IPv6 interface IP with
///    the bound port to produce dialable `SocketAddr` values.
///
/// Used both to inject addrs into the offline ticket and to display a
/// list of IPs alongside the QR code so the user can confirm their
/// device is on the expected network before sharing.
pub fn lan_direct_addrs(endpoint: &Endpoint) -> Vec<std::net::SocketAddr> {
    use std::net::{IpAddr, SocketAddr};

    let bound = endpoint.bound_sockets();
    if bound.is_empty() {
        return Vec::new();
    }

    // Pick a single port — iroh typically binds the same port across IPv4/IPv6.
    let port = bound
        .iter()
        .find(|s| !s.ip().is_unspecified())
        .map(|s| s.port())
        .unwrap_or_else(|| bound[0].port());

    let interfaces = match if_addrs::get_if_addrs() {
        Ok(ifs) => ifs,
        Err(err) => {
            tracing::debug!(error = %err, "failed to enumerate network interfaces");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for iface in interfaces {
        let ip = iface.ip();
        let usable = match ip {
            IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
            IpAddr::V6(v6) => {
                !v6.is_loopback()
                    && !v6.is_unspecified()
                    // Exclude link-local fe80::/10 — needs zone id to dial,
                    // unreliable across hosts.
                    && !(v6.segments()[0] & 0xffc0 == 0xfe80)
            }
        };
        if usable {
            out.push(SocketAddr::new(ip, port));
        }
    }
    out
}

/// Round-trips an already-known [`EndpointAddr`] back into the base64 ticket
/// string callers can persist.  Used by the sender to capture the ticket it
/// got from `claim_peer` (which is owned by the rendezvous response, not the
/// caller's request) so the saved-devices list can fast-reconnect later.
pub fn encode_ticket(addr: EndpointAddr) -> std::result::Result<String, TicketError> {
    make_ticket_from_addr(addr)
}

fn make_ticket_from_addr(addr: EndpointAddr) -> std::result::Result<String, TicketError> {
    let ticket = TransferTicket {
        node_id: addr.id.to_string(),
        addrs: addr
            .addrs
            .into_iter()
            .map(EncodedTransportAddr::from)
            .collect(),
    };

    let bytes = bincode::serialize(&ticket).map_err(|source| TicketError::Serialize { source })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Build a ticket string from a peer's `EndpointId` plus optional connection
/// hints. Used by the receiver after a transfer completes to persist a
/// "send back" ticket without requiring an active iroh `Endpoint`.
///
/// Empty / blank hint strings are dropped silently. Invalid relay URLs or
/// socket addresses produce a `TicketError`. Callers in event-emission paths
/// should treat errors as "no ticket"; iroh's pkarr discovery can resolve a
/// ticket carrying only an `EndpointId`, so empty `addrs` is acceptable.
pub fn synthesize_ticket(
    endpoint_id: iroh::EndpointId,
    relay_url: Option<&str>,
    direct_addr: Option<&str>,
) -> std::result::Result<String, TicketError> {
    let mut addrs: Vec<EncodedTransportAddr> = Vec::new();

    if let Some(s) = direct_addr.map(str::trim).filter(|s| !s.is_empty()) {
        // Validate via parse; the on-the-wire form keeps the original string.
        let _: std::net::SocketAddr = s.parse().map_err(|source| TicketError::ParseSocketAddr {
            value: s.to_owned(),
            source,
        })?;
        addrs.push(EncodedTransportAddr::Ip(s.to_owned()));
    }

    if let Some(s) = relay_url.map(str::trim).filter(|s| !s.is_empty()) {
        let _: iroh::RelayUrl = s.parse().map_err(|source| TicketError::ParseRelayUrl {
            value: s.to_owned(),
            source: Box::new(source),
        })?;
        addrs.push(EncodedTransportAddr::Relay(s.to_owned()));
    }

    let ticket = TransferTicket {
        node_id: endpoint_id.to_string(),
        addrs,
    };

    let bytes = bincode::serialize(&ticket).map_err(|source| TicketError::Serialize { source })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Decode a QR-pairing payload OR a plain ticket string. Returns the
/// endpoint addr plus optional device name/type — both empty for plain
/// tickets that don't carry that info.
///
/// Accepted inputs:
/// - `"wisp-pair:<base64url(json{ticket, device_name, device_type})>"` — new QR format.
/// - Plain ticket string (base64url of bincode `TransferTicket`) — older QR / paste.
pub fn decode_ticket_info(input: &str) -> std::result::Result<DecodedTicketInfo, TicketError> {
    let trimmed = input.trim();

    if let Some(rest) = trimmed.strip_prefix(QR_PAYLOAD_PREFIX) {
        let bytes = URL_SAFE_NO_PAD
            .decode(rest)
            .map_err(|source| TicketError::DecodeBase64 { source })?;
        let payload: QrPayload =
            serde_json::from_slice(&bytes).map_err(|_| TicketError::InvalidPayload)?;
        let endpoint_addr = decode_ticket(&payload.ticket)?;
        return Ok(DecodedTicketInfo {
            endpoint_addr,
            device_name: payload.device_name,
            device_type: payload.device_type,
        });
    }

    // Fallback: plain ticket without device info.
    Ok(DecodedTicketInfo {
        endpoint_addr: decode_ticket(trimmed)?,
        device_name: String::new(),
        device_type: String::new(),
    })
}

pub fn decode_ticket(input: &str) -> std::result::Result<EndpointAddr, TicketError> {
    let trimmed = input.trim();
    // Accept QR-payload form too: "wisp-pair:<base64url(json{ticket,...})>" —
    // unwrap to the inner ticket and recurse so the send flow can use the
    // QR-scanned string directly without first stripping the prefix.
    if let Some(rest) = trimmed.strip_prefix(QR_PAYLOAD_PREFIX) {
        let bytes = URL_SAFE_NO_PAD
            .decode(rest)
            .map_err(|source| TicketError::DecodeBase64 { source })?;
        let payload: QrPayload =
            serde_json::from_slice(&bytes).map_err(|_| TicketError::InvalidPayload)?;
        return decode_ticket(&payload.ticket);
    }

    let bytes = URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|source| TicketError::DecodeBase64 { source })?;
    let ticket = parse_transfer_ticket(&bytes)?;

    let node_id = ticket
        .node_id
        .parse()
        .map_err(|source| TicketError::ParseNodeId {
            value: ticket.node_id.clone(),
            source: Box::new(source),
        })?;

    let addrs = ticket
        .addrs
        .into_iter()
        .map(TryInto::try_into)
        .collect::<std::result::Result<Vec<TransportAddr>, TicketError>>()?;

    Ok(EndpointAddr::new(node_id).with_addrs(addrs))
}

fn parse_transfer_ticket(bytes: &[u8]) -> std::result::Result<TransferTicket, TicketError> {
    bincode::deserialize::<TransferTicket>(bytes).map_err(|_| TicketError::InvalidPayload)
}

impl From<TransportAddr> for EncodedTransportAddr {
    fn from(value: TransportAddr) -> Self {
        match value {
            TransportAddr::Relay(url) => Self::Relay(url.to_string()),
            TransportAddr::Ip(addr) => Self::Ip(addr.to_string()),
            _ => unreachable!("unsupported transport address variant"),
        }
    }
}

impl TryFrom<EncodedTransportAddr> for TransportAddr {
    type Error = TicketError;

    fn try_from(value: EncodedTransportAddr) -> std::result::Result<Self, Self::Error> {
        match value {
            EncodedTransportAddr::Relay(url) => {
                Ok(TransportAddr::Relay(url.parse().map_err(|source| {
                    TicketError::ParseRelayUrl {
                        value: url.clone(),
                        source: Box::new(source),
                    }
                })?))
            }
            EncodedTransportAddr::Ip(addr) => {
                Ok(TransportAddr::Ip(addr.parse().map_err(|source| {
                    TicketError::ParseSocketAddr {
                        value: addr.clone(),
                        source,
                    }
                })?))
            }
        }
    }
}

#[cfg(test)]
mod connection_path_tests {
    use super::*;
    use std::net::SocketAddr;

    fn ip_addr() -> TransportAddr {
        let socket: SocketAddr = "127.0.0.1:0".parse().expect("socket");
        TransportAddr::Ip(socket)
    }

    fn relay_addr(url: &str) -> TransportAddr {
        TransportAddr::Relay(url.parse().expect("relay url"))
    }

    #[test]
    fn classify_addrs_returns_unknown_for_empty() {
        let path = classify_addrs(std::iter::empty());
        assert_eq!(path.kind, ConnectionPathKind::Unknown);
        assert!(path.relay_url.is_none());
        assert!(path.direct_addr.is_none());
    }

    #[test]
    fn classify_addrs_returns_relay_when_only_relay_addr() {
        let relay = relay_addr("https://relay.example/");
        let path = classify_addrs([&relay]);
        assert_eq!(path.kind, ConnectionPathKind::Relay);
        assert_eq!(path.relay_url.as_deref(), Some("https://relay.example/"));
        assert!(path.direct_addr.is_none());
    }

    #[test]
    fn classify_addrs_returns_direct_with_first_ip_socket_when_ip_present() {
        let relay = relay_addr("https://relay.example/");
        let ip = ip_addr();
        let path = classify_addrs([&relay, &ip]);
        assert_eq!(path.kind, ConnectionPathKind::Direct);
        assert_eq!(path.direct_addr.as_deref(), Some("127.0.0.1:0"));
        assert!(
            path.relay_url.is_none(),
            "relay_url must be None when path is Direct (no stale relay leak)"
        );
    }

    #[test]
    fn classify_addrs_picks_first_relay_url() {
        let relay_a = relay_addr("https://relay-a.example/");
        let relay_b = relay_addr("https://relay-b.example/");
        let path = classify_addrs([&relay_a, &relay_b]);
        assert_eq!(path.kind, ConnectionPathKind::Relay);
        assert_eq!(path.relay_url.as_deref(), Some("https://relay-a.example/"));
    }

    #[test]
    fn label_returns_stable_strings() {
        assert_eq!(
            ConnectionPath {
                kind: ConnectionPathKind::Direct,
                relay_url: None,
                direct_addr: Some("192.168.1.5:5000".into()),
            }
            .label(),
            "p2p"
        );
        assert_eq!(
            ConnectionPath {
                kind: ConnectionPathKind::Relay,
                relay_url: Some("https://relay.example/".into()),
                direct_addr: None,
            }
            .label(),
            "relay"
        );
        assert_eq!(ConnectionPath::unknown().label(), "unknown");
    }

    #[test]
    fn default_is_unknown_with_no_addrs() {
        let default = ConnectionPath::default();
        assert_eq!(default.kind, ConnectionPathKind::Unknown);
        assert!(default.relay_url.is_none());
        assert!(default.direct_addr.is_none());
    }
}

#[cfg(test)]
mod synthetic_ticket_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[7u8; 32]).public()
    }

    #[test]
    fn empty_hints_round_trip_endpoint_id_only() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, None, None).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert!(decoded.addrs.is_empty());
    }

    #[test]
    fn blank_hints_treated_as_none() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("   "), Some("")).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert!(decoded.addrs.is_empty());
    }

    #[test]
    fn relay_only_round_trips() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("https://relay.example/"), None).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        let has_relay = decoded.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        assert!(has_relay, "decoded ticket missing relay addr");
    }

    #[test]
    fn direct_only_round_trips() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, None, Some("192.168.1.5:5000")).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        let has_ip = decoded
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Ip(s) if s.to_string() == "192.168.1.5:5000"));
        assert!(has_ip, "decoded ticket missing direct addr");
    }

    #[test]
    fn both_hints_round_trip() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("https://relay.example/"), Some("10.0.0.1:1234"))
            .expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert_eq!(decoded.addrs.len(), 2);
    }

    #[test]
    fn invalid_relay_url_errors() {
        let id = sample_id();
        let result = synthesize_ticket(id, Some("not a url"), None);
        assert!(matches!(result, Err(TicketError::ParseRelayUrl { .. })));
    }

    #[test]
    fn invalid_socket_addr_errors() {
        let id = sample_id();
        let result = synthesize_ticket(id, None, Some("not.a.socket"));
        assert!(matches!(result, Err(TicketError::ParseSocketAddr { .. })));
    }
}

#[cfg(test)]
mod qr_payload_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[9u8; 32]).public()
    }

    fn build_qr_payload(ticket: String, name: &str, dtype: &str) -> String {
        let payload = QrPayload {
            ticket,
            device_name: name.to_owned(),
            device_type: dtype.to_owned(),
        };
        let json = serde_json::to_vec(&payload).expect("serialize");
        format!("{QR_PAYLOAD_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    #[test]
    fn decode_ticket_info_round_trips_qr_payload_fields() {
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("192.168.1.5:5000")).expect("inner ticket");
        let qr = build_qr_payload(inner, "Maya MacBook", "laptop");

        let info = decode_ticket_info(&qr).expect("decode info");
        assert_eq!(info.endpoint_addr.id, id);
        assert_eq!(info.device_name, "Maya MacBook");
        assert_eq!(info.device_type, "laptop");
    }

    #[test]
    fn decode_ticket_info_returns_empty_device_fields_for_plain_ticket() {
        let id = sample_id();
        let plain = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");

        let info = decode_ticket_info(&plain).expect("decode plain");
        assert_eq!(info.endpoint_addr.id, id);
        assert!(info.device_name.is_empty());
        assert!(info.device_type.is_empty());
    }

    #[test]
    fn decode_ticket_accepts_wisp_pair_prefix_and_unwraps() {
        let id = sample_id();
        let inner = synthesize_ticket(id, Some("https://relay.example/"), None).expect("inner");
        let qr = build_qr_payload(inner, "Phone", "phone");

        let addr = decode_ticket(&qr).expect("decode");
        assert_eq!(addr.id, id);
        let has_relay = addr.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        assert!(has_relay, "expected relay addr to survive QR-wrap unwrap");
    }

    #[test]
    fn decode_ticket_info_tolerates_surrounding_whitespace() {
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");
        let qr = build_qr_payload(inner, "Pad", "laptop");
        let padded = format!("\n\t  {qr}  \n");

        let info = decode_ticket_info(&padded).expect("decode padded");
        assert_eq!(info.endpoint_addr.id, id);
        assert_eq!(info.device_name, "Pad");
    }

    #[test]
    fn decode_ticket_info_errors_on_malformed_base64_after_prefix() {
        let bogus = format!("{QR_PAYLOAD_PREFIX}!!!not-base64!!!");
        let result = decode_ticket_info(&bogus);
        assert!(matches!(result, Err(TicketError::DecodeBase64 { .. })));
    }

    #[test]
    fn decode_ticket_info_errors_on_non_json_payload_after_prefix() {
        let raw = URL_SAFE_NO_PAD.encode(b"this is not json");
        let bogus = format!("{QR_PAYLOAD_PREFIX}{raw}");
        let result = decode_ticket_info(&bogus);
        assert!(matches!(result, Err(TicketError::InvalidPayload)));
    }

    #[test]
    fn qr_payload_missing_device_fields_decode_as_empty() {
        // Older sender wrote `{"ticket": "..."}` only — the decoder must still
        // accept it via serde(default) instead of erroring on missing fields.
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");
        let json = format!(r#"{{"ticket":"{}"}}"#, inner);
        let qr = format!(
            "{QR_PAYLOAD_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(json.as_bytes())
        );

        let info = decode_ticket_info(&qr).expect("decode legacy");
        assert_eq!(info.endpoint_addr.id, id);
        assert!(info.device_name.is_empty());
        assert!(info.device_type.is_empty());
    }
}

#[cfg(test)]
mod encode_ticket_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[11u8; 32]).public()
    }

    #[test]
    fn encode_round_trips_back_to_endpoint_addr() {
        // The whole point of `encode_ticket`: take an EndpointAddr we already
        // hold (e.g. from a rendezvous claim) and serialize it back so Dart
        // can persist it as the saved-devices `lastTicket`.  Going through
        // `decode_ticket` and back must preserve both the EndpointId and the
        // addr set without loss.
        let id = sample_id();
        let original =
            synthesize_ticket(id, Some("https://relay.example/"), Some("192.168.1.5:5000"))
                .expect("synth");
        let decoded = decode_ticket(&original).expect("decode");

        let encoded = encode_ticket(decoded.clone()).expect("encode");
        let decoded_again = decode_ticket(&encoded).expect("decode again");

        assert_eq!(decoded_again.id, id);
        assert_eq!(decoded_again.addrs.len(), decoded.addrs.len());
        let has_relay = decoded_again.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        let has_ip = decoded_again
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Ip(s) if s.to_string() == "192.168.1.5:5000"));
        assert!(
            has_relay,
            "relay hint must survive encode_ticket round-trip"
        );
        assert!(has_ip, "ip hint must survive encode_ticket round-trip");
    }

    #[test]
    fn encode_endpoint_id_only_when_no_addrs() {
        // Sender's resolved EndpointAddr from pkarr can legitimately carry
        // zero addrs (relay-less, direct unknown).  encode_ticket must still
        // produce a valid ticket so the lastTicket persistence path doesn't
        // silently drop those peers.
        let id = sample_id();
        let original = synthesize_ticket(id, None, None).expect("synth");
        let decoded = decode_ticket(&original).expect("decode");
        assert!(decoded.addrs.is_empty());

        let encoded = encode_ticket(decoded).expect("encode");
        let decoded_again = decode_ticket(&encoded).expect("decode again");
        assert_eq!(decoded_again.id, id);
        assert!(decoded_again.addrs.is_empty());
    }
}
