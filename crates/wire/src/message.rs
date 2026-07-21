#![allow(dead_code)]

use iroh::EndpointId;
use iroh_blobs::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plan::{TransferFileId, TransferPhase, TransferPlan};

/// ALPN for the `wisp/transfer/v1` control protocol. The receiver advertises
/// this on its iroh endpoint; the sender dials it to run the handshake.
pub const ALPN: &[u8] = b"wisp/transfer/v1";

pub const PROTOCOL_VERSION: u32 = 5;

/// Maximum UTF-8 byte length of text carried inline on the control stream via
/// [`Offer::inline_text`].  Text at or below this rides the offer frame itself
/// (no iroh-blobs, effectively instant); anything larger falls back to a
/// synthetic `.txt` file sent through the normal blob pipeline.
pub const INLINE_TEXT_MAX_BYTES: usize = 16 * 1024;

/// Hard ceiling the receiver enforces on a peer-supplied [`Offer::inline_text`]
/// before allocating / rendering it.  Generously above [`INLINE_TEXT_MAX_BYTES`]
/// so a well-behaved sender never trips it, but bounds a malicious/buggy peer
/// from shipping an unbounded control frame.
pub const INLINE_TEXT_HARD_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Phone,
    Laptop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferRole {
    Sender,
    Receiver,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Hello,
    Offer,
    OfferAck,
    Accept,
    Decline,
    Cancel,
    TransferStarted,
    TransferProgress,
    TransferCompleted,
    BlobTicket,
    TransferResult,
    TransferAck,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub role: TransferRole,
    pub endpoint_id: EndpointId,
    pub device_name: String,
    pub device_type: DeviceType,
    /// True when this peer is the browser receiver, so the other side can render
    /// a web glyph for it instead of the phone/laptop `device_type`. Optional on
    /// the wire (`default` = false) so peers that predate the field still
    /// deserialize.
    #[serde(default)]
    pub web: bool,
    /// True when this identity is throwaway — no persistent key, so it changes
    /// every session (the default browser receiver). The peer uses this to skip
    /// remembering it in Recent/Saved devices, since it can never be reconnected
    /// to. A future browser receiver with a persisted key would send `false`.
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CancelPhase {
    WaitingForDecision,
    Transferring,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferErrorCode {
    ProtocolViolation,
    UnexpectedMessage,
    FileConflict,
    IoError,
    ChecksumMismatch,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransferStatus {
    Ok,
    Error {
        code: TransferErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferManifest {
    pub items: Vec<ManifestItem>,
}

impl TransferManifest {
    pub fn count(&self) -> usize {
        self.items.len()
    }

    pub fn total_size(&self) -> u64 {
        self.items
            .iter()
            .map(|item| match item {
                ManifestItem::File { size, .. } => *size,
            })
            .sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManifestItem {
    File { path: String, size: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub session_id: String,
    pub identity: Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offer {
    pub session_id: String,
    pub manifest: TransferManifest,
    pub collection_hash: Hash,
    /// Optional plain-text payload riding inline on the control stream.
    /// `Some(_)` marks a text-only transfer with no blobs — the manifest is
    /// empty and `collection_hash` is a placeholder.  Capped at
    /// [`INLINE_TEXT_MAX_BYTES`] by the sender; the receiver guards against
    /// [`INLINE_TEXT_HARD_MAX_BYTES`].  Older peers (protocol < 4) never set
    /// this; `serde(default)` keeps the field forward-compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_text: Option<String>,
}

/// Sent by the receiver the instant it has read the sender's [`Offer`] off the
/// wire, before any human decision. It lets the sender confirm the offer
/// actually landed — otherwise a large offer that stalls in flight leaves the
/// sender falsely showing "waiting for decision" while the receiver is still
/// stuck on "connecting". The sender waits for this ack (bounded) before
/// declaring `WaitingForDecision`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferAck {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accept {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decline {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferStarted {
    pub session_id: String,
    pub plan: TransferPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferProgressPayload {
    pub phase: TransferPhase,
    pub completed_files: u32,
    pub total_files: u32,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub active_file_id: Option<TransferFileId>,
    pub active_file_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferProgress {
    pub session_id: String,
    pub snapshot: TransferProgressPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferCompleted {
    pub session_id: String,
    pub snapshot: TransferProgressPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cancel {
    pub session_id: String,
    pub by: TransferRole,
    pub phase: CancelPhase,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobTicketMessage {
    pub session_id: String,
    pub ticket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferResult {
    pub session_id: String,
    pub status: TransferStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferAck {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageEnvelope {
    pub version: u32,
    pub role: TransferRole,
    pub kind: MessageKind,
    pub message: Value,
}

/// Messages the sender can emit on the transfer control channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SenderMessage {
    Hello(Hello),
    Offer(Offer),
    BlobTicket(BlobTicketMessage),
    Cancel(Cancel),
    TransferAck(TransferAck),
}

impl SenderMessage {
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::Hello,
            Self::Offer(_) => MessageKind::Offer,
            Self::BlobTicket(_) => MessageKind::BlobTicket,
            Self::Cancel(_) => MessageKind::Cancel,
            Self::TransferAck(_) => MessageKind::TransferAck,
        }
    }

    pub fn role(&self) -> TransferRole {
        TransferRole::Sender
    }
}

/// Messages the receiver can emit on the transfer control channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReceiverMessage {
    Hello(Hello),
    OfferAck(OfferAck),
    Accept(Accept),
    Decline(Decline),
    TransferStarted(TransferStarted),
    TransferProgress(TransferProgress),
    TransferCompleted(TransferCompleted),
    Cancel(Cancel),
    TransferResult(TransferResult),
}

impl ReceiverMessage {
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::Hello,
            Self::OfferAck(_) => MessageKind::OfferAck,
            Self::Accept(_) => MessageKind::Accept,
            Self::Decline(_) => MessageKind::Decline,
            Self::TransferStarted(_) => MessageKind::TransferStarted,
            Self::TransferProgress(_) => MessageKind::TransferProgress,
            Self::TransferCompleted(_) => MessageKind::TransferCompleted,
            Self::Cancel(_) => MessageKind::Cancel,
            Self::TransferResult(_) => MessageKind::TransferResult,
        }
    }

    pub fn role(&self) -> TransferRole {
        TransferRole::Receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{TransferPlan, TransferPlanFile};
    use iroh::SecretKey;

    #[test]
    fn sender_message_serializes_with_directional_tag() {
        let message = SenderMessage::Hello(Hello {
            version: PROTOCOL_VERSION,
            session_id: "session-1".to_owned(),
            identity: Identity {
                role: TransferRole::Sender,
                endpoint_id: SecretKey::from_bytes(&[1; 32]).public(),
                device_name: "sam-mac".to_owned(),
                device_type: DeviceType::Laptop,
                web: false,
                ephemeral: false,
            },
        });

        let json = serde_json::to_string(&message).unwrap();

        assert!(json.contains("\"type\":\"hello\""));
        assert!(json.contains("\"role\":\"sender\""));
    }

    #[test]
    fn receiver_message_serializes_expected_payloads() {
        let message = ReceiverMessage::TransferResult(TransferResult {
            session_id: "session-1".to_owned(),
            status: TransferStatus::Ok,
        });

        let json = serde_json::to_string(&message).unwrap();

        assert!(json.contains("\"type\":\"transfer_result\""));
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn offer_omits_inline_text_when_none() {
        // `skip_serializing_if` keeps the wire compact, and the absence of the
        // field on a v3-style frame must deserialize to `None` (forward compat).
        let offer = Offer {
            session_id: "s".to_owned(),
            manifest: TransferManifest { items: vec![] },
            collection_hash: [0u8; 32].into(),
            inline_text: None,
        };
        let json = serde_json::to_string(&offer).unwrap();
        assert!(!json.contains("inline_text"));
        let back: Offer = serde_json::from_str(&json).unwrap();
        assert!(back.inline_text.is_none());
    }

    #[test]
    fn offer_round_trips_inline_text() {
        let offer = Offer {
            session_id: "s".to_owned(),
            manifest: TransferManifest { items: vec![] },
            collection_hash: [0u8; 32].into(),
            inline_text: Some("héllo · wörld".to_owned()),
        };
        let json = serde_json::to_string(&offer).unwrap();
        assert!(json.contains("inline_text"));
        let back: Offer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.inline_text.as_deref(), Some("héllo · wörld"));
    }

    #[test]
    fn message_enums_cover_current_transfer() {
        let manifest = TransferManifest {
            items: vec![ManifestItem::File {
                path: "a.txt".to_owned(),
                size: 1,
            }],
        };
        let plan = TransferPlan::try_new(
            "session-1",
            vec![TransferPlanFile {
                id: 0,
                path: "a.txt".to_owned(),
                size: 1,
            }],
        )
        .unwrap();
        let snapshot = TransferProgressPayload {
            phase: TransferPhase::Transferring,
            completed_files: 0,
            total_files: 1,
            bytes_transferred: 1,
            total_bytes: 1,
            active_file_id: Some(0),
            active_file_bytes: Some(1),
        };

        let sender_messages = [
            SenderMessage::Hello(Hello {
                version: PROTOCOL_VERSION,
                session_id: "session-1".to_owned(),
                identity: Identity {
                    role: TransferRole::Sender,
                    endpoint_id: SecretKey::from_bytes(&[1; 32]).public(),
                    device_name: "sam-mac".to_owned(),
                    device_type: DeviceType::Laptop,
                    web: false,
                    ephemeral: false,
                },
            }),
            SenderMessage::Offer(Offer {
                session_id: "session-1".to_owned(),
                manifest: manifest.clone(),
                collection_hash: [0u8; 32].into(),
                inline_text: None,
            }),
            SenderMessage::BlobTicket(BlobTicketMessage {
                session_id: "session-1".to_owned(),
                ticket: "ticket".to_owned(),
            }),
            SenderMessage::Cancel(Cancel {
                session_id: "session-1".to_owned(),
                by: TransferRole::Sender,
                phase: CancelPhase::WaitingForDecision,
                reason: "cancelled".to_owned(),
            }),
            SenderMessage::TransferAck(TransferAck {
                session_id: "session-1".to_owned(),
            }),
        ];

        let receiver_messages = [
            ReceiverMessage::Hello(Hello {
                version: PROTOCOL_VERSION,
                session_id: "session-1".to_owned(),
                identity: Identity {
                    role: TransferRole::Receiver,
                    endpoint_id: SecretKey::from_bytes(&[2; 32]).public(),
                    device_name: "phone".to_owned(),
                    device_type: DeviceType::Phone,
                    web: false,
                    ephemeral: false,
                },
            }),
            ReceiverMessage::Accept(Accept {
                session_id: "session-1".to_owned(),
            }),
            ReceiverMessage::Decline(Decline {
                session_id: "session-1".to_owned(),
                reason: "not now".to_owned(),
            }),
            ReceiverMessage::TransferStarted(TransferStarted {
                session_id: "session-1".to_owned(),
                plan: plan.clone(),
            }),
            ReceiverMessage::TransferProgress(TransferProgress {
                session_id: "session-1".to_owned(),
                snapshot: snapshot.clone(),
            }),
            ReceiverMessage::TransferCompleted(TransferCompleted {
                session_id: "session-1".to_owned(),
                snapshot: snapshot.clone(),
            }),
            ReceiverMessage::Cancel(Cancel {
                session_id: "session-1".to_owned(),
                by: TransferRole::Receiver,
                phase: CancelPhase::Transferring,
                reason: "cancelled".to_owned(),
            }),
            ReceiverMessage::TransferResult(TransferResult {
                session_id: "session-1".to_owned(),
                status: TransferStatus::Ok,
            }),
        ];

        assert_eq!(sender_messages.len(), 5);
        assert_eq!(receiver_messages.len(), 8);
        let json = serde_json::to_string(&receiver_messages[3]).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(!json.contains("bytes_per_sec"));
    }
}
