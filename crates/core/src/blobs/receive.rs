use std::net::SocketAddr;
use std::time::{Duration, Instant};

use iroh::endpoint::{ConnectOptions, MtuDiscoveryConfig, QuicTransportConfig};
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::{ALPN as BLOBS_ALPN, ticket::BlobTicket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, trace};

use super::error::{BlobError, BlobTextError, Result};
use super::source::{BlobSource, LanTarget};
use super::stream::{StreamTarget, stream_collection};
use super::telemetry::{BlobTransferTelemetry, TransferEnd, is_enabled as telemetry_enabled};
use crate::lan::in_usb_tunnel_subnet;
use crate::lan_transport::local_ipv4_nets;

/// QUIC MTU-discovery ceiling (max UDP payload, bytes) for the Android↔Android
/// AOA USB-cable tunnel (`10.42.0.0/30`).
///
/// The cable is a private point-to-point link, so a large datagram is safe
/// framing-wise. DPLPMTUD only *probes* toward this bound and falls back on
/// loss, so raising it is safe to land ahead of Tier 3: until the AOA TUN MTU is
/// raised (`UsbAoaChannel.kt::TUNNEL_MTU`), discovery simply settles at ~1252 as
/// before. Keep this at or below that TUN MTU minus IPv4+UDP overhead (28 bytes)
/// so the raised TUN MTU is actually usable end-to-end.
const AOA_MTU_DISCOVERY_UPPER_BOUND: u16 = 7_900;

// Defaults from the resolved noq-proto 1.1 `TransportConfig` used by noq 1.1.
// The AOA per-dial override starts from `QuicTransportConfig::builder()`, so
// these are its expected flow-control values unless that override changes.
// Unchanged across the iroh 0.97 → 1.0 upgrade: both versions derive them from
// the same `MAX_STREAM_BANDWIDTH`/`EXPECTED_RTT` pair.
const NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES: u64 = 1_250_000;
const NOQ_DEFAULT_SEND_WINDOW_BYTES: u64 = 10_000_000;

/// Flow-control and congestion settings applied to blob QUIC connections.
///
/// iroh does not currently expose getters for `QuicTransportConfig`, so the
/// application passes the values alongside the config at construction time.
/// Unknown profiles remain explicit rather than emitting guessed numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobTransportProfile {
    pub(super) known: bool,
    pub(super) config_source: &'static str,
    pub(super) stream_receive_window_bytes: u64,
    pub(super) connection_receive_window_bytes: u64,
    pub(super) send_window_bytes: u64,
    pub(super) congestion_controller: &'static str,
}

impl BlobTransportProfile {
    pub const fn new(
        stream_receive_window_bytes: u64,
        connection_receive_window_bytes: u64,
        send_window_bytes: u64,
        congestion_controller: &'static str,
    ) -> Self {
        Self {
            known: true,
            config_source: "configured",
            stream_receive_window_bytes,
            connection_receive_window_bytes,
            send_window_bytes,
            congestion_controller,
        }
    }

    fn noq_default() -> Self {
        Self {
            // These values match the pinned noq version but are not readable
            // from the built transport config. Keep them visible for diagnosis
            // without presenting a copied upstream default as measured truth.
            known: false,
            config_source: "assumed_upstream_default",
            stream_receive_window_bytes: NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES,
            connection_receive_window_bytes: u64::from(iroh::endpoint::VarInt::MAX),
            send_window_bytes: NOQ_DEFAULT_SEND_WINDOW_BYTES,
            congestion_controller: "cubic",
        }
    }
}

impl Default for BlobTransportProfile {
    fn default() -> Self {
        Self {
            known: false,
            config_source: "unknown",
            stream_receive_window_bytes: 0,
            connection_receive_window_bytes: 0,
            send_window_bytes: 0,
            congestion_controller: "unknown",
        }
    }
}

/// Maximum rate at which blob progress crosses into the transfer/application
/// layers. `iroh-blobs` reports progress at BAO-content granularity (16 KiB for
/// regular leaves), which can otherwise create thousands of JSON checkpoints,
/// QUIC progress frames, and UI events per second on a fast link.
pub(crate) const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) struct ProgressCoalescer {
    interval: Duration,
    last_emit_at: Option<Instant>,
    latest_bytes: Option<u64>,
    last_emitted_bytes: Option<u64>,
}

impl ProgressCoalescer {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit_at: None,
            latest_bytes: None,
            last_emitted_bytes: None,
        }
    }

    pub(crate) fn observe(&mut self, now: Instant, bytes_received: u64) -> Option<u64> {
        self.latest_bytes = Some(bytes_received);
        let should_emit = self
            .last_emit_at
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        should_emit.then(|| self.mark_emitted(now, bytes_received))
    }

    /// Return the newest value when it has not been emitted yet. This is used
    /// immediately before `Done`/`Failed`, so throttling never hides the final
    /// byte position from resume state or the UI.
    pub(crate) fn flush_pending(&mut self, now: Instant) -> Option<u64> {
        let latest = self.latest_bytes?;
        (self.last_emitted_bytes != Some(latest)).then(|| self.mark_emitted(now, latest))
    }

    fn mark_emitted(&mut self, now: Instant, bytes_received: u64) -> u64 {
        self.last_emit_at = Some(now);
        self.last_emitted_bytes = Some(bytes_received);
        bytes_received
    }
}

const BENCH_SINGLE_PATH_ENV: &str = "WISP_BENCH_SINGLE_PATH";

/// Narrows a blob dial to one direct address when [`BENCH_SINGLE_PATH_ENV`] is
/// set, returning `None` when the switch is off or no direct address is on offer.
///
/// Telemetry showed 3-8 paths carrying payload at the same time, with the
/// selected path accounting for as little as 11% of the bytes and a relay path
/// taking 25.7% of a transfer the receiver reported as direct throughout. That
/// raises a question worth answering: does the concurrency help, or do paths
/// with differing RTT reorder enough that BAO verification stalls behind the
/// slowest one?
///
/// **This switch does not answer it, and neither does anything else in iroh
/// 0.97.** Both `max_concurrent_multipath_paths` and
/// `set_max_remote_nat_traversal_addresses` refuse values below their
/// recommended floors â€” they warn and keep the default â€” so the path count can
/// be raised but never lowered. Narrowing the dial does not substitute for it
/// either: iroh tracks addresses per *remote*, not per dial, and
/// `remote_state.rs` deliberately reopens relay paths once a direct path comes
/// up ("we may have raced this with a relay address") and then triggers hole
/// punching for more. Measured on loopback, this switch took the path count from
/// 1/3/4 down to 1/3 â€” a narrower start, not control.
///
/// The lever that does work is binding without a relay at all â€” see
/// `WISP_BENCH_NO_RELAY` in `wisp_app::bench`, which removes relay addresses at
/// the source rather than asking iroh not to reopen them. On loopback that
/// found no reliable throughput difference; this switch remains useful for
/// narrowing the direct address set alongside it.
///
/// Prefers the USB cable subnet when present so the AOA path keeps its own dial,
/// then falls back to the first IPv4 address in the ticket.
fn bench_single_path_addr(addr: &EndpointAddr) -> Option<EndpointAddr> {
    if std::env::var_os(BENCH_SINGLE_PATH_ENV).is_none() {
        return None;
    }
    let mut ipv4 = addr.ip_addrs().filter(|sa| matches!(sa, SocketAddr::V4(_)));
    let cable = addr.ip_addrs().find(|sa| match sa {
        SocketAddr::V4(v4) => in_usb_tunnel_subnet(*v4.ip()),
        SocketAddr::V6(_) => false,
    });
    let chosen = cable.copied().or_else(|| ipv4.next().copied())?;
    Some(EndpointAddr::from_parts(
        addr.id,
        [iroh::TransportAddr::Ip(chosen)],
    ))
}

/// Chooses a per-path QUIC transport config for the receiver's blob dial.
///
/// The receiver is the puller, so the `stream_receive_window` it advertises is
/// what governs throughput â€” making this dial the right place to tune per path.
///
/// Returns `None` for relay / Wi-Fi / LAN, so the dial inherits the endpoint's
/// global config (Tier 1: tuned `stream_receive_window` + CUBIC + keepalive) â€” that
/// is exactly the large window that lifts the relay ceiling.
///
/// Returns a tunnel-specific override only for the AOA USB cable: there the win
/// is a *larger MTU*, not a larger window (sub-ms RTT means the default window is
/// already far from limiting), so we raise the MTU-discovery ceiling and keep
/// everything else lean. Keepalive mirrors the global config so the cable path
/// behaves identically otherwise.
fn blob_connect_options(addr: &EndpointAddr) -> Option<(ConnectOptions, BlobTransportProfile)> {
    let is_aoa_tunnel = addr.ip_addrs().any(|sa| match sa {
        SocketAddr::V4(v4) => in_usb_tunnel_subnet(*v4.ip()),
        SocketAddr::V6(_) => false,
    });
    if !is_aoa_tunnel {
        return None;
    }

    let mut mtu = MtuDiscoveryConfig::default();
    mtu.upper_bound(AOA_MTU_DISCOVERY_UPPER_BOUND);

    let transport = QuicTransportConfig::builder()
        // Mirror the global keepalive (must stay under iroh's 6.5s / 5s clamp).
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        // Let DPLPMTUD climb toward the (Tier 3) raised TUN MTU.
        .mtu_discovery_config(Some(mtu))
        .build();

    Some((
        ConnectOptions::new().with_transport_config(transport),
        BlobTransportProfile::noq_default(),
    ))
}

/// Picks the transport this transfer's payload will use.
///
/// The LAN TCP path is taken only when the sender published a port *and* one of
/// the addresses in its ticket is on a subnet of ours, and only if the first
/// dial succeeds. Choosing once, up front, is deliberate: a per-request
/// decision could leave one transfer split across two transports, and a
/// per-request fallback would pay the cap on every file of a transfer that was
/// never going to work.
///
/// When TCP is used the QUIC blob dial is **skipped entirely**, which also
/// avoids the path-finding that measured 2.5 s between manifest and offer on a
/// receiver advertising nine addresses. Falling back costs the cap (1 s) and
/// then behaves exactly as before.
///
/// Returns the transport profile alongside, because the QUIC arm resolves one
/// during its dial and the caller reports it.
async fn choose_source(
    endpoint: &Endpoint,
    ticket: &BlobTicket,
    sender_tcp_port: Option<u16>,
    transport_profile: BlobTransportProfile,
    context: &str,
) -> Result<(BlobSource, BlobTransportProfile)> {
    let peer = ticket.addr().id;
    if let Some(target) =
        crate::lan_transport::tcp_target(ticket.addr(), sender_tcp_port, &local_ipv4_nets())
    {
        let lan = Box::new(LanTarget {
            target,
            secret: endpoint.secret_key().clone(),
            peer,
        });
        // Prove the path before committing the transfer to it. The probe is a
        // real connection and handshake, thrown away: the alternative is
        // discovering on file 1 of 76 that the port is firewalled.
        match crate::lan_transport::dial(target, &lan.secret, peer).await {
            Ok(mut probe) => {
                // Close it properly rather than dropping it: a TLS stream that
                // ends without close_notify makes the provider log a truncation
                // warning for what was a successful probe, and a warning that
                // fires on every healthy transfer is a warning nobody reads.
                use tokio::io::AsyncWriteExt;
                let _ = probe.shutdown().await;
                debug!(%target, "blob source: lan tcp");
                return Ok((BlobSource::LanTcp(lan), transport_profile));
            }
            Err(error) => {
                debug!(%target, %error, "blob source: lan tcp unreachable, falling back to quic");
            }
        }
    }
    let (connection, transport_profile) =
        dial_blob_provider(endpoint, ticket.addr(), transport_profile, context).await?;
    Ok((BlobSource::Quic(connection), transport_profile))
}

/// Dials the blob provider named by `addr`.
///
/// Per-path dial: relay/Wi-Fi/LAN inherit the endpoint's global transport
/// config (Tier 1 window + CUBIC); the AOA USB tunnel gets a raised
/// MTU-discovery ceiling instead. See [`blob_connect_options`].
pub(crate) async fn dial_blob_provider(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    transport_profile: BlobTransportProfile,
    context: &str,
) -> Result<(iroh::endpoint::Connection, BlobTransportProfile)> {
    let mut addr = addr.clone();
    if let Some(single) = bench_single_path_addr(&addr) {
        debug!(
            from = ?addr,
            to = ?single,
            "blob dial: {BENCH_SINGLE_PATH_ENV} set, offering one direct address"
        );
        addr = single;
    }
    match blob_connect_options(&addr) {
        Some((opts, aoa_profile)) => {
            debug!(
                ?addr,
                "blob dial: AOA USB tunnel path (raised MTU discovery)"
            );
            let connecting = endpoint
                .connect_with_opts(addr, BLOBS_ALPN, opts)
                .await
                .map_err(|source| BlobError::connect(context.to_owned(), source))?;
            Ok((
                connecting
                    .await
                    .map_err(|source| BlobError::connect(context.to_owned(), source))?,
                aoa_profile,
            ))
        }
        None => Ok((
            endpoint
                .connect(addr, BLOBS_ALPN)
                .await
                .map_err(|source| BlobError::connect(context.to_owned(), source))?,
            transport_profile,
        )),
    }
}

#[derive(Debug)]
pub enum BlobDownloadUpdate {
    Progress { bytes_received: u64 },
    Done,
    Failed { error: BlobError },
}

pub type BlobDownloadUpdateStream = UnboundedReceiverStream<BlobDownloadUpdate>;

#[derive(Debug)]
pub struct BlobDownloadSession {
    events: BlobDownloadUpdateStream,
    task: JoinHandle<Result<()>>,
}

impl BlobDownloadSession {
    pub(crate) fn events_mut(&mut self) -> &mut BlobDownloadUpdateStream {
        &mut self.events
    }

    pub(crate) fn abort(&self) {
        self.task.abort();
    }

    pub async fn shutdown(self) -> Result<()> {
        let BlobDownloadSession { events: _, task } = self;
        match task.await {
            Ok(result) => result,
            // Cancelled is how `abort` reports, and the caller already knows.
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(BlobError::join_download_task(error)),
        }
    }
}

#[derive(Debug)]
pub struct BlobReceiver {
    endpoint: Endpoint,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
}

impl BlobReceiver {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            transport_profile: BlobTransportProfile::default(),
            benchmark_run_id: None,
        }
    }

    pub fn with_transport_profile(mut self, transport_profile: BlobTransportProfile) -> Self {
        self.transport_profile = transport_profile;
        self
    }

    pub fn with_benchmark_run_id(mut self, benchmark_run_id: Option<u64>) -> Self {
        self.benchmark_run_id = benchmark_run_id;
        self
    }

    /// Starts a download that writes each file straight to its destination.
    ///
    /// Unlike [`Self::start`] there is no intermediate store, so the receiver
    /// only ever needs room for the files themselves. Everything else — the
    /// per-path dial, the update stream, cancellation — behaves the same, so
    /// the caller drives this session exactly like a store-backed one.
    /// `sender_tcp_port` is what the sender published alongside its ticket. When
    /// it is set and the sender turns out to be on one of our subnets, the
    /// payload goes over the LAN TCP transport instead of QUIC.
    pub(crate) async fn start_streaming(
        &self,
        ticket: BlobTicket,
        sender_tcp_port: Option<u16>,
        targets: Vec<StreamTarget>,
    ) -> Result<BlobDownloadSession> {
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let endpoint = self.endpoint.clone();
        let transport_profile = self.transport_profile;
        let benchmark_run_id = self.benchmark_run_id;
        let task = tokio::spawn(async move {
            let ticket_context = format!("ticket {ticket:?}");
            let (source, transport_profile) = choose_source(
                &endpoint,
                &ticket,
                sender_tcp_port,
                transport_profile,
                &ticket_context,
            )
            .await?;
            // The telemetry reads QUIC's own congestion and path counters, so
            // it has nothing to read on the LAN transport. Absent rather than
            // zeroed: a run with no numbers is easier to notice than a run with
            // numbers that mean nothing.
            let mut telemetry = match &source {
                BlobSource::Quic(connection) => telemetry_enabled().then(|| {
                    BlobTransferTelemetry::start(
                        Instant::now(),
                        connection.clone(),
                        transport_profile,
                        benchmark_run_id,
                    )
                }),
                BlobSource::LanTcp(_) => None,
            };
            let result = stream_collection(
                source,
                ticket.hash(),
                targets,
                update_tx.clone(),
                telemetry.as_ref(),
            )
            .await;
            if let Some(telemetry) = telemetry.as_mut() {
                telemetry
                    .finish(if result.is_ok() {
                        TransferEnd::Complete
                    } else {
                        TransferEnd::Failed
                    })
                    .await;
            }
            if let Err(error) = &result {
                let _ = update_tx.send(BlobDownloadUpdate::Failed {
                    error: BlobError::fetch(ticket_context, BlobTextError::new(error.to_string())),
                });
            }
            result
        });

        trace!("started streaming blob download session");

        Ok(BlobDownloadSession {
            events: UnboundedReceiverStream::new(update_rx),
            task,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};

    use iroh::{EndpointAddr, SecretKey, TransportAddr};

    use super::{ProgressCoalescer, blob_connect_options};

    fn addr_with(ip: &str) -> EndpointAddr {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let sa: SocketAddr = ip.parse().unwrap();
        EndpointAddr::new(id).with_addrs(vec![TransportAddr::Ip(sa)])
    }

    #[test]
    fn aoa_tunnel_addr_gets_per_path_override() {
        // Any address inside the AOA point-to-point /30 (10.42.0.0/30) selects
        // the tunnel-specific transport config (raised MTU-discovery ceiling).
        for addr in ["10.42.0.1:11204", "10.42.0.2:11204"] {
            let (_, profile) = blob_connect_options(&addr_with(addr)).unwrap();
            assert!(!profile.known);
            assert_eq!(profile.config_source, "assumed_upstream_default");
            assert_eq!(
                profile.stream_receive_window_bytes,
                super::NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES
            );
            assert_eq!(
                profile.send_window_bytes,
                super::NOQ_DEFAULT_SEND_WINDOW_BYTES
            );
        }
    }

    #[test]
    fn non_tunnel_addr_inherits_global_config() {
        // LAN / Wi-Fi and look-alike subnets fall through to None so the dial
        // inherits the endpoint's global (Tier 1) transport config.
        assert!(blob_connect_options(&addr_with("192.168.1.50:11204")).is_none());
        // Same first two octets but a different third octet is NOT the tunnel.
        assert!(blob_connect_options(&addr_with("10.42.1.2:11204")).is_none());
        assert!(blob_connect_options(&addr_with("100.64.0.1:11204")).is_none());
    }

    #[test]
    fn relay_only_addr_inherits_global_config() {
        // Relay-only ticket (no direct IPs) → None.
        let id = SecretKey::from_bytes(&[9u8; 32]).public();
        assert!(blob_connect_options(&EndpointAddr::new(id)).is_none());
    }

    #[test]
    fn progress_coalescer_limits_rate_and_flushes_latest_value() {
        let start = Instant::now();
        let mut progress = ProgressCoalescer::new(Duration::from_millis(100));

        assert_eq!(progress.observe(start, 16 * 1024), Some(16 * 1024));
        assert_eq!(
            progress.observe(start + Duration::from_millis(25), 32 * 1024),
            None
        );
        assert_eq!(
            progress.observe(start + Duration::from_millis(100), 48 * 1024),
            Some(48 * 1024)
        );
        assert_eq!(
            progress.observe(start + Duration::from_millis(125), 64 * 1024),
            None
        );

        assert_eq!(
            progress.flush_pending(start + Duration::from_millis(126)),
            Some(64 * 1024)
        );
        assert_eq!(
            progress.flush_pending(start + Duration::from_millis(127)),
            None
        );
    }
}
