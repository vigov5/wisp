//! Serving one transfer's blobs over the LAN TCP transport.
//!
//! The mirror of [`crate::blobs::source::BlobSource::LanTcp`]. Where the getter
//! dials per request, this accepts per request: TCP carries one stream per
//! connection, so a transfer of *n* files arrives as *n* connections. Each is
//! handed to `iroh_blobs::provider::handle_stream`, which is the same code the
//! QUIC path reaches — only the socket underneath differs.
//!
//! # What this port exposes
//!
//! The same thing the QUIC blob endpoint exposes: this transfer's store, to
//! anyone who can name a hash in it. The hashes travel in the ticket, which
//! goes over the authenticated control connection, so knowing one already
//! implies having been told. That is parity with the existing path rather than
//! a new exposure, and the TLS from [`crate::lan_tls`] means the bytes are
//! still encrypted and the peer still proves an identity.
//!
//! Pinning the accept side to the receiver's identity would be strictly
//! stronger, and the identity is right there in [`lan_transport::accept`]'s
//! return. It is not done here only because the sender does not currently
//! thread the peer's key down to this layer; if this port ever serves anything
//! broader than one transfer's files, that stops being an acceptable gap.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iroh::SecretKey;
use iroh_blobs::api::Store;
use iroh_blobs::provider::events::EventSender;
use iroh_blobs::provider::{StreamPair, handle_stream};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::error::{BlobError, Result};
use crate::lan_transport;

/// A bound LAN TCP port serving one transfer, for as long as it is held.
#[derive(Debug)]
pub(crate) struct LanBlobProvider {
    port: u16,
    accept: JoinHandle<()>,
}

impl LanBlobProvider {
    /// Binds an ephemeral port on every interface and serves `store` on it.
    ///
    /// Ephemeral rather than fixed: two transfers can be in flight, and a fixed
    /// port would make the second fail to bind or, worse, answer for the first.
    /// The port is published in the ticket message, so it never has to be
    /// guessed.
    pub(crate) async fn start(store: Store, secret: SecretKey) -> Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", 0)).await.map_err(|source| {
            BlobError::connect("binding the lan tcp provider".to_owned(), source)
        })?;
        let port = listener
            .local_addr()
            .map_err(|source| {
                BlobError::connect("reading the lan tcp provider port".to_owned(), source)
            })?
            .port();
        debug!(port, "lan_tcp.provider_listening");

        let accept = tokio::spawn(async move {
            let connection_ids = Arc::new(AtomicU64::new(0));
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        // A per-connection accept error (a descriptor limit, a
                        // peer that vanished mid-handshake) must not take the
                        // listener down while the transfer is still running.
                        warn!(%error, "lan_tcp.accept_failed");
                        continue;
                    }
                };
                let store = store.clone();
                let secret = secret.clone();
                let id = connection_ids.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    let (stream, identity) = match lan_transport::accept(tcp, &secret).await {
                        Ok(accepted) => accepted,
                        Err(error) => {
                            warn!(%peer, %error, "lan_tcp.handshake_failed");
                            return;
                        }
                    };
                    let (recv, send) = lan_transport::stream_halves(stream);
                    let pair = StreamPair::new(id, recv, send, EventSender::DEFAULT);
                    match handle_stream(pair, store).await {
                        Ok(()) => debug!(%peer, peer_id = %identity.fmt_short(), "lan_tcp.served"),
                        Err(error) => {
                            warn!(%peer, peer_id = %identity.fmt_short(), %error, "lan_tcp.serve_failed")
                        }
                    }
                });
            }
        });

        Ok(Self { port, accept })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for LanBlobProvider {
    fn drop(&mut self) {
        // The accept loop borrows nothing outside itself, so aborting is enough
        // to close the port. In-flight `handle_stream` tasks are detached and
        // finish or fail on their own; a transfer that has been torn down has
        // no use for their results either way.
        self.accept.abort();
    }
}
