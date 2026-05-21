//! Process-wide dispatcher that lets the receiver service's accept loop hand
//! `iroh_blobs::ALPN` connections to whichever sender is currently active.
//!
//! Why this exists: the app keeps **one** iroh `Endpoint` per process so the
//! receiver service and any in-flight sender share the same `EndpointId` and
//! relay slot.  That single endpoint runs a single `iroh::protocol::Router`
//! that multiplexes the two ALPNs we speak — `wisp_core::protocol::ALPN`
//! (handshake / control) and `iroh_blobs::ALPN` (data).  When a send session
//! starts it prepares a `BlobsProtocol` and registers it here; the receiver's
//! router calls into this dispatcher whenever the peer dials back over the
//! blobs ALPN.  When the send session ends, it clears the slot.

use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::BlobsProtocol;
use std::future::Future;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use wisp_core::blobs::{BlobError, ExternalBlobRegistrar};

/// Singleton that wires the receiver-service `Router` to the current sender's
/// `BlobsProtocol`.  At most one sender is active per process today, matching
/// the UI (one Send screen at a time).
#[derive(Debug, Clone)]
pub struct BlobDispatcher {
    active: Arc<RwLock<Option<BlobsProtocol>>>,
}

impl BlobDispatcher {
    /// Returns the process-wide instance.  Cheap clone — the inner state is
    /// behind an `Arc<RwLock<…>>` so all clones see the same slot.
    pub fn global() -> Self {
        static INSTANCE: OnceLock<BlobDispatcher> = OnceLock::new();
        INSTANCE
            .get_or_init(|| BlobDispatcher {
                active: Arc::new(RwLock::new(None)),
            })
            .clone()
    }
}

/// Sender-side wiring: install the prepared `BlobsProtocol` before writing
/// the `BlobTicket` to the peer; remove it once the transfer completes (or
/// fails).  See [`wisp_core::blobs::ExternalBlobRegistrar`] for the contract.
impl ExternalBlobRegistrar for BlobDispatcher {
    fn register_blob_protocol(
        &self,
        protocol: BlobsProtocol,
    ) -> Pin<Box<dyn Future<Output = Result<(), BlobError>> + Send + '_>> {
        Box::pin(async move {
            let mut guard = self.active.write().await;
            if guard.is_some() {
                warn!(
                    target: "wisp_app::blob_dispatcher",
                    "replacing an already-active blob protocol; previous sender's transfer \
                     is being cut off — should not happen with the current one-send-at-a-time UI"
                );
            }
            *guard = Some(protocol);
            debug!(
                target: "wisp_app::blob_dispatcher",
                "registered active blob protocol"
            );
            Ok(())
        })
    }

    fn unregister_blob_protocol(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut guard = self.active.write().await;
            if guard.take().is_some() {
                debug!(
                    target: "wisp_app::blob_dispatcher",
                    "cleared active blob protocol"
                );
            }
        })
    }
}

/// Receiver-side wiring: the receiver service's `iroh::protocol::Router`
/// registers `BlobDispatcher` against `iroh_blobs::ALPN`, so any inbound
/// blobs connection lands here.  We forward it to whichever sender is
/// currently registered.  If no sender is active (e.g. an unsolicited dial),
/// we close the connection gracefully so the peer learns immediately rather
/// than hanging.
impl ProtocolHandler for BlobDispatcher {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let active = { self.active.read().await.clone() };
        match active {
            Some(protocol) => {
                debug!(
                    target: "wisp_app::blob_dispatcher",
                    remote = %connection.remote_id(),
                    "dispatching inbound blobs ALPN connection to active sender"
                );
                protocol.accept(connection).await
            }
            None => {
                debug!(
                    target: "wisp_app::blob_dispatcher",
                    remote = %connection.remote_id(),
                    "no active sender; rejecting inbound blobs connection"
                );
                connection.close(0u32.into(), b"no active blob serving");
                Ok(())
            }
        }
    }
}
