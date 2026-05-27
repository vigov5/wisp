//! Self-diagnose checklist for the Flutter "Connection Test" feature.
//!
//! Emits per-check results progressively over an `mpsc::UnboundedSender`
//! so the UI can tick each row off as it resolves rather than waiting on
//! the whole run.

use iroh::Endpoint;
use tokio::sync::mpsc::UnboundedSender;

mod lan;
mod local;
mod network;
mod p2p;
mod rendezvous;
mod vpn;

pub use local::{create_firewall_rule_for_current_exe, firewall_inbound_warning};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Running,
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckGroup {
    Network,
    Rendezvous,
    Lan,
    P2p,
    Permissions,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckActionKind {
    OpenAppSettings,
    OpenUrl,
    Retry,
    CreateFirewallRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckAction {
    pub label: String,
    pub kind: CheckActionKind,
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub id: String,
    pub group: CheckGroup,
    pub status: CheckStatus,
    pub label: String,
    pub detail: String,
    pub hint: Option<String>,
    pub action: Option<CheckAction>,
}

impl CheckResult {
    pub(crate) fn skipped(id: &str, group: CheckGroup, label: &str, reason: &str) -> Self {
        Self {
            id: id.to_owned(),
            group,
            status: CheckStatus::Skipped,
            label: label.to_owned(),
            detail: reason.to_owned(),
            hint: None,
            action: None,
        }
    }
}

/// Runs the connection-test checks that live on the Rust side.
///
/// `server_url` should be the configured rendezvous URL (or `None` to fall
/// back to the bundled default). `download_root` is the configured receive
/// folder (raw path or Android SAF URI).  Permissions are probed on the
/// Dart side; LAN self-scan and loopback iroh checks land in Steps 5 & 6.
pub async fn run_connection_test(
    server_url: Option<String>,
    download_root: String,
    endpoint: Option<Endpoint>,
    tx: UnboundedSender<CheckResult>,
) {
    let server_url = server_url
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| wisp_core::rendezvous::DEFAULT_RENDEZVOUS_URL.to_owned());

    let internet = network::check_internet().await;
    let internet_ok = matches!(internet.status, CheckStatus::Pass);
    let _ = tx.send(internet);

    if internet_ok {
        let _ = tx.send(rendezvous::check_health(&server_url).await);
    } else {
        let _ = tx.send(CheckResult::skipped(
            "rendezvous.health",
            CheckGroup::Rendezvous,
            "Server /healthz",
            "Skipped — internet unreachable.",
        ));
    }

    let _ = tx.send(lan::check_self_scan().await);
    let _ = tx.send(p2p::check_loopback(endpoint).await);
    let _ = tx.send(vpn::check_vpn_interference().await);

    let _ = tx.send(local::check_writable(&download_root).await);
    let _ = tx.send(local::check_disk_space(&download_root).await);
    if let Some(result) = local::check_firewall_windows().await {
        let _ = tx.send(result);
    }
}
