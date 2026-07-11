//! Base64 / bincode ticket codec — the pure, wasm-clean half of what used to live
//! in `wisp_core::util`.
//!
//! A [`TransferTicket`] captures a peer's iroh `EndpointId` plus its transport
//! addresses (relay URL and/or direct IPs), serialized as base64url(bincode). The
//! `Endpoint`-bound helpers that *produce* an `EndpointAddr` (`make_ticket`,
//! `make_ticket_offline`, `make_qr_payload`, `lan_direct_addrs`) stay native-side
//! in `wisp-core`; this module only encodes/decodes an already-known address, so
//! it compiles for the browser receiver too.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::{EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransferTicket {
    node_id: String,
    addrs: Vec<EncodedTransportAddr>,
}

/// JSON envelope used by QR pairing. Carries the original ticket plus
/// device info so the sender can show "From <name>" on its tile before
/// dialing.  Wire-format separate from `TransferTicket` (bincode) so the
/// LAN broadcast stays binary-compatible with older wisp binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct QrPayload {
    /// The same string that `make_ticket` / `make_ticket_offline` produce —
    /// base64 of bincode `TransferTicket`.
    ticket: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    device_type: String,
}

/// Magic prefix on a QR-encoded pairing payload so the scanner can
/// distinguish it from a raw ticket string. Short enough to keep the
/// QR small while still being self-describing.
const QR_PAYLOAD_PREFIX: &str = "wisp-pair:";

/// Public read-only view of a decoded ticket. Returned by [`decode_ticket_info`]
/// so callers (e.g., Flutter QR scan) can pre-populate UI before dialing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTicketInfo {
    pub endpoint_addr: EndpointAddr,
    /// Empty string when the ticket didn't carry a name (older format).
    pub device_name: String,
    pub device_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum EncodedTransportAddr {
    Relay(String),
    Ip(String),
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
    #[error("ticket payload is not a supported wisp ticket")]
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

/// Round-trips an already-known [`EndpointAddr`] back into the base64 ticket
/// string callers can persist.  Used by the sender to capture the ticket it
/// got from `claim_peer` (which is owned by the rendezvous response, not the
/// caller's request) so the saved-devices list can fast-reconnect later.
pub fn encode_ticket(addr: EndpointAddr) -> std::result::Result<String, TicketError> {
    make_ticket_from_addr(addr)
}

/// Encode an [`EndpointAddr`] into the base64url(bincode) ticket string.
///
/// The `Endpoint`-bound producers in `wisp-core` (`make_ticket`,
/// `make_ticket_offline`) build the `EndpointAddr` and call this to serialize it.
pub fn make_ticket_from_addr(addr: EndpointAddr) -> std::result::Result<String, TicketError> {
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

/// Wrap an already-encoded ticket string plus device info into the QR-pairing
/// payload form: `"wisp-pair:" + base64url(json)`. Split out from the
/// `Endpoint`-bound `make_qr_payload` (which stays in `wisp-core`) so the pure
/// JSON+prefix encoding lives next to the codec it mirrors.
pub fn encode_qr_payload(
    ticket: String,
    device_name: &str,
    device_type: &str,
) -> std::result::Result<String, TicketError> {
    let payload = QrPayload {
        ticket,
        device_name: device_name.to_owned(),
        device_type: device_type.to_owned(),
    };
    let json = serde_json::to_vec(&payload).map_err(|_| TicketError::InvalidPayload)?;
    Ok(format!(
        "{QR_PAYLOAD_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(json)
    ))
}

/// Build a ticket string from a peer's `EndpointId` plus optional connection
/// hints. Used by the receiver after a transfer completes to persist a
/// "send back" ticket without requiring an active iroh `Endpoint`.
///
/// Empty / blank hint strings are dropped silently. Invalid relay URLs or
/// socket addresses produce a `TicketError`. Callers in event-emission paths
/// should treat errors as "no ticket"; iroh's pkarr discovery can resolve a
/// ticket carrying only an `EndpointId`, so empty `addrs` is acceptable.
pub fn synthesize_ticket(
    endpoint_id: iroh::EndpointId,
    relay_url: Option<&str>,
    direct_addr: Option<&str>,
) -> std::result::Result<String, TicketError> {
    let mut addrs: Vec<EncodedTransportAddr> = Vec::new();

    if let Some(s) = direct_addr.map(str::trim).filter(|s| !s.is_empty()) {
        // Validate via parse; the on-the-wire form keeps the original string.
        let _: std::net::SocketAddr = s.parse().map_err(|source| TicketError::ParseSocketAddr {
            value: s.to_owned(),
            source,
        })?;
        addrs.push(EncodedTransportAddr::Ip(s.to_owned()));
    }

    if let Some(s) = relay_url.map(str::trim).filter(|s| !s.is_empty()) {
        let _: iroh::RelayUrl = s.parse().map_err(|source| TicketError::ParseRelayUrl {
            value: s.to_owned(),
            source: Box::new(source),
        })?;
        addrs.push(EncodedTransportAddr::Relay(s.to_owned()));
    }

    let ticket = TransferTicket {
        node_id: endpoint_id.to_string(),
        addrs,
    };

    let bytes = bincode::serialize(&ticket).map_err(|source| TicketError::Serialize { source })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Decode a QR-pairing payload OR a plain ticket string. Returns the
/// endpoint addr plus optional device name/type — both empty for plain
/// tickets that don't carry that info.
///
/// Accepted inputs:
/// - `"wisp-pair:<base64url(json{ticket, device_name, device_type})>"` — new QR format.
/// - Plain ticket string (base64url of bincode `TransferTicket`) — older QR / paste.
pub fn decode_ticket_info(input: &str) -> std::result::Result<DecodedTicketInfo, TicketError> {
    let trimmed = input.trim();

    if let Some(rest) = trimmed.strip_prefix(QR_PAYLOAD_PREFIX) {
        let bytes = URL_SAFE_NO_PAD
            .decode(rest)
            .map_err(|source| TicketError::DecodeBase64 { source })?;
        let payload: QrPayload =
            serde_json::from_slice(&bytes).map_err(|_| TicketError::InvalidPayload)?;
        let endpoint_addr = decode_ticket(&payload.ticket)?;
        return Ok(DecodedTicketInfo {
            endpoint_addr,
            device_name: payload.device_name,
            device_type: payload.device_type,
        });
    }

    // Fallback: plain ticket without device info.
    Ok(DecodedTicketInfo {
        endpoint_addr: decode_ticket(trimmed)?,
        device_name: String::new(),
        device_type: String::new(),
    })
}

pub fn decode_ticket(input: &str) -> std::result::Result<EndpointAddr, TicketError> {
    let trimmed = input.trim();
    // Accept QR-payload form too: "wisp-pair:<base64url(json{ticket,...})>" —
    // unwrap to the inner ticket and recurse so the send flow can use the
    // QR-scanned string directly without first stripping the prefix.
    if let Some(rest) = trimmed.strip_prefix(QR_PAYLOAD_PREFIX) {
        let bytes = URL_SAFE_NO_PAD
            .decode(rest)
            .map_err(|source| TicketError::DecodeBase64 { source })?;
        let payload: QrPayload =
            serde_json::from_slice(&bytes).map_err(|_| TicketError::InvalidPayload)?;
        return decode_ticket(&payload.ticket);
    }

    let bytes = URL_SAFE_NO_PAD
        .decode(trimmed)
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
mod synthetic_ticket_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[7u8; 32]).public()
    }

    #[test]
    fn empty_hints_round_trip_endpoint_id_only() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, None, None).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert!(decoded.addrs.is_empty());
    }

    #[test]
    fn blank_hints_treated_as_none() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("   "), Some("")).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert!(decoded.addrs.is_empty());
    }

    #[test]
    fn relay_only_round_trips() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("https://relay.example/"), None).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        let has_relay = decoded.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        assert!(has_relay, "decoded ticket missing relay addr");
    }

    #[test]
    fn direct_only_round_trips() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, None, Some("192.168.1.5:5000")).expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        let has_ip = decoded
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Ip(s) if s.to_string() == "192.168.1.5:5000"));
        assert!(has_ip, "decoded ticket missing direct addr");
    }

    #[test]
    fn both_hints_round_trip() {
        let id = sample_id();
        let ticket = synthesize_ticket(id, Some("https://relay.example/"), Some("10.0.0.1:1234"))
            .expect("ticket");
        let decoded = decode_ticket(&ticket).expect("decode");
        assert_eq!(decoded.id, id);
        assert_eq!(decoded.addrs.len(), 2);
    }

    #[test]
    fn invalid_relay_url_errors() {
        let id = sample_id();
        let result = synthesize_ticket(id, Some("not a url"), None);
        assert!(matches!(result, Err(TicketError::ParseRelayUrl { .. })));
    }

    #[test]
    fn invalid_socket_addr_errors() {
        let id = sample_id();
        let result = synthesize_ticket(id, None, Some("not.a.socket"));
        assert!(matches!(result, Err(TicketError::ParseSocketAddr { .. })));
    }
}

#[cfg(test)]
mod qr_payload_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[9u8; 32]).public()
    }

    fn build_qr_payload(ticket: String, name: &str, dtype: &str) -> String {
        encode_qr_payload(ticket, name, dtype).expect("qr payload")
    }

    #[test]
    fn decode_ticket_info_round_trips_qr_payload_fields() {
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("192.168.1.5:5000")).expect("inner ticket");
        let qr = build_qr_payload(inner, "Maya MacBook", "laptop");

        let info = decode_ticket_info(&qr).expect("decode info");
        assert_eq!(info.endpoint_addr.id, id);
        assert_eq!(info.device_name, "Maya MacBook");
        assert_eq!(info.device_type, "laptop");
    }

    #[test]
    fn decode_ticket_info_returns_empty_device_fields_for_plain_ticket() {
        let id = sample_id();
        let plain = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");

        let info = decode_ticket_info(&plain).expect("decode plain");
        assert_eq!(info.endpoint_addr.id, id);
        assert!(info.device_name.is_empty());
        assert!(info.device_type.is_empty());
    }

    #[test]
    fn decode_ticket_accepts_wisp_pair_prefix_and_unwraps() {
        let id = sample_id();
        let inner = synthesize_ticket(id, Some("https://relay.example/"), None).expect("inner");
        let qr = build_qr_payload(inner, "Phone", "phone");

        let addr = decode_ticket(&qr).expect("decode");
        assert_eq!(addr.id, id);
        let has_relay = addr.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        assert!(has_relay, "expected relay addr to survive QR-wrap unwrap");
    }

    #[test]
    fn decode_ticket_info_tolerates_surrounding_whitespace() {
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");
        let qr = build_qr_payload(inner, "Pad", "laptop");
        let padded = format!("\n\t  {qr}  \n");

        let info = decode_ticket_info(&padded).expect("decode padded");
        assert_eq!(info.endpoint_addr.id, id);
        assert_eq!(info.device_name, "Pad");
    }

    #[test]
    fn decode_ticket_info_errors_on_malformed_base64_after_prefix() {
        let bogus = format!("{QR_PAYLOAD_PREFIX}!!!not-base64!!!");
        let result = decode_ticket_info(&bogus);
        assert!(matches!(result, Err(TicketError::DecodeBase64 { .. })));
    }

    #[test]
    fn decode_ticket_info_errors_on_non_json_payload_after_prefix() {
        let raw = URL_SAFE_NO_PAD.encode(b"this is not json");
        let bogus = format!("{QR_PAYLOAD_PREFIX}{raw}");
        let result = decode_ticket_info(&bogus);
        assert!(matches!(result, Err(TicketError::InvalidPayload)));
    }

    #[test]
    fn qr_payload_missing_device_fields_decode_as_empty() {
        // Older sender wrote `{"ticket": "..."}` only — the decoder must still
        // accept it via serde(default) instead of erroring on missing fields.
        let id = sample_id();
        let inner = synthesize_ticket(id, None, Some("10.0.0.1:1234")).expect("ticket");
        let json = format!(r#"{{"ticket":"{}"}}"#, inner);
        let qr = format!(
            "{QR_PAYLOAD_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(json.as_bytes())
        );

        let info = decode_ticket_info(&qr).expect("decode legacy");
        assert_eq!(info.endpoint_addr.id, id);
        assert!(info.device_name.is_empty());
        assert!(info.device_type.is_empty());
    }
}

#[cfg(test)]
mod encode_ticket_tests {
    use super::*;
    use iroh::SecretKey;

    fn sample_id() -> iroh::EndpointId {
        SecretKey::from_bytes(&[11u8; 32]).public()
    }

    #[test]
    fn encode_round_trips_back_to_endpoint_addr() {
        // The whole point of `encode_ticket`: take an EndpointAddr we already
        // hold (e.g. from a rendezvous claim) and serialize it back so Dart
        // can persist it as the saved-devices `lastTicket`.  Going through
        // `decode_ticket` and back must preserve both the EndpointId and the
        // addr set without loss.
        let id = sample_id();
        let original =
            synthesize_ticket(id, Some("https://relay.example/"), Some("192.168.1.5:5000"))
                .expect("synth");
        let decoded = decode_ticket(&original).expect("decode");

        let encoded = encode_ticket(decoded.clone()).expect("encode");
        let decoded_again = decode_ticket(&encoded).expect("decode again");

        assert_eq!(decoded_again.id, id);
        assert_eq!(decoded_again.addrs.len(), decoded.addrs.len());
        let has_relay = decoded_again.addrs.iter().any(
            |a| matches!(a, TransportAddr::Relay(url) if url.as_str() == "https://relay.example/"),
        );
        let has_ip = decoded_again
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Ip(s) if s.to_string() == "192.168.1.5:5000"));
        assert!(
            has_relay,
            "relay hint must survive encode_ticket round-trip"
        );
        assert!(has_ip, "ip hint must survive encode_ticket round-trip");
    }

    #[test]
    fn encode_endpoint_id_only_when_no_addrs() {
        // Sender's resolved EndpointAddr from pkarr can legitimately carry
        // zero addrs (relay-less, direct unknown).  encode_ticket must still
        // produce a valid ticket so the lastTicket persistence path doesn't
        // silently drop those peers.
        let id = sample_id();
        let original = synthesize_ticket(id, None, None).expect("synth");
        let decoded = decode_ticket(&original).expect("decode");
        assert!(decoded.addrs.is_empty());

        let encoded = encode_ticket(decoded).expect("encode");
        let decoded_again = decode_ticket(&encoded).expect("decode again");
        assert_eq!(decoded_again.id, id);
        assert!(decoded_again.addrs.is_empty());
    }
}
