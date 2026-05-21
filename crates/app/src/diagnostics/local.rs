use std::path::PathBuf;

use super::{CheckGroup, CheckResult, CheckStatus};

const WRITABLE_ID: &str = "local.writable";
const DISK_ID: &str = "local.disk_space";
const FIREWALL_ID: &str = "local.firewall_win";
const MIN_FREE_BYTES: u64 = 500 * 1024 * 1024;

pub(super) async fn check_writable(download_root: &str) -> CheckResult {
    let trimmed = download_root.trim();
    if trimmed.is_empty() {
        return CheckResult {
            id: WRITABLE_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Download folder not configured".to_owned(),
            detail: "Set a download folder in Settings.".to_owned(),
            hint: None,
            action: None,
        };
    }
    if is_saf_uri(trimmed) {
        return CheckResult {
            id: WRITABLE_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Skipped,
            label: "Download folder writable".to_owned(),
            detail: "Android SAF folder — write is exercised at transfer time.".to_owned(),
            hint: None,
            action: None,
        };
    }
    let path = PathBuf::from(trimmed);
    let probe = path.join(".wisp_diag_write_test");
    let path_for_create = path.clone();
    let probe_path = probe.clone();
    let attempt = tokio::task::spawn_blocking(move || {
        // Mirror what the receiver does on the first incoming transfer:
        // create the download root if it's missing (the Android default lives
        // in app-private external storage and only materialises when a file
        // first lands there).  Without this the diagnostic falsely fails on
        // a fresh install before any transfer has run.
        std::fs::create_dir_all(&path_for_create)?;
        std::fs::write(&probe_path, b"wisp diagnostic probe")?;
        std::fs::remove_file(&probe_path)
    })
    .await;

    match attempt {
        Ok(Ok(())) => CheckResult {
            id: WRITABLE_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Pass,
            label: "Download folder writable".to_owned(),
            detail: path.display().to_string(),
            hint: None,
            action: None,
        },
        Ok(Err(error)) => CheckResult {
            id: WRITABLE_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Fail,
            label: "Download folder not writable".to_owned(),
            detail: error.to_string(),
            hint: Some(
                "Pick a different download folder in Settings, or check permissions on this one."
                    .to_owned(),
            ),
            action: None,
        },
        Err(join_error) => CheckResult {
            id: WRITABLE_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Download folder writable".to_owned(),
            detail: format!("Probe task failed: {join_error}"),
            hint: None,
            action: None,
        },
    }
}

pub(super) async fn check_disk_space(download_root: &str) -> CheckResult {
    let trimmed = download_root.trim();
    if trimmed.is_empty() || is_saf_uri(trimmed) {
        return CheckResult::skipped(
            DISK_ID,
            CheckGroup::Local,
            "Disk space",
            "Skipped — folder is unset or managed by Android SAF.",
        );
    }
    let path = PathBuf::from(trimmed);
    let result = tokio::task::spawn_blocking(move || fs2::available_space(&path)).await;
    match result {
        Ok(Ok(free)) if free >= MIN_FREE_BYTES => CheckResult {
            id: DISK_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Pass,
            label: "Disk space".to_owned(),
            detail: format!("{} free", format_bytes(free)),
            hint: None,
            action: None,
        },
        Ok(Ok(free)) => CheckResult {
            id: DISK_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Disk space low".to_owned(),
            detail: format!("{} free", format_bytes(free)),
            hint: Some(
                "Large transfers may fail. Free up space or pick a folder on another drive."
                    .to_owned(),
            ),
            action: None,
        },
        Ok(Err(error)) => CheckResult {
            id: DISK_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Disk space".to_owned(),
            detail: error.to_string(),
            hint: None,
            action: None,
        },
        Err(join_error) => CheckResult {
            id: DISK_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Disk space".to_owned(),
            detail: format!("Probe task failed: {join_error}"),
            hint: None,
            action: None,
        },
    }
}

#[cfg(windows)]
pub(super) async fn check_firewall_windows() -> Option<CheckResult> {
    let output = tokio::task::spawn_blocking(|| {
        std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "show", "rule", "name=Wisp"])
            .output()
    })
    .await
    .unwrap_or_else(|err| Err(std::io::Error::other(err.to_string())));
    Some(match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stdout_lower = stdout.to_lowercase();
            if stdout_lower.contains("no rules match") {
                CheckResult {
                    id: FIREWALL_ID.to_owned(),
                    group: CheckGroup::Local,
                    status: CheckStatus::Warn,
                    label: "Firewall rule 'Wisp' not found".to_owned(),
                    detail: "Direct LAN transfers may be blocked until Windows Firewall \
                             allows Wisp.exe on UDP inbound."
                        .to_owned(),
                    hint: Some(
                        "Accept the system prompt the first time Wisp binds, or add a \
                         firewall rule named 'Wisp' for the executable."
                            .to_owned(),
                    ),
                    action: None,
                }
            } else if !output.status.success() {
                CheckResult {
                    id: FIREWALL_ID.to_owned(),
                    group: CheckGroup::Local,
                    status: CheckStatus::Warn,
                    label: "Firewall rule check failed".to_owned(),
                    detail: format!(
                        "netsh exited with {}",
                        output
                            .status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "<unknown>".to_owned())
                    ),
                    hint: None,
                    action: None,
                }
            } else {
                CheckResult {
                    id: FIREWALL_ID.to_owned(),
                    group: CheckGroup::Local,
                    status: CheckStatus::Pass,
                    label: "Firewall rule present".to_owned(),
                    detail: "Found Windows Firewall rule named 'Wisp'.".to_owned(),
                    hint: None,
                    action: None,
                }
            }
        }
        Err(error) => CheckResult {
            id: FIREWALL_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Firewall rule check failed".to_owned(),
            detail: format!("Couldn't invoke netsh: {error}"),
            hint: None,
            action: None,
        },
    })
}

#[cfg(not(windows))]
pub(super) async fn check_firewall_windows() -> Option<CheckResult> {
    None
}

fn is_saf_uri(path: &str) -> bool {
    path.starts_with("content://")
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}
