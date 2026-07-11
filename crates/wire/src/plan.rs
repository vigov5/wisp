//! Transfer-plan value types carried by the control-protocol messages.
//!
//! These are the pure, wasm-clean subset of what used to live in
//! `wisp_core::transfer::types`: the plan / file-id / phase types the wire schema
//! references. The transfer-engine types (`TransferOutcome`, `TransferSnapshot`,
//! `wait_for_cancel`, …) stay native-side in `wisp-core`.

use std::error::Error as StdError;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::message::{ManifestItem, TransferManifest};

pub type TransferFileId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferPlanError {
    TooManyFiles,
    SparseFileIds { expected: u32, got: u32 },
    TotalSizeOverflow,
    SessionIdMismatchInCancelMessage,
}

impl fmt::Display for TransferPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyFiles => f.write_str("too many files"),
            Self::SparseFileIds { expected, got } => write!(
                f,
                "transfer file ids must be contiguous and ordered from 0..n-1 (expected {}, got {})",
                expected, got
            ),
            Self::TotalSizeOverflow => f.write_str("total transfer size exceeds u64"),
            Self::SessionIdMismatchInCancelMessage => {
                f.write_str("session id mismatch in cancel message")
            }
        }
    }
}

impl StdError for TransferPlanError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPlanFile {
    pub id: TransferFileId,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPlan {
    pub session_id: String,
    pub total_files: u32,
    pub total_bytes: u64,
    pub files: Vec<TransferPlanFile>,
}

impl TransferPlan {
    pub fn try_new(
        session_id: impl Into<String>,
        files: Vec<TransferPlanFile>,
    ) -> std::result::Result<Self, TransferPlanError> {
        let session_id = session_id.into();
        for (expected_id, file) in files.iter().enumerate() {
            let expected_id =
                u32::try_from(expected_id).map_err(|_| TransferPlanError::TooManyFiles)?;
            if file.id != expected_id {
                return Err(TransferPlanError::SparseFileIds {
                    expected: expected_id,
                    got: file.id,
                });
            }
        }
        let total_files =
            u32::try_from(files.len()).map_err(|_| TransferPlanError::TooManyFiles)?;
        let total_bytes = files.iter().try_fold(0_u64, |acc, file| {
            acc.checked_add(file.size)
                .ok_or(TransferPlanError::TotalSizeOverflow)
        })?;
        Ok(Self {
            session_id,
            total_files,
            total_bytes,
            files,
        })
    }

    pub fn from_manifest(
        session_id: impl Into<String>,
        manifest: &TransferManifest,
    ) -> std::result::Result<Self, TransferPlanError> {
        let files = manifest
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| match item {
                ManifestItem::File { path, size } => Ok(TransferPlanFile {
                    id: u32::try_from(index).map_err(|_| TransferPlanError::TooManyFiles)?,
                    path: path.clone(),
                    size: *size,
                }),
            })
            .collect::<std::result::Result<Vec<_>, TransferPlanError>>()?;
        Self::try_new(session_id, files)
    }

    pub fn file(&self, id: TransferFileId) -> Option<&TransferPlanFile> {
        self.files.iter().find(|file| file.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    Connecting,
    AwaitingAcceptance,
    Transferring,
    Finalizing,
    Completed,
    Cancelled,
    Failed,
}

impl TransferPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_plan_rejects_sparse_ids() {
        let err = TransferPlan::try_new(
            "session-1",
            vec![
                TransferPlanFile {
                    id: 0,
                    path: "a.txt".to_owned(),
                    size: 1,
                },
                TransferPlanFile {
                    id: 2,
                    path: "b.txt".to_owned(),
                    size: 1,
                },
            ],
        )
        .unwrap_err();

        assert!(err.to_string().contains("contiguous"));
    }

    #[test]
    fn transfer_plan_accepts_contiguous_ids() {
        let plan = TransferPlan::try_new(
            "session-1",
            vec![
                TransferPlanFile {
                    id: 0,
                    path: "a.txt".to_owned(),
                    size: 1,
                },
                TransferPlanFile {
                    id: 1,
                    path: "b.txt".to_owned(),
                    size: 2,
                },
            ],
        )
        .unwrap();

        assert_eq!(plan.total_files, 2);
        assert_eq!(plan.total_bytes, 3);
        assert_eq!(plan.file(1).map(|file| file.path.as_str()), Some("b.txt"));
    }
}
