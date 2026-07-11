use crate::protocol::message::{Cancel, CancelPhase, TransferRole};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

// Plan value types moved to the wasm-clean `wisp-wire` crate; re-export at the
// historical `crate::transfer::types` paths. The transfer-engine types below
// (snapshots, outcomes, cancellation, `wait_for_cancel`) stay native-side.
pub use wisp_wire::plan::{
    TransferFileId, TransferPhase, TransferPlan, TransferPlanError, TransferPlanFile,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSnapshot {
    pub session_id: String,
    pub phase: TransferPhase,
    pub total_files: u32,
    pub completed_files: u32,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub active_file_id: Option<TransferFileId>,
    pub active_file_bytes: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    pub eta_seconds: Option<u64>,
}

impl TransferSnapshot {
    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileLifecycleState {
    Pending,
    Active,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStateUpdate {
    pub session_id: String,
    pub file_id: TransferFileId,
    pub state: FileLifecycleState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferCancellation {
    pub by: TransferRole,
    pub phase: CancelPhase,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    Completed,
    Declined { reason: String },
    Cancelled(TransferCancellation),
}

impl TransferOutcome {
    pub fn local_cancel(by: TransferRole, phase: CancelPhase) -> Self {
        let reason = match (by, phase) {
            (TransferRole::Sender, CancelPhase::WaitingForDecision) => {
                "sender cancelled before approval".to_owned()
            }
            (TransferRole::Sender, CancelPhase::Transferring) => {
                "sender cancelled transfer".to_owned()
            }
            (TransferRole::Receiver, CancelPhase::WaitingForDecision) => {
                "receiver cancelled before approval".to_owned()
            }
            (TransferRole::Receiver, CancelPhase::Transferring) => {
                "receiver cancelled transfer".to_owned()
            }
        };
        Self::Cancelled(TransferCancellation { by, phase, reason })
    }

    pub fn from_remote_cancel(
        cancel: Cancel,
        expected_session_id: &str,
    ) -> std::result::Result<Self, TransferPlanError> {
        if !expected_session_id.is_empty() && cancel.session_id != expected_session_id {
            return Err(TransferPlanError::SessionIdMismatchInCancelMessage);
        }
        Ok(Self::Cancelled(TransferCancellation {
            by: cancel.by,
            phase: cancel.phase,
            reason: cancel.reason,
        }))
    }
}

pub async fn wait_for_cancel(cancel_rx: &mut watch::Receiver<bool>) {
    if *cancel_rx.borrow() {
        return;
    }
    let _ = cancel_rx.changed().await;
}
