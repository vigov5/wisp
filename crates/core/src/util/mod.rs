mod device_name;

pub use device_name::{normalize_hostname_label, process_display_device_name, random_device_name};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::endpoint::TransportAddrUsage;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransferTicket {
    node_id: String,
    addrs: Vec<EncodedTransportAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum EncodedTransportAddr {
    Relay(String),
    Ip(String),
}

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
pub enum TicketError {
    #[error("serializing transfer ticket")]
    Serialize {
        #[source]
        source: Box<bincode::ErrorKind>,
    },
    #[error("decoding ticket from base64")]
    DecodeBase64 {
        #[source]
        source: base64::DecodeError,
    },
    #[error("ticket payload is not a supported drift ticket")]
    InvalidPayload,
    #[error("parsing node id {value}")]
    ParseNodeId {
        value: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("parsing relay url {value}")]
    ParseRelayUrl {
        value: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("parsing socket addr {value}")]
    ParseSocketAddr {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },
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

fn make_ticket_from_addr(addr: EndpointAddr) -> std::result::Result<String, TicketError> {
    let ticket = TransferTicket {
        node_id: addr.id.to_string(),
        addrs: addr
            .addrs
            .into_iter()
            .map(EncodedTransportAddr::from)
            .collect(),
    };

    let bytes = bincode::serialize(&ticket).map_err(|source| TicketError::Serialize { source })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_ticket(ticket: &str) -> std::result::Result<EndpointAddr, TicketError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(ticket)
        .map_err(|source| TicketError::DecodeBase64 { source })?;
    let ticket = parse_transfer_ticket(&bytes)?;

    let node_id = ticket
        .node_id
        .parse()
        .map_err(|source| TicketError::ParseNodeId {
            value: ticket.node_id.clone(),
            source: Box::new(source),
        })?;

    let addrs = ticket
        .addrs
        .into_iter()
        .map(TryInto::try_into)
        .collect::<std::result::Result<Vec<TransportAddr>, TicketError>>()?;

    Ok(EndpointAddr::new(node_id).with_addrs(addrs))
}

fn parse_transfer_ticket(bytes: &[u8]) -> std::result::Result<TransferTicket, TicketError> {
    bincode::deserialize::<TransferTicket>(bytes).map_err(|_| TicketError::InvalidPayload)
}

impl From<TransportAddr> for EncodedTransportAddr {
    fn from(value: TransportAddr) -> Self {
        match value {
            TransportAddr::Relay(url) => Self::Relay(url.to_string()),
            TransportAddr::Ip(addr) => Self::Ip(addr.to_string()),
            _ => unreachable!("unsupported transport address variant"),
        }
    }
}

impl TryFrom<EncodedTransportAddr> for TransportAddr {
    type Error = TicketError;

    fn try_from(value: EncodedTransportAddr) -> std::result::Result<Self, Self::Error> {
        match value {
            EncodedTransportAddr::Relay(url) => {
                Ok(TransportAddr::Relay(url.parse().map_err(|source| {
                    TicketError::ParseRelayUrl {
                        value: url.clone(),
                        source: Box::new(source),
                    }
                })?))
            }
            EncodedTransportAddr::Ip(addr) => {
                Ok(TransportAddr::Ip(addr.parse().map_err(|source| {
                    TicketError::ParseSocketAddr {
                        value: addr.clone(),
                        source,
                    }
                })?))
            }
        }
    }
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
}
