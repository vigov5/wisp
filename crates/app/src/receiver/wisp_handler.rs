//! `iroh::protocol::ProtocolHandler` implementation for wisp's control ALPN.
//!
//! The receiver service used to drive a manual `endpoint.accept()` loop and
//! spawn a `ReceiverSession` for every inbound connection.  We replaced that
//! loop with an `iroh::protocol::Router` so the same endpoint can multiplex
//! `wisp_core::protocol::ALPN` (handled here) and `iroh_blobs::ALPN`
//! (handled by `BlobDispatcher`).  Sharing one endpoint between the receiver
//! service and any active sender eliminates the relay duplicate-id failure
//! mode where two endpoints with the same secret key fight for the relay
//! slot.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iroh::Endpoint;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::mpsc;
use tracing::debug;
use wisp_core::protocol::DeviceType;

use crate::types::ConflictPolicy;

use super::actor::ReceiverCommand;
use super::session::ReceiverSession;

#[derive(Debug, Clone)]
pub(super) struct WispProtocolHandler {
    inner: Arc<WispProtocolHandlerInner>,
}

#[derive(Debug)]
struct WispProtocolHandlerInner {
    cmd_tx: mpsc::Sender<ReceiverCommand>,
    out_dir: PathBuf,
    device_name: String,
    device_type: DeviceType,
    conflict_policy: ConflictPolicy,
    endpoint: Endpoint,
    /// Monotonic id stamped on each accepted offer so the actor can match
    /// later events (`OfferProgress`, `OfferFinished`, …) back to the right
    /// session.
    next_offer_id: AtomicU64,
}

impl WispProtocolHandler {
    pub(super) fn new(
        cmd_tx: mpsc::Sender<ReceiverCommand>,
        out_dir: PathBuf,
        device_name: String,
        device_type: DeviceType,
        conflict_policy: ConflictPolicy,
        endpoint: Endpoint,
    ) -> Self {
        Self {
            inner: Arc::new(WispProtocolHandlerInner {
                cmd_tx,
                out_dir,
                device_name,
                device_type,
                conflict_policy,
                endpoint,
                next_offer_id: AtomicU64::new(1),
            }),
        }
    }
}

impl ProtocolHandler for WispProtocolHandler {
    /// One spawned task per accepted wisp connection.  The Router already
    /// runs `accept` on a fresh tokio task, so we don't need to detach again
    /// here — we just delegate to `ReceiverSession::spawn` (which itself
    /// spawns the handshake/transfer driver) and return `Ok(())`.  Returning
    /// quickly is important: the Router awaits this future before accepting
    /// the next connection on this ALPN, but the actual transfer work runs
    /// on the session's spawned task so concurrent offers are still fine.
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let inner = Arc::clone(&self.inner);
        let offer_id = inner.next_offer_id.fetch_add(1, Ordering::Relaxed);
        debug!(
            target: "wisp_app::receiver::wisp_handler",
            offer_id,
            "accepted wisp ALPN connection"
        );
        let session = ReceiverSession::new(
            offer_id,
            inner.endpoint.clone(),
            connection,
            inner.out_dir.clone(),
            inner.device_name.clone(),
            inner.device_type,
            inner.conflict_policy,
            inner.cmd_tx.clone(),
        );
        let _ = session.spawn();
        Ok(())
    }
}
