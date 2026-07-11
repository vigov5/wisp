mod device_name;

pub use device_name::{normalize_hostname_label, process_display_device_name, random_device_name};

// Ticket codec moved to the wasm-clean `wisp-wire` crate; re-export at the
// historical `crate::util` paths so call sites (and the Flutter bridge) are
// unchanged. The `Endpoint`-bound producers below (`make_ticket`,
// `make_ticket_offline`, `make_qr_payload`) stay here and delegate encoding to
// wisp-wire.
pub use wisp_wire::ticket::{
    DecodedTicketInfo, TicketError, decode_ticket, decode_ticket_info, encode_ticket,
    make_ticket_from_addr, synthesize_ticket,
};

use iroh::endpoint::TransportAddrUsage;
use iroh::{Endpoint, TransportAddr};
use std::io::{self, Write};
use thiserror::Error;

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

/// A single candidate transport address iroh is aware of for the peer, plus
/// whether iroh is *actively* using it right now.
///
/// Unlike [`ConnectionPath`] (which collapses everything down to the single
/// active path), this preserves every candidate so the connecting UI can show
/// the full set of IPs/relays being attempted in parallel. iroh 0.97 only
/// exposes `Active`/`Inactive` per address — there is no per-candidate
/// "probing/failed/latency" signal — so `active` is the only liveness bit we
/// can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePath {
    /// Socket address ("ip:port") for direct candidates, or the relay URL for
    /// relay candidates.
    pub addr: String,
    /// `Direct` for an IP candidate, `Relay` for a relay candidate. Never
    /// `Unknown` (we drop addresses that aren't IP or relay).
    pub kind: ConnectionPathKind,
    /// True when iroh reports this candidate's usage as `Active`.
    pub active: bool,
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

/// Snapshot the **actually selected** connection path from a live connection
/// handle.
///
/// [`snapshot_connection_path`] scans the endpoint's address book and reports
/// the first `Active` candidate, preferring direct over relay. That heuristic
/// is wrong when iroh keeps a *stale* direct candidate marked `Active` — e.g.
/// a LAN `192.168.x` address left over from when the peer was on Wi-Fi — even
/// though traffic has migrated to the relay after the peer moved to mobile
/// data. iroh 0.97 exposes only `Active`/`Inactive` per candidate (no
/// recency), so the address book alone can't tell the stale candidate from the
/// real one.
///
/// The connection's [`ConnectionInfo::selected_path`] is authoritative: it's
/// the path QUIC has actually selected to carry traffic. Use this for the
/// connection-path badge. Returns `Unknown` when no path is selected yet (or
/// the connection has been dropped).
pub fn connection_path_from_info(info: &iroh::endpoint::ConnectionInfo) -> ConnectionPath {
    match info.selected_path() {
        Some(path) => connection_path_from_transport_addr(path.remote_addr()),
        None => ConnectionPath::unknown(),
    }
}

/// Pure mapping of a single selected [`TransportAddr`] to a [`ConnectionPath`].
/// Extracted so the Direct/Relay classification is unit-testable without a live
/// iroh connection (which [`connection_path_from_info`] requires).
fn connection_path_from_transport_addr(addr: &TransportAddr) -> ConnectionPath {
    match addr {
        TransportAddr::Ip(socket) => ConnectionPath {
            kind: ConnectionPathKind::Direct,
            relay_url: None,
            direct_addr: Some(socket.to_string()),
        },
        TransportAddr::Relay(url) => ConnectionPath {
            kind: ConnectionPathKind::Relay,
            relay_url: Some(url.to_string()),
            direct_addr: None,
        },
        _ => ConnectionPath::unknown(),
    }
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

/// Snapshot *every* candidate transport address iroh currently knows about for
/// `remote_id`, each tagged with whether it's the active path. Used by the
/// connecting UI to show the full set of IPs/relays being attempted in
/// parallel (see [`CandidatePath`]).
///
/// Returns an empty vec when iroh has no record of the peer yet (e.g. the dial
/// hasn't been registered). Addresses that are neither IP nor relay are
/// dropped — iroh's `TransportAddr` is `#[non_exhaustive]`.
pub async fn snapshot_connection_candidates(
    endpoint: &iroh::Endpoint,
    remote_id: iroh::EndpointId,
) -> Vec<CandidatePath> {
    let Some(info) = endpoint.remote_info(remote_id).await else {
        return Vec::new();
    };

    info.addrs()
        .filter_map(|addr| {
            let active = matches!(addr.usage(), TransportAddrUsage::Active);
            match addr.addr() {
                TransportAddr::Ip(socket) => Some(CandidatePath {
                    addr: socket.to_string(),
                    kind: ConnectionPathKind::Direct,
                    active,
                }),
                TransportAddr::Relay(url) => Some(CandidatePath {
                    addr: url.to_string(),
                    kind: ConnectionPathKind::Relay,
                    active,
                }),
                _ => None,
            }
        })
        .collect()
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
    wisp_wire::ticket::encode_qr_payload(ticket, device_name, device_type)
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

    #[test]
    fn selected_ip_addr_maps_to_direct() {
        // The selected-path mapping behind `connection_path_from_info`: an IP
        // path is Direct and carries the socket, never a relay url.
        let path = connection_path_from_transport_addr(&ip_addr());
        assert_eq!(path.kind, ConnectionPathKind::Direct);
        assert_eq!(path.direct_addr.as_deref(), Some("127.0.0.1:0"));
        assert!(path.relay_url.is_none());
    }

    #[test]
    fn selected_relay_addr_maps_to_relay() {
        let path = connection_path_from_transport_addr(&relay_addr("https://relay.example/"));
        assert_eq!(path.kind, ConnectionPathKind::Relay);
        assert_eq!(path.relay_url.as_deref(), Some("https://relay.example/"));
        assert!(path.direct_addr.is_none());
    }
}
