use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use iroh::endpoint::{ConnectOptions, MtuDiscoveryConfig, QuicTransportConfig};
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::{
    ALPN as BLOBS_ALPN, api::remote::GetProgressItem, store::fs::FsStore, ticket::BlobTicket,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, trace};

use super::error::{BlobError, BlobTextError, Result};
use super::telemetry::{BlobTransferTelemetry, TransferEnd, is_enabled as telemetry_enabled};
use crate::lan::in_usb_tunnel_subnet;

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

// Defaults from noq 0.16's `TransportConfig`. The AOA per-dial override starts
// from `QuicTransportConfig::builder()`, so these are its actual flow-control
// values unless that override is explicitly changed.
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
            stream_receive_window_bytes,
            connection_receive_window_bytes,
            send_window_bytes,
            congestion_controller,
        }
    }

    fn noq_default() -> Self {
        Self::new(
            NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES,
            u64::from(iroh::endpoint::VarInt::MAX),
            NOQ_DEFAULT_SEND_WINDOW_BYTES,
            "cubic",
        )
    }
}

impl Default for BlobTransportProfile {
    fn default() -> Self {
        Self {
            known: false,
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
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
struct ProgressCoalescer {
    interval: Duration,
    last_emit_at: Option<Instant>,
    latest_bytes: Option<u64>,
    last_emitted_bytes: Option<u64>,
}

impl ProgressCoalescer {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit_at: None,
            latest_bytes: None,
            last_emitted_bytes: None,
        }
    }

    fn observe(&mut self, now: Instant, bytes_received: u64) -> Option<u64> {
        self.latest_bytes = Some(bytes_received);
        let should_emit = self
            .last_emit_at
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        should_emit.then(|| self.mark_emitted(now, bytes_received))
    }

    /// Return the newest value when it has not been emitted yet. This is used
    /// immediately before `Done`/`Failed`, so throttling never hides the final
    /// byte position from resume state or the UI.
    fn flush_pending(&mut self, now: Instant) -> Option<u64> {
        let latest = self.latest_bytes?;
        (self.last_emitted_bytes != Some(latest)).then(|| self.mark_emitted(now, latest))
    }

    fn mark_emitted(&mut self, now: Instant, bytes_received: u64) -> u64 {
        self.last_emit_at = Some(now);
        self.last_emitted_bytes = Some(bytes_received);
        bytes_received
    }
}

fn handle_download_item(
    item: Option<GetProgressItem>,
    progress: &mut ProgressCoalescer,
    update_tx: &mpsc::UnboundedSender<BlobDownloadUpdate>,
    ticket_context: &str,
    telemetry: Option<&BlobTransferTelemetry>,
) -> ControlFlow<DownloadTerminal> {
    let now = Instant::now();
    match item {
        Some(GetProgressItem::Progress(offset)) => {
            if let Some(telemetry) = telemetry {
                telemetry.observe_progress(offset);
            }
            if let Some(bytes_received) = progress.observe(now, offset) {
                let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
            }
            ControlFlow::Continue(())
        }
        Some(GetProgressItem::Done(_)) => {
            if let Some(bytes_received) = progress.flush_pending(now) {
                let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
            }
            let _ = update_tx.send(BlobDownloadUpdate::Done);
            ControlFlow::Break(DownloadTerminal {
                outcome: TransferEnd::Complete,
                result: Ok(()),
            })
        }
        Some(GetProgressItem::Error(err)) => {
            if let Some(bytes_received) = progress.flush_pending(now) {
                let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
            }
            let message = format!("blob fetch error: {err}");
            let _ = update_tx.send(BlobDownloadUpdate::Failed {
                error: BlobError::fetch(
                    ticket_context.to_owned(),
                    BlobTextError::new(message.clone()),
                ),
            });
            ControlFlow::Break(DownloadTerminal {
                outcome: TransferEnd::Failed,
                result: Err(BlobError::fetch(
                    ticket_context.to_owned(),
                    BlobTextError::new(message),
                )),
            })
        }
        None => {
            if let Some(bytes_received) = progress.flush_pending(now) {
                let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
            }
            let message = "blob fetch stream ended before completion".to_owned();
            let _ = update_tx.send(BlobDownloadUpdate::Failed {
                error: BlobError::fetch(
                    ticket_context.to_owned(),
                    BlobTextError::new(message.clone()),
                ),
            });
            ControlFlow::Break(DownloadTerminal {
                outcome: TransferEnd::Failed,
                result: Err(BlobError::fetch(
                    ticket_context.to_owned(),
                    BlobTextError::new(message),
                )),
            })
        }
    }
}

#[derive(Debug)]
struct DownloadTerminal {
    outcome: TransferEnd,
    result: Result<()>,
}

/// Chooses a per-path QUIC transport config for the receiver's blob dial.
///
/// The receiver is the puller, so the `stream_receive_window` it advertises is
/// what governs throughput — making this dial the right place to tune per path.
///
/// Returns `None` for relay / Wi-Fi / LAN, so the dial inherits the endpoint's
/// global config (Tier 1: tuned `stream_receive_window` + CUBIC + keepalive) — that
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
    store: Arc<FsStore>,
    root_dir: PathBuf,
    is_temp: bool,
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
        let BlobDownloadSession {
            events: _,
            store,
            root_dir,
            is_temp,
            task,
        } = self;
        let task_result = match task.await {
            Ok(v) => v,
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(BlobError::join_download_task(error)),
        };
        let store = Arc::try_unwrap(store).map_err(|_| BlobError::store_still_shared())?;
        store
            .shutdown()
            .await
            .map_err(|source| BlobError::store_shutdown("blob download session", source))?;
        if is_temp {
            let _ = tokio::fs::remove_dir_all(&root_dir).await;
        }
        task_result?;
        Ok(())
    }
}

pub trait BlobDownloadStrategy: Send + Sync + 'static {
    fn spawn(
        &self,
        endpoint: Endpoint,
        store: Arc<FsStore>,
        ticket: BlobTicket,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
        update_tx: mpsc::UnboundedSender<BlobDownloadUpdate>,
    ) -> JoinHandle<Result<()>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SequentialBlobDownload;

impl BlobDownloadStrategy for SequentialBlobDownload {
    fn spawn(
        &self,
        endpoint: Endpoint,
        store: Arc<FsStore>,
        ticket: BlobTicket,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
        update_tx: mpsc::UnboundedSender<BlobDownloadUpdate>,
    ) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let ticket_context = format!("ticket {ticket:?}");
            let addr = ticket.addr().clone();
            // Per-path dial: relay/Wi-Fi/LAN inherit the endpoint's global
            // transport config (Tier 1 window + CUBIC); the AOA USB tunnel gets a
            // raised MTU-discovery ceiling instead. See `blob_connect_options`.
            let (connection, transport_profile) = match blob_connect_options(&addr) {
                Some((opts, aoa_profile)) => {
                    debug!(
                        ?addr,
                        "blob dial: AOA USB tunnel path (raised MTU discovery)"
                    );
                    let connecting = endpoint
                        .connect_with_opts(addr, BLOBS_ALPN, opts)
                        .await
                        .map_err(|source| BlobError::connect(ticket_context.clone(), source))?;
                    (
                        connecting
                            .await
                            .map_err(|source| BlobError::connect(ticket_context.clone(), source))?,
                        aoa_profile,
                    )
                }
                None => (
                    endpoint
                        .connect(addr, BLOBS_ALPN)
                        .await
                        .map_err(|source| BlobError::connect(ticket_context.clone(), source))?,
                    transport_profile,
                ),
            };

            // Telemetry is fully absent from the hot path when its tracing
            // target is disabled. When enabled, a separate sampler owns the
            // timer and path-stat reads; this loop still awaits `next()` to
            // completion and only publishes a monotonic atomic byte counter.
            let mut telemetry = telemetry_enabled().then(|| {
                BlobTransferTelemetry::start(
                    Instant::now(),
                    connection.to_info(),
                    transport_profile,
                    benchmark_run_id,
                )
            });
            let mut stream = store.remote().fetch(connection, ticket).stream();
            let mut progress = ProgressCoalescer::new(PROGRESS_EMIT_INTERVAL);
            loop {
                if let ControlFlow::Break(terminal) = handle_download_item(
                    stream.next().await,
                    &mut progress,
                    &update_tx,
                    &ticket_context,
                    telemetry.as_ref(),
                ) {
                    if let Some(telemetry) = telemetry.as_mut() {
                        telemetry.finish(terminal.outcome).await;
                    }
                    break terminal.result;
                }
            }
        })
    }
}

#[derive(Debug)]
pub struct BlobReceiver<S = SequentialBlobDownload> {
    endpoint: Endpoint,
    strategy: S,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
}

impl BlobReceiver<SequentialBlobDownload> {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            strategy: SequentialBlobDownload,
            transport_profile: BlobTransportProfile::default(),
            benchmark_run_id: None,
        }
    }
}

impl<S> BlobReceiver<S>
where
    S: BlobDownloadStrategy,
{
    pub fn with_strategy(endpoint: Endpoint, strategy: S) -> Self {
        Self {
            endpoint,
            strategy,
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

    pub async fn start(
        &self,
        root_dir: PathBuf,
        ticket: BlobTicket,
        is_temp: bool,
    ) -> Result<BlobDownloadSession> {
        if is_temp {
            tokio::fs::create_dir_all(&root_dir)
                .await
                .map_err(|source| {
                    BlobError::scratch_dir_create(
                        root_dir.clone(),
                        BlobTextError::new(source.to_string()),
                    )
                })?;
        }
        let store = Arc::new(
            FsStore::load(&root_dir)
                .await
                .map_err(|source| BlobError::store_load(root_dir.clone(), source))?,
        );
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let task = self.strategy.spawn(
            self.endpoint.clone(),
            store.clone(),
            ticket,
            self.transport_profile,
            self.benchmark_run_id,
            update_tx,
        );

        trace!(root_dir = %root_dir.display(), "started blob download session");

        Ok(BlobDownloadSession {
            events: UnboundedReceiverStream::new(update_rx),
            store,
            root_dir,
            is_temp,
            task,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::ops::ControlFlow;
    use std::time::{Duration, Instant};

    use iroh::{EndpointAddr, SecretKey, TransportAddr};
    use tokio::sync::mpsc;

    use super::{
        BlobDownloadUpdate, ProgressCoalescer, TransferEnd, blob_connect_options,
        handle_download_item,
    };

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
            assert!(profile.known);
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

    #[test]
    fn premature_progress_stream_eof_is_a_failed_download() {
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        let mut progress = ProgressCoalescer::new(Duration::from_millis(100));

        let terminal =
            match handle_download_item(None, &mut progress, &update_tx, "test ticket", None) {
                ControlFlow::Break(terminal) => terminal,
                ControlFlow::Continue(()) => panic!("EOF must terminate the download"),
            };

        assert_eq!(terminal.outcome, TransferEnd::Failed);
        assert!(terminal.result.is_err());
        assert!(matches!(
            update_rx.try_recv(),
            Ok(BlobDownloadUpdate::Failed { .. })
        ));
        assert!(update_rx.try_recv().is_err());
    }
}
