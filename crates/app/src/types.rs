use std::path::PathBuf;

use crate::error::UserFacingError;

use iroh::SecretKey;
pub use wisp_core::fs_plan::ConflictPolicy;
pub use wisp_core::transfer::{TransferPlan, TransferSnapshot};
pub use wisp_core::util::{ConnectionPath, ConnectionPathKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPhase {
    Connecting,
    WaitingForDecision,
    Accepted,
    Declined,
    Sending,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendConfig {
    pub device_name: String,
    pub device_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendEvent {
    pub phase: SendPhase,
    pub destination_label: String,
    pub status_message: String,
    pub item_count: u64,
    pub total_size: u64,
    pub bytes_sent: u64,
    pub plan: Option<TransferPlan>,
    pub snapshot: Option<TransferSnapshot>,
    pub remote_device_type: Option<String>,
    pub remote_endpoint_id: Option<String>,
    /// Re-serialized ticket of the resolved peer addr.  For code-based sends
    /// the ticket is owned by the rendezvous server, not the original
    /// request, so we surface it here once `claim_peer` returns — the Dart
    /// saved-devices repo persists it as `lastTicket` for fast-reconnect.
    /// `None` until the destination resolves, or when re-encoding fails.
    pub remote_ticket: Option<String>,
    pub connection_path: Option<ConnectionPath>,
    pub error: Option<UserFacingError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyReceiver {
    pub fullname: String,
    pub label: String,
    pub device_type: String,
    pub code: String,
    pub ticket: String,
    /// Receiver's pubkey (base32 EndpointId), decoded from the advertised
    /// ticket. Empty when the ticket couldn't be parsed (bad input).
    pub endpoint_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverRegistration {
    pub code: String,
    pub expires_at: String,
}

/// Snapshot used by the QR pairing screen: a ticket built from currently
/// known addresses (no `online()` wait — works offline-LAN) and the
/// LAN-routable direct socket addresses for user confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPairingInfo {
    pub ticket: String,
    pub lan_ips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingCodeState {
    Unavailable,
    Active(ReceiverRegistration),
    /// Server says the code is no longer claimable (likely the previous
    /// sender already claimed it) but the receiver couldn't immediately
    /// register a fresh one — either the rendezvous call errored or we're
    /// still observing the previous registration.  UI should keep showing
    /// the stale code grey'd out plus a "may have been used, tap Refresh"
    /// hint so the user has an explicit recovery action.
    Stale(ReceiverRegistration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverOfferPhase {
    Connecting,
    OfferReady,
    Receiving,
    Completed,
    Cancelled,
    Failed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverOfferFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverOfferEvent {
    pub phase: ReceiverOfferPhase,
    pub sender_name: String,
    pub sender_device_type: String,
    pub destination_label: String,
    pub save_root_label: String,
    pub status_message: String,
    pub item_count: u64,
    pub total_size_bytes: u64,
    pub bytes_received: u64,
    pub plan: Option<TransferPlan>,
    pub snapshot: Option<TransferSnapshot>,
    pub connection_path: Option<ConnectionPath>,
    pub sender_endpoint_id: Option<String>,
    pub sender_ticket: Option<String>,
    pub total_size_label: String,
    pub files: Vec<ReceiverOfferFile>,
    pub error: Option<UserFacingError>,
}

#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    pub device_name: String,
    pub device_type: String,
    pub download_root: PathBuf,
    pub conflict_policy: ConflictPolicy,
    pub secret_key: SecretKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionPreview {
    pub items: Vec<SelectionItem>,
    pub file_count: u64,
    pub total_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionChange {
    pub paths: Vec<PathBuf>,
    pub added_count: u64,
    pub removed_count: u64,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionItem {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub file_count: u64,
    pub total_size: u64,
}
