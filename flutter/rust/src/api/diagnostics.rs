//! FFI surface for the Connection Test feature.
//!
//! Streams progressively-resolving check results to Dart over a
//! `StreamSink<DiagnosticsCheckData>` so the UI can tick each row off as it
//! resolves.  Permissions are probed on the Dart side; this surface covers
//! the network-side checks.

use wisp_app::diagnostics::{self as app_diag};

use super::RUNTIME;
use crate::frb_generated::StreamSink;

#[derive(Debug, Clone)]
pub enum DiagnosticsCheckStatus {
    Running,
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Debug, Clone)]
pub enum DiagnosticsCheckGroup {
    Network,
    Rendezvous,
    Lan,
    P2p,
    Permissions,
    Local,
}

#[derive(Debug, Clone)]
pub enum DiagnosticsActionKind {
    OpenAppSettings,
    OpenUrl,
    Retry,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsActionData {
    pub label: String,
    pub kind: DiagnosticsActionKind,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsCheckData {
    pub id: String,
    pub group: DiagnosticsCheckGroup,
    pub status: DiagnosticsCheckStatus,
    pub label: String,
    pub detail: String,
    pub hint: Option<String>,
    pub action: Option<DiagnosticsActionData>,
}

/// Runs the Rust-side connection-test checks and streams each result as
/// it resolves.  The stream closes when all checks have been emitted.
pub fn run_connection_test(
    server_url: Option<String>,
    download_root: String,
    sink: StreamSink<DiagnosticsCheckData>,
) {
    let endpoint = super::receiver::current_service_endpoint();
    RUNTIME.spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<app_diag::CheckResult>();
        let sink_clone = sink.clone();
        let relay = RUNTIME.spawn(async move {
            while let Some(result) = rx.recv().await {
                if sink_clone.add(map_check_result(result)).is_err() {
                    break;
                }
            }
        });
        app_diag::run_connection_test(server_url, download_root, endpoint, tx).await;
        let _ = relay.await;
    });
}

fn map_check_result(result: app_diag::CheckResult) -> DiagnosticsCheckData {
    DiagnosticsCheckData {
        id: result.id,
        group: map_group(result.group),
        status: map_status(result.status),
        label: result.label,
        detail: result.detail,
        hint: result.hint,
        action: result.action.map(map_action),
    }
}

fn map_status(status: app_diag::CheckStatus) -> DiagnosticsCheckStatus {
    match status {
        app_diag::CheckStatus::Running => DiagnosticsCheckStatus::Running,
        app_diag::CheckStatus::Pass => DiagnosticsCheckStatus::Pass,
        app_diag::CheckStatus::Warn => DiagnosticsCheckStatus::Warn,
        app_diag::CheckStatus::Fail => DiagnosticsCheckStatus::Fail,
        app_diag::CheckStatus::Skipped => DiagnosticsCheckStatus::Skipped,
    }
}

fn map_group(group: app_diag::CheckGroup) -> DiagnosticsCheckGroup {
    match group {
        app_diag::CheckGroup::Network => DiagnosticsCheckGroup::Network,
        app_diag::CheckGroup::Rendezvous => DiagnosticsCheckGroup::Rendezvous,
        app_diag::CheckGroup::Lan => DiagnosticsCheckGroup::Lan,
        app_diag::CheckGroup::P2p => DiagnosticsCheckGroup::P2p,
        app_diag::CheckGroup::Permissions => DiagnosticsCheckGroup::Permissions,
        app_diag::CheckGroup::Local => DiagnosticsCheckGroup::Local,
    }
}

fn map_action(action: app_diag::CheckAction) -> DiagnosticsActionData {
    DiagnosticsActionData {
        label: action.label,
        kind: match action.kind {
            app_diag::CheckActionKind::OpenAppSettings => DiagnosticsActionKind::OpenAppSettings,
            app_diag::CheckActionKind::OpenUrl => DiagnosticsActionKind::OpenUrl,
            app_diag::CheckActionKind::Retry => DiagnosticsActionKind::Retry,
        },
        target: action.target,
    }
}
