use std::path::PathBuf;

use super::{CheckAction, CheckActionKind, CheckGroup, CheckResult, CheckStatus};

const WRITABLE_ID: &str = "local.writable";
const DISK_ID: &str = "local.disk_space";
const FIREWALL_ID: &str = "local.firewall_win";
const FIREWALL_RULE_NAME: &str = "Wisp";
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
fn current_exe_for_firewall() -> Result<String, String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?;
    // current_exe() may return a path with the \\?\ extended-length prefix on
    // Windows; firewall rules store the literal path without it, so strip.
    let s = path.to_string_lossy().to_string();
    Ok(s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s))
}

#[cfg(windows)]
fn firewall_check_failed(detail: String) -> CheckResult {
    CheckResult {
        id: FIREWALL_ID.to_owned(),
        group: CheckGroup::Local,
        status: CheckStatus::Warn,
        label: "Firewall rule check failed".to_owned(),
        detail,
        hint: None,
        action: None,
    }
}

#[cfg(windows)]
fn create_firewall_action() -> CheckAction {
    CheckAction {
        label: "Create firewall rule".to_owned(),
        kind: CheckActionKind::CreateFirewallRule,
        target: None,
    }
}

#[cfg(windows)]
#[derive(Debug, Clone)]
enum FirewallProbeStatus {
    Allow(usize),
    Block(usize),
    Disabled(usize),
    None,
    Error(String),
}

/// `CREATE_NO_WINDOW` for `CreateProcessW` — suppresses the brief console
/// flash you'd otherwise see every time we shell out to PowerShell for the
/// firewall probe or rule creation.  The elevated child still pops the UAC
/// dialog (intentional: user has to consent).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
async fn query_firewall_status(exe: &str) -> FirewallProbeStatus {
    // Single-quote escape for PowerShell literal string: each ' becomes ''.
    let exe_for_ps = exe.replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'\n\
         $exe = '{exe_for_ps}'\n\
         try {{\n\
           $rules = @(Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -ieq $exe }} | ForEach-Object {{ $_ | Get-NetFirewallRule }})\n\
           if ($rules.Count -eq 0) {{ Write-Output 'STATUS:NONE'; exit 0 }}\n\
           $allow = @($rules | Where-Object {{ $_.Action -eq 'Allow' -and $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }})\n\
           if ($allow.Count -gt 0) {{ Write-Output ('STATUS:ALLOW:' + $allow.Count); exit 0 }}\n\
           $blocked = @($rules | Where-Object {{ $_.Action -eq 'Block' -and $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }})\n\
           if ($blocked.Count -gt 0) {{ Write-Output ('STATUS:BLOCK:' + $blocked.Count); exit 0 }}\n\
           Write-Output ('STATUS:DISABLED:' + $rules.Count)\n\
         }} catch {{ Write-Output ('STATUS:ERROR:' + $_.Exception.Message); exit 1 }}\n"
    );

    let probe = tokio::task::spawn_blocking(move || {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
    })
    .await;

    let output = match probe {
        Ok(Ok(out)) => out,
        Ok(Err(error)) => {
            return FirewallProbeStatus::Error(format!("Couldn't invoke PowerShell: {error}"));
        }
        Err(join_error) => {
            return FirewallProbeStatus::Error(format!("Probe task failed: {join_error}"));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let status_line = stdout
        .lines()
        .rev()
        .find(|l| l.starts_with("STATUS:"))
        .unwrap_or("")
        .trim();

    if let Some(rest) = status_line.strip_prefix("STATUS:ALLOW:") {
        FirewallProbeStatus::Allow(rest.trim().parse().unwrap_or(0))
    } else if let Some(rest) = status_line.strip_prefix("STATUS:BLOCK:") {
        FirewallProbeStatus::Block(rest.trim().parse().unwrap_or(0))
    } else if let Some(rest) = status_line.strip_prefix("STATUS:DISABLED:") {
        FirewallProbeStatus::Disabled(rest.trim().parse().unwrap_or(0))
    } else if status_line == "STATUS:NONE" {
        FirewallProbeStatus::None
    } else if let Some(rest) = status_line.strip_prefix("STATUS:ERROR:") {
        FirewallProbeStatus::Error(format!("PowerShell error: {}", rest.trim()))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        FirewallProbeStatus::Error(format!(
            "PowerShell exit {} — unexpected output. stderr: {}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<unknown>".to_owned()),
            stderr.trim()
        ))
    }
}

#[cfg(windows)]
pub(super) async fn check_firewall_windows() -> Option<CheckResult> {
    let exe = match current_exe_for_firewall() {
        Ok(p) => p,
        Err(error) => {
            return Some(firewall_check_failed(format!(
                "Couldn't resolve current executable: {error}"
            )));
        }
    };

    Some(match query_firewall_status(&exe).await {
        FirewallProbeStatus::Allow(count) => CheckResult {
            id: FIREWALL_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Pass,
            label: "Firewall rule present".to_owned(),
            detail: format!("{count} Allow rule(s) match {exe}"),
            hint: None,
            action: None,
        },
        FirewallProbeStatus::Block(count) => CheckResult {
            id: FIREWALL_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Firewall is blocking Wisp".to_owned(),
            detail: format!(
                "{count} Block rule(s) match {exe} — inbound transfers will fail."
            ),
            hint: Some(
                "Remove the blocking rule(s) in Windows Defender Firewall, then create a new Allow rule."
                    .to_owned(),
            ),
            action: Some(create_firewall_action()),
        },
        FirewallProbeStatus::Disabled(count) => CheckResult {
            id: FIREWALL_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "Firewall rule disabled".to_owned(),
            detail: format!(
                "Found {count} rule(s) for this exe but none are Enabled+Allow+Inbound."
            ),
            hint: Some(
                "Re-enable the existing rule in Windows Defender Firewall, or create a fresh Allow rule."
                    .to_owned(),
            ),
            action: Some(create_firewall_action()),
        },
        FirewallProbeStatus::None => CheckResult {
            id: FIREWALL_ID.to_owned(),
            group: CheckGroup::Local,
            status: CheckStatus::Warn,
            label: "No firewall rule for Wisp".to_owned(),
            detail: format!("Direct LAN transfers may be blocked. Program: {exe}"),
            hint: Some(
                "Tap the button below to add an inbound Allow rule for this executable. Windows will ask for admin permission."
                    .to_owned(),
            ),
            action: Some(create_firewall_action()),
        },
        FirewallProbeStatus::Error(reason) => firewall_check_failed(reason),
    })
}

#[cfg(not(windows))]
pub(super) async fn check_firewall_windows() -> Option<CheckResult> {
    None
}

/// Lightweight version of [`check_firewall_windows`] for surfacing a startup
/// banner when inbound transfers will silently fail.  Returns:
///
/// * `None` — firewall is fine (or non-Windows), no banner needed.
/// * `Some(detail)` — short user-facing reason the inbound path is at risk.
///
/// Re-runs the same PowerShell probe as the full diagnostics check so the
/// banner state matches what self-test would show.  Probe errors return
/// `None` rather than warn-spamming — if the probe is broken, the worst
/// case is a missed banner, not a false alarm.
#[cfg(windows)]
pub async fn firewall_inbound_warning() -> Option<String> {
    let exe = current_exe_for_firewall().ok()?;
    match query_firewall_status(&exe).await {
        FirewallProbeStatus::Allow(_) => None,
        FirewallProbeStatus::None => {
            Some("No firewall rule for Wisp — inbound transfers may be blocked.".to_owned())
        }
        FirewallProbeStatus::Block(_) => Some(
            "Windows Firewall has a Block rule for Wisp — inbound transfers will fail.".to_owned(),
        ),
        FirewallProbeStatus::Disabled(_) => {
            Some("Wisp's firewall rule is disabled — inbound transfers may be blocked.".to_owned())
        }
        // Swallow probe errors here; the full self-test surfaces them.
        FirewallProbeStatus::Error(_) => None,
    }
}

#[cfg(not(windows))]
pub async fn firewall_inbound_warning() -> Option<String> {
    None
}

/// Removes any existing Block rules pointing at the currently-running
/// executable, then creates a Windows Firewall inbound-Allow rule for it.
/// Spawns an elevated PowerShell via `Start-Process -Verb RunAs`, which
/// triggers a UAC prompt unless the caller is already elevated.
///
/// The two steps run in a single elevated session: if Windows had auto-
/// created a Block rule (e.g. the user denied the first-bind prompt), an
/// Allow rule alone would not unblock the exe because Windows applies the
/// most-restrictive matching rule.
///
/// Returns the human-readable error reason on failure (so it can be surfaced
/// to the user via a snackbar).  Caller is expected to re-run diagnostics on
/// success so the firewall check picks up the new rule.
#[cfg(windows)]
pub async fn create_firewall_rule_for_current_exe() -> Result<(), String> {
    use base64::Engine;

    let exe = current_exe_for_firewall()?;
    let exe_for_ps = exe.replace('\'', "''");

    // The elevated PowerShell receives this via -EncodedCommand so we don't
    // have to wrestle with nested quoting through Start-Process / -Command.
    //
    // Step 1: nuke any existing Block rules pointing at this exe — otherwise
    // Windows applies the most-restrictive matching rule and our new Allow
    // sits idle.  This covers the common case where the user denied the
    // first-bind firewall prompt, which auto-creates an inbound Block rule.
    //
    // Step 2: add a fresh inbound-Allow rule for the exe.
    let inner = format!(
        "$exe = '{exe_for_ps}'\n\
         Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -ieq $exe }} | \
         ForEach-Object {{ \
           $rule = $_ | Get-NetFirewallRule; \
           if ($rule.Action -eq 'Block') {{ $rule | Remove-NetFirewallRule -Confirm:$false }} \
         }}\n\
         New-NetFirewallRule -DisplayName '{FIREWALL_RULE_NAME}' \
         -Direction Inbound -Program $exe \
         -Action Allow -Profile Any | Out-Null"
    );
    let utf16_bytes: Vec<u8> = inner.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&utf16_bytes);

    // Outer wrapper: spawn the elevated process, wait, propagate exit code.
    // Catch Win32Exception so UAC-denied (0x800704C7 / "operation cancelled")
    // surfaces as exit 1223 (ERROR_CANCELLED) instead of an uncaught throw.
    let outer = format!(
        "try {{\n\
           $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
                -ArgumentList '-NoProfile','-EncodedCommand','{b64}'\n\
           exit $p.ExitCode\n\
         }} catch [System.ComponentModel.Win32Exception] {{ exit 1223 }}\n\
         catch {{ Write-Error $_.Exception.Message; exit 1 }}\n"
    );

    let output = tokio::task::spawn_blocking(move || {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
            .output()
    })
    .await
    .map_err(|e| format!("Task join failed: {e}"))?
    .map_err(|e| format!("Couldn't invoke PowerShell: {e}"))?;

    if output.status.success() {
        return Ok(());
    }

    // 1223 = ERROR_CANCELLED — user declined the UAC prompt.
    if output.status.code() == Some(1223) {
        return Err("Permission denied — admin elevation was cancelled.".to_owned());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "<unknown>".to_owned());
    Err(format!(
        "PowerShell exited with {code}{}",
        if stderr.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", stderr.trim())
        }
    ))
}

#[cfg(not(windows))]
pub async fn create_firewall_rule_for_current_exe() -> Result<(), String> {
    Err("Firewall rule creation is only supported on Windows.".to_owned())
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
