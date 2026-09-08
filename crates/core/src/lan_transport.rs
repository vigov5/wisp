//! Choosing and opening the LAN TCP transport.
//!
//! The blob protocol runs over any bidirectional stream pair, and over TCP it
//! reached 97% of line rate from a phone where QUIC reached 28% — the phone's
//! 4.14 kernel has no UDP GSO, so quinn pays a `sendmsg` per datagram. See
//! `examples/blob_over_tcp.rs` and the 2026-09-08 section of
//! `docs/transfer-performance-plan.md`.
//!
//! This module answers two questions and nothing else: *should* this peer be
//! reached over TCP, and how is that connection opened and authenticated.
//! Deciding to use it, and feeding the result to the get protocol, belongs to
//! the callers.
//!
//! # Why decide rather than race
//!
//! Racing TCP against QUIC would mean paying the QUIC dial every time, which is
//! the cost being removed. It is also the expensive direction on this codebase's
//! own evidence: a receiver advertises up to nine addresses of which one is
//! dialable, worth ~2.5 s between manifest and offer, and the USB work found
//! dead advertised addresses costing iroh 15-20 s. So: decide from the
//! addresses already in hand, try one thing with a short cap, and fall back.
//!
//! Both ways of being wrong are cheap. A missed subnet match falls back to QUIC
//! and loses only the speedup; a wrong match fails the connect in milliseconds
//! on a LAN and then falls back. That is what makes a decision affordable where
//! a race is not.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use iroh::{EndpointAddr, PublicKey, SecretKey, TransportAddr};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};
use tracing::debug;

use iroh_blobs::util::{AsyncReadRecvStream, AsyncWriteSendStream};

use crate::blobs::error::{BlobError, BlobTextError, Result};
use crate::lan_tls;

/// How long a LAN TCP connect and handshake may take before falling back.
///
/// The round trip measured between two phones on the same access point is
/// 6 ms, and a refused connection returns immediately, so this only bites when
/// something blackholes the port — a host firewall defaulting to block inbound,
/// which is the common case on Windows. Long enough to absorb a slow handshake
/// on a busy phone, short enough that paying it and then dialling QUIC is still
/// better than having raced.
pub const LAN_TCP_CONNECT_CAP: Duration = Duration::from_millis(1_000);

/// One of this device's IPv4 interfaces, as an address and its real netmask.
///
/// A netmask rather than the 3-octet compare used elsewhere in this module's
/// neighbours: `if_addrs` reports the real prefix, so a /16 or a /30 is
/// classified correctly instead of being assumed to be a /24.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalNet {
    pub addr: Ipv4Addr,
    pub mask: Ipv4Addr,
}

impl LocalNet {
    /// True when `peer` sits inside this network.
    fn contains(&self, peer: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask);
        (u32::from(peer) & mask) == (u32::from(self.addr) & mask)
    }

    /// Prefix length, used to prefer the most specific matching network.
    fn prefix_len(&self) -> u32 {
        u32::from(self.mask).count_ones()
    }
}

/// This device's non-loopback IPv4 networks.
pub fn local_ipv4_nets() -> Vec<LocalNet> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .filter_map(|iface| match iface.addr {
            if_addrs::IfAddr::V4(v4) => Some(LocalNet {
                addr: v4.ip,
                mask: v4.netmask,
            }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect()
}

/// Where to reach `peer` over the LAN TCP transport, if anywhere.
///
/// Requires both that the peer published a port (see
/// [`crate::lan::NearbyReceiver::tcp_port`]) and that one of the addresses it
/// advertises shares a subnet with one of ours — the transport is for peers
/// already on the same link, where the relay and hole punching that justify
/// QUIC are not in play.
///
/// When several advertised addresses match, the one on the **most specific**
/// network wins. That is what puts the USB cable's `/30` ahead of Wi-Fi's `/24`
/// on a pair connected both ways, matching the preference the QUIC dial already
/// applies for the same reason.
pub fn tcp_target(
    peer: &EndpointAddr,
    tcp_port: Option<u16>,
    local_nets: &[LocalNet],
) -> Option<SocketAddr> {
    let port = tcp_port?;
    peer.addrs
        .iter()
        .filter_map(|transport| match transport {
            TransportAddr::Ip(SocketAddr::V4(v4)) => Some(*v4.ip()),
            _ => None,
        })
        .filter_map(|ip| {
            local_nets
                .iter()
                .filter(|net| net.contains(ip))
                .map(|net| net.prefix_len())
                .max()
                .map(|specificity| (specificity, ip))
        })
        .max_by_key(|(specificity, _)| *specificity)
        .map(|(_, ip)| SocketAddr::new(IpAddr::V4(ip), port))
}

/// A LAN transport connection: TCP, with the TLS from [`crate::lan_tls`] on it.
pub type LanStream = TlsStream<TcpStream>;

/// Connects to `target` and authenticates it as `expected_peer`, giving up
/// after [`LAN_TCP_CONNECT_CAP`].
///
/// The cap covers the connect *and* the handshake together, because to the
/// caller they are one wait: what matters is how long before falling back to
/// QUIC, not which half was slow.
pub async fn dial(
    target: SocketAddr,
    secret: &SecretKey,
    expected_peer: PublicKey,
) -> Result<LanStream> {
    let config = lan_tls::client_config(secret, expected_peer)?;
    let connect = async {
        let tcp = TcpStream::connect(target)
            .await
            .map_err(|source| connect_failed(target, source))?;
        // Nagle would hold back the small frames the get protocol's request and
        // header phases produce, waiting for an ack that has nothing to ride on.
        tcp.set_nodelay(true)
            .map_err(|source| connect_failed(target, source))?;
        // The pinned key is the identity; this name is required by the API and
        // is not part of the trust decision. See `lan_tls::client_config`.
        let name = rustls::pki_types::ServerName::try_from("lan.wisp.invalid")
            .expect("a static, valid DNS name");
        TlsConnector::from(config)
            .connect(name, tcp)
            .await
            .map_err(|source| connect_failed(target, source))
    };
    match tokio::time::timeout(LAN_TCP_CONNECT_CAP, connect).await {
        Ok(result) => {
            let stream = result?;
            debug!(%target, "lan_tcp.dialed");
            Ok(TlsStream::Client(stream))
        }
        Err(_) => Err(BlobError::connect(
            format!("lan tcp {target}"),
            BlobTextError::new(format!(
                "no answer within {} ms",
                LAN_TCP_CONNECT_CAP.as_millis()
            )),
        )),
    }
}

/// Completes the server side of the handshake on an accepted socket, and
/// reports which peer it turned out to be.
///
/// The identity is returned rather than checked here: whether that peer is
/// welcome — paired, expected, not rate-limited — is the application's
/// decision, and it already makes it once for the QUIC path.
pub async fn accept(tcp: TcpStream, secret: &SecretKey) -> Result<(LanStream, PublicKey)> {
    let peer_addr = tcp.peer_addr().ok();
    let describe = || match peer_addr {
        Some(addr) => format!("lan tcp {addr}"),
        None => "lan tcp".to_owned(),
    };
    tcp.set_nodelay(true)
        .map_err(|source| BlobError::connect(describe(), source))?;
    let config = lan_tls::server_config(secret)?;
    let stream = TlsAcceptor::from(config)
        .accept(tcp)
        .await
        .map_err(|source| BlobError::connect(describe(), source))?;
    let identity = {
        let (_, connection) = stream.get_ref();
        lan_tls::peer_identity(connection.peer_certificates())?
    };
    debug!(peer = %identity.fmt_short(), "lan_tcp.accepted");
    Ok((TlsStream::Server(stream), identity))
}

fn connect_failed(target: SocketAddr, source: std::io::Error) -> BlobError {
    BlobError::connect(format!("lan tcp {target}"), source)
}

/// Splits a connection into the halves `iroh-blobs` drives its get protocol
/// over.
///
/// The crate's `AsyncReadRecvStream`/`AsyncWriteSendStream` do the framing; all
/// these wrappers add is the three questions those helpers ask, and two of them
/// have no TCP answer. A TCP stream has no per-stream error codes, so `stop`
/// and `reset` are no-ops and `stopped` reports "not stopped" — answered
/// honestly rather than emulated, because a caller that needed real QUIC stream
/// semantics would be silently misled by a fake.
pub fn stream_halves(stream: LanStream) -> (LanRecv, LanSend) {
    let (read, write) = tokio::io::split(stream);
    (
        AsyncReadRecvStream::new(LanRecvStream::new(read)),
        AsyncWriteSendStream::new(LanSendStream::new(write)),
    )
}

/// The receive half of a LAN connection, ready for `StreamPair::new`.
///
/// The wrapping matters and is easy to get wrong: [`LanRecvStream`] implements
/// `AsyncReadRecvStreamExtra`, which is the *input* to `iroh-blobs`' adapter —
/// it is `AsyncReadRecvStream` around it that implements `RecvStream`. Handing
/// the inner type to the get protocol does not compile, so these aliases spell
/// the finished shape out once.
pub type LanRecv = AsyncReadRecvStream<LanRecvStream<ReadHalf<LanStream>>>;

/// The send half of a LAN connection, ready for `StreamPair::new`.
pub type LanSend = AsyncWriteSendStream<LanSendStream<WriteHalf<LanStream>>>;

/// The read half of a byte stream, as an `iroh-blobs` receive stream.
///
/// Generic over the half so the TLS transport and `examples/blob_over_tcp.rs`
/// share one implementation: the interesting part is the answers below, and
/// having them written twice would be two places to get them wrong.
pub struct LanRecvStream<R>(R);

impl<R> LanRecvStream<R> {
    pub fn new(inner: R) -> Self {
        Self(inner)
    }
}

impl<R: AsyncRead + Unpin + Send> iroh_blobs::util::AsyncReadRecvStreamExtra for LanRecvStream<R> {
    fn inner(&mut self) -> &mut (impl AsyncRead + Unpin + Send) {
        &mut self.0
    }

    fn stop(&mut self, _code: iroh::endpoint::VarInt) -> std::io::Result<()> {
        Ok(())
    }

    fn id(&self) -> u64 {
        0
    }
}

/// The write half of a byte stream, as an `iroh-blobs` send stream.
pub struct LanSendStream<W>(W);

impl<W> LanSendStream<W> {
    pub fn new(inner: W) -> Self {
        Self(inner)
    }
}

impl<W: AsyncWrite + Unpin + Send> iroh_blobs::util::AsyncWriteSendStreamExtra
    for LanSendStream<W>
{
    fn inner(&mut self) -> &mut (impl AsyncWrite + Unpin + Send) {
        &mut self.0
    }

    fn reset(&mut self, _code: iroh::endpoint::VarInt) -> std::io::Result<()> {
        Ok(())
    }

    async fn stopped(&mut self) -> std::io::Result<Option<iroh::endpoint::VarInt>> {
        Ok(None)
    }

    fn id(&self) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_blobs::util::{RecvStream, SendStream};
    use tokio::net::TcpListener;

    fn net(addr: &str, mask: &str) -> LocalNet {
        LocalNet {
            addr: addr.parse().unwrap(),
            mask: mask.parse().unwrap(),
        }
    }

    fn peer_at(addrs: &[&str]) -> EndpointAddr {
        let key = SecretKey::generate().public();
        EndpointAddr::from_parts(
            key,
            addrs
                .iter()
                .map(|a| TransportAddr::Ip(a.parse().unwrap()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn no_port_means_no_tcp() {
        let peer = peer_at(&["192.168.1.50:11223"]);
        let nets = [net("192.168.1.10", "255.255.255.0")];
        assert_eq!(tcp_target(&peer, None, &nets), None);
    }

    #[test]
    fn a_peer_off_our_subnets_is_not_a_tcp_target() {
        // Advertised addresses that are real but unreachable are the norm here:
        // a carrier address and a VPN tunnel were both seen on a test device.
        let peer = peer_at(&["166.161.40.241:11223", "10.109.207.8:11223"]);
        let nets = [net("192.168.1.10", "255.255.255.0")];
        assert_eq!(tcp_target(&peer, Some(5300), &nets), None);
    }

    #[test]
    fn picks_the_address_on_our_subnet() {
        let peer = peer_at(&["10.109.207.8:11223", "192.168.1.50:11223"]);
        let nets = [net("192.168.1.10", "255.255.255.0")];
        assert_eq!(
            tcp_target(&peer, Some(5300), &nets),
            Some("192.168.1.50:5300".parse().unwrap())
        );
    }

    #[test]
    fn the_most_specific_network_wins() {
        // Reachable over both Wi-Fi and the USB cable; the cable is the /30.
        let peer = peer_at(&["192.168.1.50:11223", "10.42.0.2:11223"]);
        let nets = [
            net("192.168.1.10", "255.255.255.0"),
            net("10.42.0.1", "255.255.255.252"),
        ];
        assert_eq!(
            tcp_target(&peer, Some(5300), &nets),
            Some("10.42.0.2:5300".parse().unwrap())
        );
    }

    #[test]
    fn a_real_netmask_is_used_not_an_assumed_slash_24() {
        let peer = peer_at(&["10.0.5.7:11223"]);
        // On a /16 the peer is local; assuming /24 would have missed it.
        assert_eq!(
            tcp_target(&peer, Some(5300), &[net("10.0.1.2", "255.255.0.0")]),
            Some("10.0.5.7:5300".parse().unwrap())
        );
        // On a /24 it is not.
        assert_eq!(
            tcp_target(&peer, Some(5300), &[net("10.0.1.2", "255.255.255.0")]),
            None
        );
    }

    #[test]
    fn ipv6_and_relay_addresses_are_ignored() {
        let peer = peer_at(&["[2405:4802:1beb:db30::1]:11223"]);
        let nets = [net("192.168.1.10", "255.255.255.0")];
        assert_eq!(tcp_target(&peer, Some(5300), &nets), None);
    }

    #[tokio::test]
    async fn a_dialled_connection_carries_bytes_and_both_identities() {
        let server_key = SecretKey::generate();
        let client_key = SecretKey::generate();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_secret = server_key.clone();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let (stream, identity) = accept(tcp, &server_secret).await.expect("accept");
            // Exercise the halves the get protocol would be driven over.
            // Drive the halves through the traits the get protocol uses, so
            // the test fails if they are ever wrapped at the wrong level.
            let (mut recv, mut send) = stream_halves(stream);
            let got = recv.recv_bytes_exact(5).await.unwrap();
            send.send(b"pong!").await.unwrap();
            send.sync().await.unwrap();
            (identity, got)
        });

        let stream = dial(addr, &client_key, server_key.public())
            .await
            .expect("dial");
        let (mut recv, mut send) = stream_halves(stream);
        send.send(b"ping!").await.unwrap();
        send.sync().await.unwrap();
        let got = recv.recv_bytes_exact(5).await.unwrap();
        assert_eq!(got.as_ref(), b"pong!");

        let (seen_by_server, seen_by_client) = server.await.unwrap();
        assert_eq!(seen_by_server, client_key.public());
        assert_eq!(seen_by_client.as_ref(), b"ping!");
    }

    #[tokio::test]
    async fn dialling_the_wrong_identity_fails() {
        let server_key = SecretKey::generate();
        let impostor = SecretKey::generate();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_secret = server_key.clone();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let _ = accept(tcp, &server_secret).await;
            }
        });

        let result = dial(addr, &SecretKey::generate(), impostor.public()).await;
        assert!(result.is_err(), "pinning must reject the wrong peer");
    }

    #[tokio::test]
    async fn a_blackholed_port_gives_up_within_the_cap() {
        // A listener that accepts TCP and then never speaks TLS is the shape a
        // firewall or a half-open port produces, and the one the cap exists for.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let held = listener.accept().await;
            // Hold the connection open, saying nothing.
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(held);
        });

        let started = std::time::Instant::now();
        let result = dial(addr, &SecretKey::generate(), SecretKey::generate().public()).await;
        let waited = started.elapsed();
        assert!(result.is_err(), "a silent peer must not succeed");
        assert!(
            waited < LAN_TCP_CONNECT_CAP * 3,
            "gave up after {waited:?}, cap is {LAN_TCP_CONNECT_CAP:?}"
        );
    }
}
