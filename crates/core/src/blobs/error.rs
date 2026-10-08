use std::error::Error as StdError;
use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("loading blob store at {path}")]
    StoreLoad {
        path: PathBuf,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("shutting down blob store for {context}")]
    StoreShutdown {
        context: &'static str,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("connecting to blob provider for {context}")]
    Connect {
        context: String,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("fetching blob content for {context}")]
    Fetch {
        context: String,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("storing blob collection")]
    StoreCollection {
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("duplicate transfer path in manifest: {path}")]
    DuplicateTransferPath { path: String },
    #[error("importing files from {path}")]
    ImportFiles {
        path: String,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("joining blob download task")]
    JoinDownloadTask {
        #[source]
        source: tokio::task::JoinError,
    },
}

pub(crate) type Result<T> = std::result::Result<T, BlobError>;

#[derive(Debug)]
pub(crate) struct BlobTextError(String);

impl BlobTextError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for BlobTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for BlobTextError {}

// One implementation, in `crate::util`, because the app layer formats the same
// chains for the same screens.
pub(crate) use crate::util::error_chain;

/// Flattens `error` into text that keeps the one typed fact above it classifies
/// on: whether the transport died.
///
/// Some hops have to flatten — `BlobError` boxes its source and so is not
/// Clone, and an update sent over a channel cannot carry it. Flattening used to
/// hand the layers above a bare string, and a string classifies as nothing, so
/// a receive killed by a reset connection showed the user three lines of
/// context instead of "Connection lost". The chain still becomes text; the io
/// kind rides along in a real [`io::Error`], which is exactly what the mappers
/// look for. Without one, `Other` leaves the behaviour as it was: the text is
/// all there is to show.
pub(crate) fn flatten_keeping_io_kind(
    error: &(dyn StdError + 'static),
    text: impl Into<String>,
) -> std::io::Error {
    std::io::Error::new(
        crate::util::io_kind_in_chain(error).unwrap_or(std::io::ErrorKind::Other),
        text.into(),
    )
}

impl BlobError {
    pub(crate) fn store_load(path: PathBuf, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::StoreLoad {
            path,
            source: Box::new(source),
        }
    }

    pub(crate) fn store_shutdown(
        context: &'static str,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::StoreShutdown {
            context,
            source: Box::new(source),
        }
    }

    pub(crate) fn connect(
        context: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Connect {
            context: context.into(),
            source: Box::new(source),
        }
    }

    pub(crate) fn fetch(
        context: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Fetch {
            context: context.into(),
            source: Box::new(source),
        }
    }

    pub(crate) fn store_collection(source: impl StdError + Send + Sync + 'static) -> Self {
        Self::StoreCollection {
            source: Box::new(source),
        }
    }

    pub(crate) fn duplicate_transfer_path(path: impl Into<String>) -> Self {
        Self::DuplicateTransferPath { path: path.into() }
    }

    pub(crate) fn import_files(
        path: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::ImportFiles {
            path: path.into(),
            source: Box::new(source),
        }
    }

    pub(crate) fn join_download_task(source: tokio::task::JoinError) -> Self {
        Self::JoinDownloadTask { source }
    }
}
