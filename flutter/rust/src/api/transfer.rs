use wisp_core::transfer::{TransferPlan, TransferPlanFile};

#[derive(Debug, Clone)]
pub enum TransferPhaseData {
    Connecting,
    AwaitingAcceptance,
    Transferring,
    Finalizing,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct TransferPlanFileData {
    pub id: u32,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct TransferPlanData {
    pub session_id: String,
    /// Pseudonymous token produced by the same Rust function used in core logs.
    pub benchmark_run_id: Option<u64>,
    pub total_files: u32,
    pub total_bytes: u64,
    pub files: Vec<TransferPlanFileData>,
}

#[derive(Debug, Clone)]
pub struct TransferSnapshotData {
    pub session_id: String,
    pub phase: TransferPhaseData,
    pub total_files: u32,
    pub completed_files: u32,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub active_file_id: Option<u32>,
    pub active_file_bytes: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    pub eta_seconds: Option<u64>,
}

pub(super) fn map_plan(plan: TransferPlan) -> TransferPlanData {
    let benchmark_run_id = wisp_core::blobs::benchmark_run_id(&plan.session_id);
    TransferPlanData {
        session_id: plan.session_id,
        benchmark_run_id,
        total_files: plan.total_files,
        total_bytes: plan.total_bytes,
        files: plan.files.into_iter().map(map_plan_file).collect(),
    }
}

fn map_plan_file(file: TransferPlanFile) -> TransferPlanFileData {
    TransferPlanFileData {
        id: file.id,
        path: file.path,
        size: file.size,
    }
}

#[cfg(test)]
mod tests {
    use wisp_core::transfer::{TransferPlan, TransferPlanFile};

    use super::map_plan;

    #[test]
    fn mapped_plan_carries_the_core_correlation_token() {
        let session_id = "0123456789abcdef";
        let plan = TransferPlan::try_new(
            session_id,
            vec![TransferPlanFile {
                id: 0,
                path: "fixture.bin".to_owned(),
                size: 16,
            }],
        )
        .expect("valid transfer plan");

        let mapped = map_plan(plan);

        assert_eq!(
            mapped.benchmark_run_id,
            wisp_core::blobs::benchmark_run_id(session_id)
        );
        assert_ne!(mapped.benchmark_run_id, Some(0x0123_4567_89ab_cdef));
    }
}
