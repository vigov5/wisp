use std::error::Error as StdError;
use thiserror::Error;

use crate::{
    blobs::error::BlobError,
    protocol::{error::ProtocolError, message::TransferErrorCode},
    transfer::{path::TransferPathError, types::TransferPlanError},
};

#[derive(Debug, Error)]
pub enum TransferError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Blob(#[from] BlobError),
    #[error(transparent)]
    Path(#[from] TransferPathError),
    #[error(transparent)]
    Plan(#[from] TransferPlanError),
    #[error("connection closed while {context}")]
    ConnectionClosed { context: &'static str },
    #[error("timed out while {context}")]
    Timeout { context: &'static str },
    #[error("channel closed while {context}")]
    ChannelClosed { context: &'static str },
    #[error("{context}: {source}")]
    Other {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
}

pub(crate) type Result<T> = std::result::Result<T, TransferError>;

/// True when `error` really means "the peer closed the connection on purpose".
///
/// QUIC separates an *application* close — which is what Wisp's own shutdown
/// path sends, carrying `session complete` — from a transport death (idle
/// timeout, reset, version failure). The receiver used to make no distinction:
/// cancelling a send left the other device reporting
///
///   receive failed = reading message length: connection lost:
///                    closed by peer: session complete (code 0)
///
/// for something the user did deliberately. Only the second class is a failure.
///
/// The close arrives buried: the read surfaces as an [`std::io::Error`] whose
/// *inner* error (reachable through `get_ref`, not `source`, because
/// `io::Error::source` skips the custom error and returns its source) wraps the
/// [`ConnectionError`]. So walk both links.
pub(crate) fn is_graceful_peer_close(error: &(dyn StdError + 'static)) -> bool {
    use iroh::endpoint::ConnectionError;

    let mut current = Some(error);
    while let Some(err) = current {
        // `ApplicationClosed` only: that is the peer choosing to end the
        // session. `LocallyClosed`, a timeout or a reset all mean something
        // went wrong, and must keep reporting as failures.
        if let Some(connection_error) = err.downcast_ref::<ConnectionError>() {
            return matches!(connection_error, ConnectionError::ApplicationClosed(_));
        }
        if let Some(io_error) = err.downcast_ref::<std::io::Error>() {
            if let Some(inner) = io_error.get_ref() {
                if is_graceful_peer_close(inner) {
                    return true;
                }
            }
        }
        current = err.source();
    }
    false
}

impl TransferError {
    pub(crate) fn connection_closed(context: &'static str) -> Self {
        Self::ConnectionClosed { context }
    }

    pub(crate) fn timeout(context: &'static str) -> Self {
        Self::Timeout { context }
    }

    pub(crate) fn channel_closed(context: &'static str) -> Self {
        Self::ChannelClosed { context }
    }

    pub(crate) fn other(
        context: &'static str,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Other {
            context,
            source: Box::new(source),
        }
    }

    pub(crate) fn code(&self) -> TransferErrorCode {
        match self {
            Self::Protocol(error) => error.code(),
            Self::Blob(error) => match error {
                BlobError::DuplicateTransferPath { .. } => TransferErrorCode::FileConflict,
                BlobError::StoreLoad { .. }
                | BlobError::StoreShutdown { .. }
                | BlobError::Connect { .. }
                | BlobError::Fetch { .. }
                | BlobError::StoreCollection { .. }
                | BlobError::ImportFiles { .. }
                | BlobError::JoinDownloadTask { .. } => TransferErrorCode::IoError,
            },
            Self::Path(error) => match error {
                TransferPathError::DestinationExists { .. } => TransferErrorCode::FileConflict,
                TransferPathError::Empty
                | TransferPathError::InvalidSeparator
                | TransferPathError::NotRelative
                | TransferPathError::InvalidSegment
                | TransferPathError::InvalidUtf8RootName { .. }
                | TransferPathError::InvalidUtf8PathComponent { .. }
                | TransferPathError::DestinationParentIsSymlink { .. }
                | TransferPathError::DestinationParentNotDirectory { .. }
                | TransferPathError::CheckPath { .. }
                | TransferPathError::CurrentDirectory { .. }
                | TransferPathError::OutputNotAbsolute { .. }
                | TransferPathError::SystemClockBeforeUnixEpoch { .. }
                | TransferPathError::CreateScratchDir { .. } => TransferErrorCode::IoError,
            },
            Self::Plan(_) => TransferErrorCode::IoError,
            Self::ConnectionClosed { .. }
            | Self::Timeout { .. }
            | Self::ChannelClosed { .. }
            | Self::Other { .. } => TransferErrorCode::IoError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rebuilds the exact nesting a cancelled send produces on the wire, as
    /// captured from a device:
    ///
    ///   FrameRead { context: "reading message length",
    ///               source: Custom { kind: NotConnected,
    ///                 error: ConnectionLost(ApplicationClosed(
    ///                          ApplicationClose { error_code: 0,
    ///                                             reason: b"session complete" })) } }
    ///
    /// The `ConnectionError` sits two links down and behind `io::Error`'s
    /// `get_ref`, which is why a single `downcast_ref` never found it.
    fn peer_closed_the_session() -> ProtocolError {
        use iroh::endpoint::{ApplicationClose, ConnectionError, ReadError, VarInt};

        let close = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(0),
            reason: bytes::Bytes::from_static(b"session complete"),
        });
        ProtocolError::FrameRead {
            context: "reading message length",
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                ReadError::ConnectionLost(close),
            )),
        }
    }

    #[test]
    fn a_peer_closing_the_session_is_not_a_failure() {
        assert!(is_graceful_peer_close(&peer_closed_the_session()));
    }

    #[test]
    fn a_transport_death_is_still_a_failure() {
        use iroh::endpoint::{ConnectionError, ReadError};

        let timed_out = ProtocolError::FrameRead {
            context: "reading message length",
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                ReadError::ConnectionLost(ConnectionError::TimedOut),
            )),
        };
        assert!(!is_graceful_peer_close(&timed_out));

        let plain_io = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        assert!(!is_graceful_peer_close(&plain_io));
    }
}
