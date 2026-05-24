use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio::fs;

/// Validates a transfer path string: non-empty, relative, `/`-separated, no `.` or `..`.
#[derive(Debug, Error)]
pub enum TransferPathError {
    #[error("transfer path must not be empty")]
    Empty,
    #[error("transfer path must use '/' separators")]
    InvalidSeparator,
    #[error("transfer path must be relative")]
    NotRelative,
    #[error("transfer path contains an invalid segment")]
    InvalidSegment,
    #[error("{path} does not have a valid UTF-8 final path component")]
    InvalidUtf8RootName { path: PathBuf },
    #[error("{path} contains a path component that is not valid UTF-8")]
    InvalidUtf8PathComponent { path: PathBuf },
    #[error("destination already exists: {path}")]
    DestinationExists { path: PathBuf },
    #[error("destination parent is a symbolic link: {path}")]
    DestinationParentIsSymlink { path: PathBuf },
    #[error("destination parent is not a directory: {path}")]
    DestinationParentNotDirectory { path: PathBuf },
    #[error("checking {path}")]
    CheckPath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("resolving current working directory")]
    CurrentDirectory {
        #[source]
        source: std::io::Error,
    },
    #[error("output directory is not absolute: {path}")]
    OutputNotAbsolute { path: PathBuf },
    #[error("system clock before unix epoch")]
    SystemClockBeforeUnixEpoch {
        #[source]
        source: std::time::SystemTimeError,
    },
    #[error("creating temp directory {path}")]
    CreateScratchDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn local_record_dir(
    out_dir: &Path,
    collection_hash: iroh_blobs::Hash,
) -> std::result::Result<PathBuf, TransferPathError> {
    Ok(out_dir
        .join(".wisp")
        .join("transfers")
        .join(collection_hash.to_hex()))
}

pub fn validate_transfer_path(path: &str) -> std::result::Result<Vec<&str>, TransferPathError> {
    if path.is_empty() {
        return Err(TransferPathError::Empty);
    }

    if path.contains('\\') {
        return Err(TransferPathError::InvalidSeparator);
    }

    if Path::new(path).is_absolute() {
        return Err(TransferPathError::NotRelative);
    }

    let mut segments = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(TransferPathError::InvalidSegment);
        }
        segments.push(segment);
    }

    Ok(segments)
}

pub fn input_root_name(path: &Path) -> std::result::Result<String, TransferPathError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| TransferPathError::InvalidUtf8RootName {
            path: path.to_path_buf(),
        })
        .map(|name| name.to_owned())
}

pub fn normalize_transfer_path(path: &Path) -> std::result::Result<String, TransferPathError> {
    let mut segments = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                let segment = segment.to_str().ok_or_else(|| {
                    TransferPathError::InvalidUtf8PathComponent {
                        path: path.to_path_buf(),
                    }
                })?;
                segments.push(segment);
            }
            _ => return Err(TransferPathError::NotRelative),
        }
    }

    if segments.is_empty() {
        return Err(TransferPathError::Empty);
    }

    let normalized = segments.join("/");
    validate_transfer_path(&normalized)?;
    Ok(normalized)
}

pub fn resolve_transfer_destination(
    out_dir: &Path,
    transfer_path: &str,
) -> std::result::Result<PathBuf, TransferPathError> {
    let segments = validate_transfer_path(transfer_path)?;
    let mut destination = out_dir.to_path_buf();
    for segment in segments {
        destination.push(segment);
    }
    Ok(destination)
}

pub async fn ensure_destination_available(
    out_dir: &Path,
    destination: &Path,
) -> std::result::Result<(), TransferPathError> {
    if path_exists(destination).await? {
        return Err(TransferPathError::DestinationExists {
            path: destination.to_path_buf(),
        });
    }

    let mut current = destination.parent();
    while let Some(parent) = current {
        if parent == out_dir {
            break;
        }

        match fs::symlink_metadata(parent).await {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    return Err(TransferPathError::DestinationParentIsSymlink {
                        path: parent.to_path_buf(),
                    });
                }
                if !file_type.is_dir() {
                    return Err(TransferPathError::DestinationParentNotDirectory {
                        path: parent.to_path_buf(),
                    });
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(TransferPathError::CheckPath {
                    path: parent.to_path_buf(),
                    source: err,
                });
            }
        }

        current = parent.parent();
    }

    Ok(())
}

pub fn resolve_output_dir(out_dir: &Path) -> std::result::Result<PathBuf, TransferPathError> {
    let base = if out_dir.is_absolute() {
        out_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| TransferPathError::CurrentDirectory { source })?
            .join(out_dir)
    };

    let mut resolved = PathBuf::new();
    for component in base.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }

    if !resolved.is_absolute() {
        return Err(TransferPathError::OutputNotAbsolute { path: resolved });
    }

    Ok(resolved)
}

async fn path_exists(path: &Path) -> std::result::Result<bool, TransferPathError> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(TransferPathError::CheckPath {
            path: path.to_path_buf(),
            source: err,
        }),
    }
}

/// Temporary directory under the process temp dir; deleted on drop.
#[derive(Debug)]
pub struct ScratchDir {
    pub path: PathBuf,
}

impl ScratchDir {
    pub async fn new(
        prefix: &str,
        session_id: &str,
    ) -> std::result::Result<Self, TransferPathError> {
        let id_digest = blake3::hash(session_id.as_bytes()).to_hex();
        let unique = format!(
            "{prefix}-{id_digest}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|source| TransferPathError::SystemClockBeforeUnixEpoch { source })?
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path)
            .await
            .map_err(|source| TransferPathError::CreateScratchDir {
                path: path.clone(),
                source,
            })?;
        Ok(Self { path })
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// RAII guard for a receiver's per-transfer record directory under
/// `<out_dir>/.wisp/transfers/<hash>/`.
///
/// Created with `delete_on_drop = false` (the safe default — keep
/// state in case the user wants to resume).  Callers flip it to
/// `true` for terminal outcomes where there's no point keeping
/// resume state:
///
/// - `Completed` — successful transfer, files already exported.
/// - `Cancelled` — user-initiated abort on either side.
/// - `Declined` — receiver explicitly rejected the offer.
///
/// Any other terminal path (`Err(_)`, including transient
/// `ConnectionClosed`, timeouts, protocol errors) leaves
/// `delete_on_drop = false` so the data sticks around for a retry.
/// Stale dirs from those failure paths are eventually garbage-
/// collected by [`sweep_stale_transfer_records`] at next startup.
#[derive(Debug)]
pub struct RecordDirGuard {
    path: PathBuf,
    delete_on_drop: bool,
}

impl RecordDirGuard {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            delete_on_drop: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Mark the directory for deletion when the guard drops.  Idempotent.
    pub fn mark_for_delete(&mut self) {
        self.delete_on_drop = true;
    }
}

impl Drop for RecordDirGuard {
    fn drop(&mut self) {
        if self.delete_on_drop {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Total on-disk size of the receiver's `.wisp` workspace under
/// `<out_dir>/.wisp/`.  Used by the Settings → Storage UI to show
/// the user how much receiver-side cache is sitting around from
/// completed-and-not-yet-GC'd transfers, mid-flight transfers, and
/// failed-pending-retry transfers.
///
/// Best-effort: any IO error on an entry is treated as 0 bytes (we'd
/// rather under-report than crash the Settings page).  Returns 0
/// when the directory doesn't exist.
pub async fn receiver_cache_size_bytes(out_dir: &Path) -> u64 {
    let root = out_dir.join(".wisp");
    walk_dir_size(&root).await
}

/// Removes the entire `<out_dir>/.wisp/` directory and everything in
/// it — both completed-but-not-GC'd records and resume state from
/// failed transfers.  Called from the Settings → Storage "Clear"
/// button.  Returns the number of bytes freed (computed before the
/// delete, best-effort).
pub async fn clear_receiver_cache(out_dir: &Path) -> u64 {
    let root = out_dir.join(".wisp");
    let freed = walk_dir_size(&root).await;
    let _ = fs::remove_dir_all(&root).await;
    freed
}

async fn walk_dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let mut entries = match fs::read_dir(&current).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

/// Walks `<out_dir>/.wisp/transfers/` and removes per-transfer
/// directories whose `record.json` is older than `max_age`.
///
/// Intended to be called once at receiver-service startup so failed
/// or interrupted transfers from previous sessions don't accumulate
/// indefinitely.  Returns the number of stale directories removed
/// (best-effort — IO errors on individual entries are logged via
/// `tracing` and counted as "not removed").
pub async fn sweep_stale_transfer_records(out_dir: &Path, max_age: std::time::Duration) -> u64 {
    let transfers = out_dir.join(".wisp").join("transfers");
    let mut entries = match fs::read_dir(&transfers).await {
        Ok(entries) => entries,
        Err(_) => return 0, // dir doesn't exist yet — nothing to sweep
    };

    let cutoff = match SystemTime::now().checked_sub(max_age) {
        Some(cutoff) => cutoff,
        None => return 0, // clock pre-epoch; refuse to sweep anything
    };

    let mut removed = 0u64;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let dir_path = entry.path();
        // Probe modification time on the record.json file specifically —
        // it's the authoritative timestamp for "last activity on this
        // transfer".  Fall back to the directory mtime if record.json
        // is missing (incomplete cleanup).
        let probe_path = dir_path.join("record.json");
        let mtime = match fs::metadata(&probe_path).await {
            Ok(m) => m.modified().ok(),
            Err(_) => match fs::metadata(&dir_path).await {
                Ok(m) => m.modified().ok(),
                Err(_) => None,
            },
        };
        let Some(mtime) = mtime else {
            continue;
        };
        if mtime < cutoff {
            match fs::remove_dir_all(&dir_path).await {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::debug!(
                        target: "wisp_core::transfer::path",
                        path = %dir_path.display(),
                        %error,
                        "sweep_stale_transfer_records skipped: remove_dir_all failed",
                    );
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::{
        RecordDirGuard, TransferPathError, ensure_destination_available, resolve_output_dir,
        sweep_stale_transfer_records,
    };
    use std::path::Path;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn record_dir_guard_keeps_dir_by_default() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("transfers").join("abc");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("record.json"), b"{}")?;

        {
            let _guard = RecordDirGuard::new(dir.clone());
            // Drop without flipping the flag.
        }
        assert!(dir.exists(), "default guard must leave the dir intact");
        Ok(())
    }

    #[test]
    fn record_dir_guard_removes_dir_when_marked_for_delete() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("transfers").join("xyz");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("record.json"), b"{}")?;

        {
            let mut guard = RecordDirGuard::new(dir.clone());
            guard.mark_for_delete();
        }
        assert!(
            !dir.exists(),
            "guard marked for delete must remove the dir on drop"
        );
        Ok(())
    }

    #[tokio::test]
    async fn sweep_stale_transfer_records_removes_old_dirs_and_keeps_fresh() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out_dir = temp.path();
        let transfers = out_dir.join(".wisp").join("transfers");

        let old = transfers.join("old");
        let fresh = transfers.join("fresh");
        std::fs::create_dir_all(&old)?;
        std::fs::create_dir_all(&fresh)?;
        std::fs::write(old.join("record.json"), b"{}")?;
        std::fs::write(fresh.join("record.json"), b"{}")?;

        // Backdate `old`'s record.json mtime to ~30 days ago.
        let thirty_days_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        filetime::set_file_mtime(
            old.join("record.json"),
            filetime::FileTime::from_system_time(thirty_days_ago),
        )?;

        let removed =
            sweep_stale_transfer_records(out_dir, std::time::Duration::from_secs(7 * 86_400)).await;

        assert_eq!(removed, 1, "exactly one stale dir should have been swept");
        assert!(!old.exists(), "old dir must be removed");
        assert!(fresh.exists(), "fresh dir must remain");
        Ok(())
    }

    #[tokio::test]
    async fn sweep_stale_transfer_records_returns_zero_when_no_transfers_dir() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let removed =
            sweep_stale_transfer_records(temp.path(), std::time::Duration::from_secs(86_400)).await;
        assert_eq!(removed, 0);
        Ok(())
    }

    #[test]
    fn resolves_relative_output_dir_against_current_dir() -> Result<()> {
        let cwd = std::env::current_dir()?;
        let resolved = resolve_output_dir(Path::new("downloads"))?;
        assert_eq!(resolved, cwd.join("downloads"));
        Ok(())
    }

    #[test]
    fn normalizes_output_dir_lexically() -> Result<()> {
        let cwd = std::env::current_dir()?;
        let resolved = resolve_output_dir(Path::new("./downloads/../downloads/inbox"))?;
        assert_eq!(resolved, cwd.join("downloads/inbox"));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_destination_available_rejects_symlinked_parents() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let out_dir = temp.path().join("downloads");
        let escaped = temp.path().join("escaped");
        let link = out_dir.join("link");
        std::fs::create_dir_all(&out_dir)?;
        std::fs::create_dir_all(&escaped)?;
        symlink(&escaped, &link)?;

        let destination = link.join("owned.txt");
        let err = ensure_destination_available(&out_dir, &destination)
            .await
            .unwrap_err();

        match err {
            TransferPathError::DestinationParentIsSymlink { path } => {
                assert_eq!(path, link);
            }
            other => panic!("unexpected error: {other:?}"),
        }

        Ok(())
    }
}
