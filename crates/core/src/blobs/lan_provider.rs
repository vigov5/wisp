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
//! Only the receiver this transfer is for. The handshake proves which identity
//! the caller holds, and anything other than the peer the sender is already
//! talking to is refused before a request is read.
//!
//! That is stricter than the QUIC blob endpoint, which serves this transfer's
//! store to anyone who can name a hash in it — safe, because the hashes travel
//! in the ticket over the authenticated control connection, but "safe because
//! the capability is hard to guess" is weaker than "the wrong peer cannot get
//! in". There was no reason to carry the weaker property into a new port when
//! the identity was already in hand.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use iroh::{PublicKey, SecretKey};
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
    /// Binds an ephemeral port on every interface and serves `store` on it to
    /// `expected_peer` and nobody else.
    ///
    /// Ephemeral rather than fixed: two transfers can be in flight, and a fixed
    /// port would make the second fail to bind or, worse, answer for the first.
    /// The port is published in the ticket message, so it never has to be
    /// guessed.
    pub(crate) async fn start(
        store: Store,
        secret: SecretKey,
        expected_peer: PublicKey,
    ) -> Result<Self> {
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
                    if identity != expected_peer {
                        // The handshake proved they hold *an* identity; it is
                        // the wrong one. Refused before a request is read, so
                        // nothing about the store is revealed, not even whether
                        // a given hash exists.
                        warn!(
                            %peer,
                            got = %identity.fmt_short(),
                            want = %expected_peer.fmt_short(),
                            "lan_tcp.wrong_peer_refused"
                        );
                        return;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_blobs::store::mem::MemStore;
    use std::net::{Ipv4Addr, SocketAddr};

    /// The property that makes this port stricter than the QUIC one: proving
    /// possession of *an* identity is not enough, it has to be the identity the
    /// sender is already talking to.
    ///
    /// Asserted through the transport rather than by calling the check
    /// directly, because the thing worth pinning is that a wrong peer gets
    /// nothing — not that a comparison exists.
    #[tokio::test]
    async fn a_peer_that_is_not_the_expected_one_gets_nothing() {
        let store = MemStore::new();
        let sender = SecretKey::generate();
        let receiver = SecretKey::generate();
        let stranger = SecretKey::generate();

        let provider =
            LanBlobProvider::start(store.as_ref().clone(), sender.clone(), receiver.public())
                .await
                .expect("the provider should bind");
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, provider.port()));

        // The expected receiver gets a working, authenticated connection.
        let welcome = crate::lan_transport::dial(target, &receiver, sender.public()).await;
        assert!(
            welcome.is_ok(),
            "the expected peer must be served: {welcome:?}"
        );

        // The stranger's TLS handshake succeeds — it holds a real key — and
        // then it is dropped without being served. Observable as the stream
        // closing with no response to a request.
        let intruder = crate::lan_transport::dial(target, &stranger, sender.public())
            .await
            .expect("a stranger can still complete a handshake");
        let (mut recv, mut send) = crate::lan_transport::stream_halves(intruder);
        use iroh_blobs::util::{RecvStream, SendStream};
        // Whatever happens to the write, nothing may come back.
        let _ = send.send(b"a request that must not be answered").await;
        let _ = send.sync().await;
        let answer = recv.recv_bytes(1).await;
        let refused = match &answer {
            Err(_) => true,
            Ok(bytes) => bytes.is_empty(),
        };
        assert!(refused, "a stranger must be refused, got {answer:?} back");
    }
}
