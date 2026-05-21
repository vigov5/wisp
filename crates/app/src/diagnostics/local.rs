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

    #[cfg(target_os = "android")]
    {
        // Android never writes via direct filesystem APIs from Rust — the
        // receiver hands files to native Kotlin which routes through SAF
        // (if the user picked a folder) or MediaStore (the default
        // Download/Wisp).  Probing the configured download_root would
        // either succeed against an app-private path the user never sees
        // or fail on a SAF URI that fs::write can't touch.  Just surface
        // the actual destination so the diag matches what Settings shows.
        return android_writable_summary(trimmed);
    }

    #[cfg(not(target_os = "android"))]
    {
        if is_saf_uri(trimmed) {
            // Desktop shouldn't see a SAF URI, but be defensive and skip
            // rather than probe a path that can't be written via fs.
            return CheckResult {
                id: WRITABLE_ID.to_owned(),
                group: CheckGroup::Local,
                status: CheckStatus::Skipped,
                label: "Download folder writable".to_owned(),
                detail: "SAF folder — write is exercised at transfer time.".to_owned(),
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
            // create the download root if it's missing (the desktop default
            // lives under the user's Downloads folder and is normally
            // present, but tests may run against a custom path).  Without
            // this the diagnostic falsely fails on a fresh install before
            // any transfer has run.
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
}

#[cfg(target_os = "android")]
fn android_writable_summary(trimmed: &str) -> CheckResult {
    let (destination, mechanism) = if trimmed.starts_with("content://") {
        (
            saf_uri_destination(trimmed).unwrap_or_else(|| "Selected folder".to_owned()),
            "SAF",
        )
    } else {
        ("Download/Wisp".to_owned(), "MediaStore")
    };
    CheckResult {
        id: WRITABLE_ID.to_owned(),
        group: CheckGroup::Local,
        status: CheckStatus::Skipped,
        label: "Download folder writable".to_owned(),
        detail: format!("Saves to {destination} via {mechanism} at transfer time."),
        hint: None,
        action: None,
    }
}

// Extracts a user-friendly folder name from a SAF tree URI.  SAF URIs look
// like `content://com.android.externalstorage.documents/tree/primary%3ADCIM`
// where the last path segment is a percent-encoded document id with the
// shape `<volume>:<relative-path>`.  Returns the relative path part (e.g.
// "DCIM" or "Download/Wisp") so the diag's detail matches what the user
// sees in Settings.
#[cfg(target_os = "android")]
fn saf_uri_destination(uri: &str) -> Option<String> {
    let last = uri.rsplit('/').next()?;
    let decoded = percent_decode(last);
    if let Some(idx) = decoded.find(':') {
        let after = &decoded[idx + 1..];
        if !after.is_empty() {
            return Some(after.to_owned());
        }
    }
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

#[cfg(target_os = "android")]
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
