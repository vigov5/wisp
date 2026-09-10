//! Where a blob request's stream pair comes from.
//!
//! The get protocol needs one thing from a transport: a fresh bidirectional
//! stream pair per request. QUIC opens one on a long-lived connection; the LAN
//! TCP transport has exactly one stream per connection, so it dials per
//! request. [`BlobSource`] is that difference and nothing else.
//!
//! Per-request dialling is the cost of choosing TCP: a transfer of *n* files
//! pays *n* connects and TLS handshakes instead of one, roughly two round trips
//! each — about 12 ms on the 6 ms LAN this was measured on. Acceptable because
//! it buys a payload path that runs at 97% of line rate where QUIC managed 28%
//! on the same phone, and because it leaves the per-file request and resume
//! structure exactly as it is. Folding a whole collection into one hash-seq
//! request would remove the repetition, but it also rewrites how `resume_at`
//! turns into range specs, so it is a later optimisation with a number attached
//! rather than a prerequisite.

use std::io;
use std::net::SocketAddr;

use bytes::Bytes;
use iroh::endpoint::{Connection, VarInt};
use iroh::{PublicKey, SecretKey};
use iroh_blobs::util::{RecvStream, SendStream};

use super::error::{BlobError, Result};
use crate::lan_transport::{self, LanRecv, LanSend};

/// Everything needed to dial a LAN peer, which is more than a `Connection`
/// carries: a fresh connection per request means a fresh handshake, so the key
/// to authenticate with and the identity to pin travel with the address.
///
/// Boxed inside [`BlobSource`] because it is several times the size of a
/// `Connection` handle, and the QUIC variant is the one on every code path.
#[derive(Debug, Clone)]
pub(crate) struct LanTarget {
    pub(crate) target: SocketAddr,
    pub(crate) secret: SecretKey,
    pub(crate) peer: PublicKey,
}

/// A transport a blob request can be issued over.
#[derive(Debug, Clone)]
pub(crate) enum BlobSource {
    /// An established QUIC connection; each request opens a bi-stream on it.
    Quic(Connection),
    /// A peer on this LAN, dialled per request.
    LanTcp(Box<LanTarget>),
}

impl BlobSource {
    /// Opens a stream pair for one request.
    pub(crate) async fn open(&self) -> Result<(BlobRecv, BlobSend)> {
        match self {
            Self::Quic(connection) => {
                let (send, recv) = connection
                    .open_bi()
                    .await
                    .map_err(|source| BlobError::connect("blob quic stream".to_owned(), source))?;
                Ok((BlobRecv::Quic(recv), BlobSend::Quic(send)))
            }
            Self::LanTcp(lan) => {
                // `dial_for_request`, not `dial`: the probe already decided
                // this transport, so a slow handshake here is a hiccup to
                // retry rather than a reason to abandon the transfer.
                let stream =
                    lan_transport::dial_for_request(lan.target, &lan.secret, lan.peer).await?;
                let (recv, send) = lan_transport::stream_halves(stream);
                Ok((BlobRecv::Lan(recv), BlobSend::Lan(send)))
            }
        }
    }

    /// How the source describes itself in an error or a log line.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Quic(_) => "quic".to_owned(),
            Self::LanTcp(lan) => format!("lan tcp {}", lan.target),
        }
    }
}

/// The receive half of whichever transport opened the pair.
///
/// A hand-written enum rather than a boxed trait object: `RecvStream`'s methods
/// return `impl Future`, which is not dyn-compatible, so dispatch has to be
/// static. The bodies are pure forwarding — the interesting part is that both
/// arms already satisfy the trait, one from `iroh-blobs` and one from
/// [`crate::lan_transport`].
pub(crate) enum BlobRecv {
    Quic(iroh::endpoint::RecvStream),
    Lan(LanRecv),
}

impl RecvStream for BlobRecv {
    async fn recv_bytes(&mut self, len: usize) -> io::Result<Bytes> {
        match self {
            Self::Quic(stream) => stream.recv_bytes(len).await,
            Self::Lan(stream) => stream.recv_bytes(len).await,
        }
    }

    async fn recv_bytes_exact(&mut self, len: usize) -> io::Result<Bytes> {
        match self {
            Self::Quic(stream) => stream.recv_bytes_exact(len).await,
            Self::Lan(stream) => stream.recv_bytes_exact(len).await,
        }
    }

    async fn recv_exact(&mut self, target: &mut [u8]) -> io::Result<()> {
        match self {
            Self::Quic(stream) => stream.recv_exact(target).await,
            Self::Lan(stream) => stream.recv_exact(target).await,
        }
    }

    fn stop(&mut self, code: VarInt) -> io::Result<()> {
        // Qualified: `iroh::endpoint::RecvStream` has inherent `stop`/`id` of
        // its own that would shadow the trait's and return other types.
        match self {
            Self::Quic(stream) => RecvStream::stop(stream, code),
            Self::Lan(stream) => RecvStream::stop(stream, code),
        }
    }

    fn id(&self) -> u64 {
        match self {
            Self::Quic(stream) => RecvStream::id(stream),
            Self::Lan(stream) => RecvStream::id(stream),
        }
    }
}

/// The send half of whichever transport opened the pair. See [`BlobRecv`].
pub(crate) enum BlobSend {
    Quic(iroh::endpoint::SendStream),
    Lan(LanSend),
}

impl SendStream for BlobSend {
    async fn send_bytes(&mut self, bytes: Bytes) -> io::Result<()> {
        match self {
            Self::Quic(stream) => stream.send_bytes(bytes).await,
            Self::Lan(stream) => stream.send_bytes(bytes).await,
        }
    }

    async fn send(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Quic(stream) => stream.send(buf).await,
            Self::Lan(stream) => stream.send(buf).await,
        }
    }

    async fn sync(&mut self) -> io::Result<()> {
        match self {
            Self::Quic(stream) => stream.sync().await,
            Self::Lan(stream) => stream.sync().await,
        }
    }

    fn reset(&mut self, code: VarInt) -> io::Result<()> {
        // See `BlobRecv::stop` — same shadowing, same fix.
        match self {
            Self::Quic(stream) => SendStream::reset(stream, code),
            Self::Lan(stream) => SendStream::reset(stream, code),
        }
    }

    async fn stopped(&mut self) -> io::Result<Option<VarInt>> {
        match self {
            Self::Quic(stream) => SendStream::stopped(stream).await,
            Self::Lan(stream) => SendStream::stopped(stream).await,
        }
    }

    fn id(&self) -> u64 {
        match self {
            Self::Quic(stream) => SendStream::id(stream),
            Self::Lan(stream) => SendStream::id(stream),
        }
    }
}
