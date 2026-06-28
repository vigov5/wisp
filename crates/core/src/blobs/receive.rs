use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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

/// Chooses a per-path QUIC transport config for the receiver's blob dial.
///
/// The receiver is the puller, so the `stream_receive_window` it advertises is
/// what governs throughput — making this dial the right place to tune per path.
///
/// Returns `None` for relay / Wi-Fi / LAN, so the dial inherits the endpoint's
/// global config (Tier 1: tuned `stream_receive_window` + BBR + keepalive) — that
/// is exactly the large window that lifts the relay ceiling.
///
/// Returns a tunnel-specific override only for the AOA USB cable: there the win
/// is a *larger MTU*, not a larger window (sub-ms RTT means the default window is
/// already far from limiting), so we raise the MTU-discovery ceiling and keep
/// everything else lean. Keepalive mirrors the global config so the cable path
/// behaves identically otherwise.
fn blob_connect_options(addr: &EndpointAddr) -> Option<ConnectOptions> {
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

    Some(ConnectOptions::new().with_transport_config(transport))
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
        update_tx: mpsc::UnboundedSender<BlobDownloadUpdate>,
    ) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let ticket_context = format!("ticket {ticket:?}");
            let addr = ticket.addr().clone();
            // Per-path dial: relay/Wi-Fi/LAN inherit the endpoint's global
            // transport config (Tier 1 window + BBR); the AOA USB tunnel gets a
            // raised MTU-discovery ceiling instead. See `blob_connect_options`.
            let connection = match blob_connect_options(&addr) {
                Some(opts) => {
                    debug!(
                        ?addr,
                        "blob dial: AOA USB tunnel path (raised MTU discovery)"
                    );
                    let connecting = endpoint
                        .connect_with_opts(addr, BLOBS_ALPN, opts)
                        .await
                        .map_err(|source| BlobError::connect(ticket_context.clone(), source))?;
                    connecting
                        .await
                        .map_err(|source| BlobError::connect(ticket_context.clone(), source))?
                }
                None => endpoint
                    .connect(addr, BLOBS_ALPN)
                    .await
                    .map_err(|source| BlobError::connect(ticket_context.clone(), source))?,
            };

            let mut stream = store.remote().fetch(connection, ticket).stream();

            loop {
                match stream.next().await {
                    Some(GetProgressItem::Progress(offset)) => {
                        let _ = update_tx.send(BlobDownloadUpdate::Progress {
                            bytes_received: offset,
                        });
                    }
                    Some(GetProgressItem::Done(_)) | None => {
                        let _ = update_tx.send(BlobDownloadUpdate::Done);
                        break Ok(());
                    }
                    Some(GetProgressItem::Error(err)) => {
                        let message = format!("blob fetch error: {err}");
                        let _ = update_tx.send(BlobDownloadUpdate::Failed {
                            error: BlobError::fetch(
                                ticket_context.clone(),
                                BlobTextError::new(message.clone()),
                            ),
                        });
                        break Err(BlobError::fetch(
                            ticket_context,
                            BlobTextError::new(message),
                        ));
                    }
                }
            }
        })
    }
}

#[derive(Debug)]
pub struct BlobReceiver<S = SequentialBlobDownload> {
    endpoint: Endpoint,
    strategy: S,
}

impl BlobReceiver<SequentialBlobDownload> {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            strategy: SequentialBlobDownload,
        }
    }
}

impl<S> BlobReceiver<S>
where
    S: BlobDownloadStrategy,
{
    pub fn with_strategy(endpoint: Endpoint, strategy: S) -> Self {
        Self { endpoint, strategy }
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
        let task = self
            .strategy
            .spawn(self.endpoint.clone(), store.clone(), ticket, update_tx);

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

    use iroh::{EndpointAddr, SecretKey, TransportAddr};

    use super::blob_connect_options;

    fn addr_with(ip: &str) -> EndpointAddr {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let sa: SocketAddr = ip.parse().unwrap();
        EndpointAddr::new(id).with_addrs(vec![TransportAddr::Ip(sa)])
    }

    #[test]
    fn aoa_tunnel_addr_gets_per_path_override() {
        // Any address inside the AOA point-to-point /30 (10.42.0.0/30) selects
        // the tunnel-specific transport config (raised MTU-discovery ceiling).
        assert!(blob_connect_options(&addr_with("10.42.0.1:11204")).is_some());
        assert!(blob_connect_options(&addr_with("10.42.0.2:11204")).is_some());
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
}
