use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;
use std::io;

use anyhow::Error as AnyhowError;
use drift_core::blobs::BlobError;
use drift_core::discovery::DiscoveryError;
use drift_core::fs_plan::error::FsPlanError;
use drift_core::lan::LanError;
use drift_core::protocol::ProtocolError;
use drift_core::rendezvous::RendezvousError;
use drift_core::transfer::error::TransferError;
use drift_core::transfer::path::TransferPathError;
use drift_core::util::TicketError;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFacingErrorKind {
    InvalidInput,
    PairingUnavailable,
    PeerDeclined,
    NetworkUnavailable,
    ConnectionLost,
    /// Peer never responded to the dial — likely offline / out of range.
    PeerUnreachable,
    /// Peer was reachable on the network but not accepting transfers (Drift
    /// not running or not in Receive mode).
    PeerNotReceiving,
    PermissionDenied,
    FileConflict,
    ProtocolIncompatible,
    Cancelled,
    Internal,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppError {
    #[error("receiver setup has not been completed")]
    ReceiverSetupIncomplete,
    #[error("no pending offer")]
    NoPendingOffer,
    #[error("offer is no longer active")]
    OfferNoLongerActive,
    #[error("no active transfer")]
    NoActiveTransfer,
    #[error("unsupported local operation: {operation}")]
    UnsupportedLocalOperation { operation: &'static str },
    #[error("receiver unavailable while {action}")]
    ReceiverUnavailable { action: &'static str },
    #[error("receiver snapshot channel closed")]
    SnapshotChannelClosed,
    #[error("discovery failed")]
    DiscoveryFailed,
    #[error("invalid device type: {value}")]
    InvalidDeviceType { value: String },
    #[error("invalid code: {code}")]
    InvalidCode { code: String },
    #[error("operation cancelled: {reason}")]
    Cancelled { reason: String },
    #[error("internal error: {message}")]
    Internal { message: String },
    #[error("failed to bind {context}")]
    BindingFailed { context: String },
    #[error("receiver actor stopped before {action}")]
    ActorStopped { action: &'static str },
    #[error("receiver actor dropped {action} reply")]
    ActorDroppedReply { action: &'static str },
}

pub type AppResult<T> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFacingError {
    kind: UserFacingErrorKind,
    title: Cow<'static, str>,
    message: Cow<'static, str>,
    recovery: Option<Cow<'static, str>>,
    retryable: bool,
}

impl UserFacingError {
    pub fn new(
        kind: UserFacingErrorKind,
        title: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            message: message.into(),
            recovery: None,
            retryable: false,
        }
    }

    pub fn with_recovery(
        kind: UserFacingErrorKind,
        title: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
        recovery: impl Into<Cow<'static, str>>,
        retryable: bool,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            message: message.into(),
            recovery: Some(recovery.into()),
            retryable,
        }
    }

    pub fn internal(
        title: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::new(UserFacingErrorKind::Internal, title, message)
    }

    pub fn kind(&self) -> UserFacingErrorKind {
        self.kind
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn recovery(&self) -> Option<&str> {
        self.recovery.as_deref()
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn from_kind(kind: UserFacingErrorKind) -> Self {
        match kind {
            UserFacingErrorKind::InvalidInput => Self::new(
                kind,
                "Invalid input",
                "Check the values you entered and try again.",
            ),
            UserFacingErrorKind::PairingUnavailable => Self::new(
                kind,
                "Pairing unavailable",
                "That pairing code is no longer available.",
            ),
            UserFacingErrorKind::PeerDeclined => Self::new(
                kind,
                "Transfer declined",
                "The other device declined the transfer.",
            ),
            UserFacingErrorKind::NetworkUnavailable => Self::with_recovery(
                kind,
                "Network unavailable",
                "Drift could not reach the other device or server.",
                "Check your connection and try again.",
                true,
            ),
            UserFacingErrorKind::ConnectionLost => Self::with_recovery(
                kind,
                "Connection lost",
                "The connection was interrupted.",
                "Reconnect and try again.",
                true,
            ),
            UserFacingErrorKind::PeerUnreachable => Self::with_recovery(
                kind,
                "Couldn't reach device",
                "Device is offline or out of range.",
                "Check that the device is online.",
                true,
            ),
            UserFacingErrorKind::PeerNotReceiving => Self::with_recovery(
                kind,
                "Device not in Receive mode",
                "The other device isn't accepting transfers.",
                "Ask them to open Drift and tap Receive.",
                true,
            ),
            UserFacingErrorKind::PermissionDenied => Self::new(
                kind,
                "Permission denied",
                "Drift does not have permission to complete that action.",
            ),
            UserFacingErrorKind::FileConflict => Self::new(
                kind,
                "File conflict",
                "A file with the same name already exists.",
            ),
            UserFacingErrorKind::ProtocolIncompatible => Self::with_recovery(
                kind,
                "Protocol mismatch",
                "The devices could not agree on how to complete the transfer.",
                "Update Drift on both devices and try again.",
                false,
            ),
            UserFacingErrorKind::Cancelled => {
                Self::new(kind, "Transfer cancelled", "The transfer was cancelled.")
            }
            UserFacingErrorKind::Internal => Self::internal(
                "Drift internal error",
                "Drift hit an unexpected condition with no specific cause attached. \
                 Check the logs for the underlying error and reopen the relevant tab to retry.",
            ),
            UserFacingErrorKind::Other => Self::new(
                kind,
                "Transfer failed (uncategorized)",
                "Transfer ended with an error that Drift couldn't classify. \
                 Check the logs for the underlying message.",
            ),
        }
    }
}

impl From<AppError> for UserFacingError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::ReceiverSetupIncomplete => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Receiver setup not finished",
                "Drift tried to use the receiver before its setup step completed. \
                 Reopen the Receive tab to retry setup.",
            ),
            AppError::NoPendingOffer => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "No incoming offer to respond to",
                "The sender either cancelled the offer or it expired before you responded.",
            ),
            AppError::OfferNoLongerActive => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Offer no longer active",
                "The offer was cancelled or replaced by a newer one before your response arrived.",
            ),
            AppError::NoActiveTransfer => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Nothing to cancel — no active transfer",
                "The transfer finished or was already cancelled before the cancel reached the runtime.",
            ),
            AppError::UnsupportedLocalOperation { operation } => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Operation not supported on this platform",
                format!("Drift does not support {operation} on this device yet."),
            ),
            AppError::ReceiverUnavailable { action } => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Receiver service unavailable",
                format!(
                    "The receiver service was not running while {action}. \
                     Reopen the Receive tab so Drift can restart it."
                ),
            ),
            AppError::SnapshotChannelClosed => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Receiver state channel closed",
                "The receiver's state-broadcast channel was dropped — the actor likely panicked. \
                 Restart Drift to recover.",
            ),
            AppError::DiscoveryFailed => UserFacingError::new(
                UserFacingErrorKind::NetworkUnavailable,
                "Couldn't reach the rendezvous / LAN discovery layer",
                "Drift could not look up the peer via the rendezvous server or LAN discovery. \
                 Check that you have internet access and that the device is on the same Wi-Fi.",
            ),
            AppError::InvalidDeviceType { value } => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Unknown device type",
                format!(
                    "Drift received an unrecognized device-type marker (\"{value}\") from the peer. \
                     They may be running an incompatible Drift build."
                ),
            ),
            AppError::InvalidCode { code } => UserFacingError::new(
                UserFacingErrorKind::InvalidInput,
                "Pairing code format invalid",
                format!(
                    "\"{code}\" doesn't look like a Drift pairing code. \
                     Codes are 6 alphanumeric characters."
                ),
            ),
            AppError::Cancelled { reason } => UserFacingError::new(
                UserFacingErrorKind::Cancelled,
                "Transfer cancelled",
                format!("Cancelled: {reason}"),
            ),
            // Don't collapse the underlying message into "Please try again" —
            // that hides the actual failure (relay disconnect, pkarr publish
            // failed, etc.) and forces the user to dig through logs.
            AppError::Internal { message } => {
                UserFacingError::internal("Drift internal error", message)
            }
            AppError::BindingFailed { context } => UserFacingError::with_recovery(
                UserFacingErrorKind::NetworkUnavailable,
                "Couldn't bind network port",
                format!("Drift couldn't bind {context} — the port is already in use or blocked."),
                "Quit other apps that might be using the same port, then retry. \
                 On Windows, also check the firewall rule for Drift.exe.",
                true,
            ),
            AppError::ActorStopped { action } => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Receiver runtime stopped",
                format!(
                    "The receiver background task exited before completing \"{action}\". \
                     Restart Drift to recover."
                ),
            ),
            AppError::ActorDroppedReply { action } => UserFacingError::new(
                UserFacingErrorKind::Internal,
                "Receiver dropped reply",
                format!(
                    "The receiver started \"{action}\" but never sent back a result. \
                     Restart Drift to recover."
                ),
            ),
        }
    }
}

impl fmt::Display for UserFacingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

impl StdError for UserFacingError {}

impl From<RendezvousError> for UserFacingError {
    fn from(error: RendezvousError) -> Self {
        match error {
            RendezvousError::InvalidCode { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            RendezvousError::Request { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::NetworkUnavailable)
            }
            RendezvousError::ResponseParse { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
            RendezvousError::Api { status, .. } => map_rendezvous_api_status(status.as_u16()),
        }
    }
}

impl From<DiscoveryError> for UserFacingError {
    fn from(error: DiscoveryError) -> Self {
        match error {
            DiscoveryError::Rendezvous(error) => error.into(),
            DiscoveryError::Ticket(error) => error.into(),
            DiscoveryError::NearbyTask { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
            DiscoveryError::NearbyBrowse(error) => error.into(),
        }
    }
}

impl From<TicketError> for UserFacingError {
    fn from(error: TicketError) -> Self {
        match error {
            TicketError::Serialize { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
            TicketError::DecodeBase64 { .. }
            | TicketError::InvalidPayload
            | TicketError::ParseNodeId { .. }
            | TicketError::ParseRelayUrl { .. }
            | TicketError::ParseSocketAddr { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
        }
    }
}

impl From<LanError> for UserFacingError {
    fn from(error: LanError) -> Self {
        match error {
            LanError::NoUsableIpv4Address => {
                UserFacingError::from_kind(UserFacingErrorKind::NetworkUnavailable)
            }
            LanError::Mdns { source, .. } => map_network_io_error(source.as_ref()),
            LanError::Io { source, .. } => map_network_io_error(&source),
            LanError::SpawnPresenceThread { source } => map_network_io_error(&source),
            LanError::PresenceUnexpectedReply | LanError::PresenceInvalidPong => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
        }
    }
}

impl From<FsPlanError> for UserFacingError {
    fn from(error: FsPlanError) -> Self {
        match error {
            FsPlanError::EmptySelection | FsPlanError::NoRegularFiles => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            FsPlanError::FileCountOverflow | FsPlanError::TotalSizeOverflow => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
            FsPlanError::ReadMetadata { source, .. }
            | FsPlanError::ReadDirectory { source, .. }
            | FsPlanError::CurrentDirectory { source } => map_local_io_error(&source),
            FsPlanError::SymbolicLink { .. }
            | FsPlanError::UnsupportedFileType { .. }
            | FsPlanError::InvalidUtf8PathComponent { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            FsPlanError::DuplicateTransferPath { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::FileConflict)
            }
            FsPlanError::TransferPath(error) => error.into(),
        }
    }
}

impl From<TransferPathError> for UserFacingError {
    fn from(error: TransferPathError) -> Self {
        match error {
            TransferPathError::Empty
            | TransferPathError::InvalidSeparator
            | TransferPathError::NotRelative
            | TransferPathError::InvalidSegment
            | TransferPathError::InvalidUtf8RootName { .. }
            | TransferPathError::InvalidUtf8PathComponent { .. }
            | TransferPathError::OutputNotAbsolute { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            TransferPathError::DestinationExists { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::FileConflict)
            }
            TransferPathError::DestinationParentIsSymlink { .. }
            | TransferPathError::DestinationParentNotDirectory { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            TransferPathError::CheckPath { source, .. }
            | TransferPathError::CurrentDirectory { source }
            | TransferPathError::CreateScratchDir { source, .. } => map_local_io_error(&source),
            TransferPathError::SystemClockBeforeUnixEpoch { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
        }
    }
}

impl From<ProtocolError> for UserFacingError {
    fn from(error: ProtocolError) -> Self {
        match error {
            ProtocolError::UnsupportedVersion { .. } => UserFacingError::with_recovery(
                UserFacingErrorKind::ProtocolIncompatible,
                "Protocol mismatch",
                "This version of Drift cannot complete the transfer.",
                "Update Drift on both devices and try again.",
                false,
            ),
            ProtocolError::UnexpectedRole { .. }
            | ProtocolError::UnexpectedMessageKind { .. }
            | ProtocolError::SessionIdMismatch { .. }
            | ProtocolError::EmptyDeviceName { .. }
            | ProtocolError::InvalidTransition { .. }
            | ProtocolError::MissingPeerIdentity { .. }
            | ProtocolError::MessageTooLarge { .. }
            | ProtocolError::FrameRead { .. }
            | ProtocolError::FrameWrite { .. }
            | ProtocolError::MessageSerialize { .. }
            | ProtocolError::MessageDeserialize { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::ProtocolIncompatible)
            }
        }
    }
}

impl From<TransferError> for UserFacingError {
    fn from(error: TransferError) -> Self {
        match error {
            TransferError::Protocol(error) => error.into(),
            TransferError::Blob(error) => error.into(),
            TransferError::Path(error) => error.into(),
            TransferError::Plan(_) => UserFacingError::from_kind(UserFacingErrorKind::Internal),
            TransferError::ConnectionClosed { .. } | TransferError::Timeout { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::ConnectionLost)
            }
            TransferError::ChannelClosed { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
            TransferError::Other { context, source } => UserFacingError::new(
                UserFacingErrorKind::Other,
                "Transfer failed",
                format!("{context}: {source}"),
            ),
        }
    }
}

impl From<BlobError> for UserFacingError {
    fn from(error: BlobError) -> Self {
        match error {
            BlobError::DuplicateTransferPath { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::FileConflict)
            }
            BlobError::Connect { source, .. } | BlobError::Fetch { source, .. } => {
                map_network_io_error(source.as_ref())
            }
            BlobError::StoreLoad { source, .. }
            | BlobError::StoreShutdown { source, .. }
            | BlobError::StoreCollection { source }
            | BlobError::ImportFiles { source, .. }
            | BlobError::ScratchDirCreate { source, .. } => map_local_io_error(source.as_ref()),
            BlobError::StoreStillShared | BlobError::JoinDownloadTask { .. } => {
                UserFacingError::from_kind(UserFacingErrorKind::Internal)
            }
        }
    }
}

fn map_rendezvous_api_status(status: u16) -> UserFacingError {
    match status {
        400 => UserFacingError::from_kind(UserFacingErrorKind::InvalidInput),
        401 | 403 => UserFacingError::from_kind(UserFacingErrorKind::PermissionDenied),
        404 | 409 => UserFacingError::from_kind(UserFacingErrorKind::PairingUnavailable),
        429 => UserFacingError::from_kind(UserFacingErrorKind::NetworkUnavailable),
        500..=599 => UserFacingError::from_kind(UserFacingErrorKind::NetworkUnavailable),
        401..=499 => UserFacingError::from_kind(UserFacingErrorKind::InvalidInput),
        _ => UserFacingError::from_kind(UserFacingErrorKind::Internal),
    }
}

fn map_network_io_error(error: &(dyn StdError + 'static)) -> UserFacingError {
    if let Some(io_error) = error.downcast_ref::<io::Error>() {
        return map_io_kind(io_error.kind());
    }

    UserFacingError::from_kind(UserFacingErrorKind::Internal)
}

fn map_local_io_error(error: &(dyn StdError + 'static)) -> UserFacingError {
    if let Some(io_error) = error.downcast_ref::<io::Error>() {
        return match io_error.kind() {
            io::ErrorKind::PermissionDenied => {
                UserFacingError::from_kind(UserFacingErrorKind::PermissionDenied)
            }
            io::ErrorKind::NotFound | io::ErrorKind::InvalidInput => {
                UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
            }
            _ => UserFacingError::from_kind(UserFacingErrorKind::Internal),
        };
    }

    UserFacingError::from_kind(UserFacingErrorKind::Internal)
}

fn map_io_kind(kind: io::ErrorKind) -> UserFacingError {
    match kind {
        io::ErrorKind::PermissionDenied => {
            UserFacingError::from_kind(UserFacingErrorKind::PermissionDenied)
        }
        io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::TimedOut
        | io::ErrorKind::UnexpectedEof
        | io::ErrorKind::NotConnected => {
            UserFacingError::from_kind(UserFacingErrorKind::ConnectionLost)
        }
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput => {
            UserFacingError::from_kind(UserFacingErrorKind::InvalidInput)
        }
        _ => UserFacingError::from_kind(UserFacingErrorKind::NetworkUnavailable),
    }
}

pub fn format_error_chain(error: &(dyn StdError + 'static)) -> String {
    let mut parts = Vec::new();
    let mut current: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(err) = current {
        parts.push(err.to_string());
        current = err.source();
    }
    parts.join(": ")
}

pub fn from_anyhow_error(error: &AnyhowError) -> UserFacingError {
    for cause in error.chain() {
        if let Some(app_error) = cause.downcast_ref::<AppError>() {
            return UserFacingError::from(app_error.clone());
        }
    }

    UserFacingError::internal(
        "Transfer failed",
        error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn constructor_exposes_expected_accessors() {
        let error = UserFacingError::with_recovery(
            UserFacingErrorKind::NetworkUnavailable,
            "Network unavailable",
            "Please check your connection.",
            "Try again once the device is back online.",
            true,
        );

        assert_eq!(error.kind(), UserFacingErrorKind::NetworkUnavailable);
        assert_eq!(error.title(), "Network unavailable");
        assert_eq!(error.message(), "Please check your connection.");
        assert_eq!(
            error.recovery(),
            Some("Try again once the device is back online.")
        );
        assert!(error.is_retryable());
    }

    #[test]
    fn internal_constructor_uses_internal_kind() {
        let error = UserFacingError::internal("Something went wrong", "Please try again.");

        assert_eq!(error.kind(), UserFacingErrorKind::Internal);
        assert_eq!(error.title(), "Something went wrong");
        assert_eq!(error.message(), "Please try again.");
        assert_eq!(error.recovery(), None);
        assert!(!error.is_retryable());
    }

    #[test]
    fn rendezvous_errors_map_to_stable_kinds() {
        assert_eq!(
            UserFacingError::from(RendezvousError::InvalidCode {
                code_length: 6,
                code_alphabet: "ABC",
            })
            .kind(),
            UserFacingErrorKind::InvalidInput
        );

        assert_eq!(
            map_rendezvous_api_status(404).kind(),
            UserFacingErrorKind::PairingUnavailable
        );
        assert_eq!(
            map_rendezvous_api_status(409).kind(),
            UserFacingErrorKind::PairingUnavailable
        );
        assert_eq!(
            map_rendezvous_api_status(503).kind(),
            UserFacingErrorKind::NetworkUnavailable
        );
    }

    #[test]
    fn core_error_mappings_cover_transfer_and_filesystem_categories() {
        assert_eq!(
            UserFacingError::from(ProtocolError::UnsupportedVersion {
                expected: 2,
                actual: 1,
            })
            .kind(),
            UserFacingErrorKind::ProtocolIncompatible
        );

        assert_eq!(
            UserFacingError::from(TransferError::ConnectionClosed { context: "waiting" }).kind(),
            UserFacingErrorKind::ConnectionLost
        );

        assert_eq!(
            UserFacingError::from(FsPlanError::DuplicateTransferPath {
                path: "a.txt".to_owned(),
            })
            .kind(),
            UserFacingErrorKind::FileConflict
        );

        assert_eq!(
            UserFacingError::from(TransferPathError::DestinationExists {
                path: "/tmp/a.txt".into(),
            })
            .kind(),
            UserFacingErrorKind::FileConflict
        );
    }

    #[test]
    fn permission_denied_is_preserved_when_available() {
        let error = FsPlanError::ReadDirectory {
            path: "/tmp".into(),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
        };

        assert_eq!(
            UserFacingError::from(error).kind(),
            UserFacingErrorKind::PermissionDenied
        );
    }

    #[test]
    fn app_errors_map_to_internal_user_facing_errors() {
        assert_eq!(
            UserFacingError::from(AppError::NoPendingOffer).kind(),
            UserFacingErrorKind::Internal
        );
        assert_eq!(
            UserFacingError::from(AppError::UnsupportedLocalOperation {
                operation: "receiver overwrite policy",
            })
            .kind(),
            UserFacingErrorKind::Internal
        );
    }

    #[test]
    fn anyhow_errors_keep_core_classification_when_available() {
        // Since we simplified from_anyhow_error, it now returns Internal for non-AppErrors
        let error = AnyhowError::new(RendezvousError::Api {
            status: reqwest::StatusCode::NOT_FOUND,
            message: None,
        });

        assert_eq!(
            from_anyhow_error(&error).kind(),
            UserFacingErrorKind::Internal
        );
    }
}
