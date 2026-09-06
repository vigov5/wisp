use std::path::PathBuf;
use std::time::{Duration, Instant};

use iroh_blobs::{
    BlobFormat,
    api::{
        Store, TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
};
use tracing::{instrument, trace};

use super::descriptor::{self, DescriptorHandles};
use super::error::{BlobError, BlobTextError, Result as BlobResult};
use crate::{
    fs_plan::{FsPlanError, SendInput},
    transfer::path::{input_root_name, normalize_transfer_path},
};

#[derive(Debug)]
pub(super) struct ImportedFile {
    pub(super) transfer_path: String,
    pub(super) temp_tag: TempTag,
    pub(super) size_bytes: u64,
}

#[derive(Debug)]
pub(super) struct ImportFilesResult {
    pub(super) files: Vec<ImportedFile>,
    pub(super) walk_metadata: Duration,
    pub(super) import_hash: Duration,
}

#[instrument(skip_all, fields(input_path = %input.path().display()))]
pub(crate) fn walk_files(input: SendInput) -> Result<Vec<(String, PathBuf)>, FsPlanError> {
    // A descriptor input is a `/proc/self/fd/<n>` symlink onto a file we hold
    // open ourselves, so it is followed rather than rejected, and there is
    // nothing below it to traverse.
    if input.is_file_descriptor() {
        let root_name = input.transfer_path()?;
        let path = absolute_input_path(input.into_path())?;
        let metadata = descriptor::metadata(&path).map_err(|source| FsPlanError::ReadMetadata {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(FsPlanError::UnsupportedFileType { path });
        }
        trace!(file_count = 1, "discovered descriptor file for import");
        return Ok(vec![(root_name, path)]);
    }

    let path = absolute_input_path(input.into_path())?;
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|source| FsPlanError::ReadMetadata {
            path: path.clone(),
            source,
        })?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return Err(FsPlanError::SymbolicLink { path });
    }

    let mut discovered = Vec::new();
    let root_name = input_root_name(&path)?;

    if file_type.is_file() {
        discovered.push((root_name, path));
    } else if file_type.is_dir() {
        let mut stack = vec![(path, PathBuf::from(root_name))];
        while let Some((current_path, transfer_path)) = stack.pop() {
            let current_metadata = std::fs::symlink_metadata(&current_path).map_err(|source| {
                FsPlanError::ReadMetadata {
                    path: current_path.clone(),
                    source,
                }
            })?;
            let current_type = current_metadata.file_type();

            if current_type.is_symlink() {
                return Err(FsPlanError::SymbolicLink { path: current_path });
            }

            if current_type.is_file() {
                discovered.push((normalize_transfer_path(&transfer_path)?, current_path));
                continue;
            }

            if !current_type.is_dir() {
                return Err(FsPlanError::UnsupportedFileType { path: current_path });
            }

            let entries =
                std::fs::read_dir(&current_path).map_err(|source| FsPlanError::ReadDirectory {
                    path: current_path.clone(),
                    source,
                })?;

            for entry in entries {
                let entry = entry.map_err(|source| FsPlanError::ReadDirectory {
                    path: current_path.clone(),
                    source,
                })?;
                let child_name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| FsPlanError::InvalidUtf8PathComponent { path: entry.path() })?;
                stack.push((entry.path(), transfer_path.join(child_name)));
            }
        }
    } else {
        return Err(FsPlanError::UnsupportedFileType { path });
    }

    discovered.sort_by(|(a, _), (b, _)| a.cmp(b));
    trace!(file_count = discovered.len(), "discovered files for import");
    Ok(discovered)
}

fn absolute_input_path(path: PathBuf) -> Result<PathBuf, FsPlanError> {
    if path.is_absolute() {
        return Ok(path);
    }

    Ok(std::env::current_dir()
        .map_err(|source| FsPlanError::CurrentDirectory { source })?
        .join(path))
}

#[instrument(skip(store), fields(input_path = %input.path().display()))]
#[cfg(test)]
pub(super) async fn import_files(
    store: &Store,
    input: SendInput,
    handles: &mut DescriptorHandles,
) -> BlobResult<Vec<ImportedFile>> {
    Ok(import_files_with_timings(store, input, handles)
        .await?
        .files)
}

#[instrument(skip(store), fields(input_path = %input.path().display()))]
pub(super) async fn import_files_with_timings(
    store: &Store,
    input: SendInput,
    handles: &mut DescriptorHandles,
) -> BlobResult<ImportFilesResult> {
    let path_display = input.path().display().to_string();
    // Before anything looks at the path.  Everything downstream — the stat in
    // the walk, the store's own open, and every read while serving — goes
    // through the descriptor from here on.
    if input.is_file_descriptor() {
        handles.register(input.path());
    }
    let walk_started = Instant::now();
    let files = walk_files(input)
        .map_err(|source| BlobError::import_files(path_display.clone(), source))?;
    let walk_metadata = walk_started.elapsed();

    let import_started = Instant::now();
    let mut imported = Vec::with_capacity(files.len());
    for (transfer_path, local_path) in files {
        trace!(
            transfer_path = %transfer_path,
            local_path = %local_path.display(),
            "importing file into blob store"
        );
        let tag = store
            .add_path_with_opts(AddPathOptions {
                path: local_path.clone(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .temp_tag()
            .await
            .map_err(|source| {
                BlobError::import_files(
                    path_display.clone(),
                    BlobTextError::new(format!("importing {}: {source}", local_path.display())),
                )
            })?;
        imported.push(ImportedFile {
            transfer_path,
            temp_tag: tag,
            size_bytes: descriptor::metadata(&local_path)
                .map_err(|source| {
                    BlobError::import_files(
                        path_display.clone(),
                        BlobTextError::new(format!(
                            "reading metadata for {}: {source}",
                            local_path.display()
                        )),
                    )
                })?
                .len(),
        });
    }
    let import_hash = import_started.elapsed();
    trace!(imported_count = imported.len(), "finished importing files");
    Ok(ImportFilesResult {
        files: imported,
        walk_metadata,
        import_hash,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use iroh_blobs::{api::Store, store::mem::MemStore};

    use super::{DescriptorHandles, import_files, walk_files};
    use crate::fs_plan::SendInput;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "{}-{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        std::env::temp_dir().join(unique)
    }

    #[tokio::test]
    async fn import_collects_files_in_stable_order_with_sizes() -> Result<()> {
        let root = unique_temp_dir("wisp-one-shot-import");
        let input = root.join("input");
        let nested = input.join("nested");
        std::fs::create_dir_all(&nested)?;
        std::fs::write(input.join("z.txt"), b"zzz")?;
        std::fs::write(nested.join("a.txt"), b"aaaa")?;
        let store: Store = MemStore::new().into();

        let mut handles = DescriptorHandles::new();
        let imported = import_files(&store, SendInput::from(input), &mut handles).await?;
        let transfer_paths = imported
            .iter()
            .map(|file| file.transfer_path.clone())
            .collect::<Vec<_>>();
        let total_size = imported.iter().map(|file| file.size_bytes).sum::<u64>();

        assert_eq!(transfer_paths, vec!["input/nested/a.txt", "input/z.txt"]);
        assert_eq!(total_size, 7);
        assert_eq!(imported.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[tokio::test]
    async fn import_accepts_single_file_path() -> Result<()> {
        let root = unique_temp_dir("wisp-one-shot-single-file");
        let input_dir = root.join("input");
        std::fs::create_dir_all(&input_dir)?;
        let file_path = input_dir.join("hello.txt");
        std::fs::write(&file_path, b"hello")?;
        let store: Store = MemStore::new().into();

        let mut handles = DescriptorHandles::new();
        let imported = import_files(&store, SendInput::from(file_path), &mut handles).await?;
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].transfer_path, "hello.txt");
        assert_eq!(imported[0].size_bytes, 5);

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_rejects_nested_symbolic_links() -> Result<()> {
        let root = unique_temp_dir("wisp-walk-files-symlink");
        let input = root.join("input");
        std::fs::create_dir_all(&input)?;
        std::fs::write(input.join("real.txt"), b"real")?;
        symlink("real.txt", input.join("link.txt"))?;

        let err = walk_files(SendInput::from(input)).expect_err("expected symlink rejection");
        assert!(err.to_string().contains("symbolic link"));

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// The Android send path hands the core `/proc/self/fd/<n>`, which is a
    /// symlink onto an already-open file.  The plain `Path` walk rejects
    /// symlinks, so the descriptor variant has to opt out of that check and
    /// carry the real name instead of the fd number.
    /// A picked folder has no single handle to open, so it is sent as one
    /// descriptor per file, each carrying its path within the folder.  Those
    /// paths are what the receiver rebuilds the tree from.
    #[cfg(unix)]
    #[tokio::test]
    async fn import_rebuilds_a_folder_from_per_file_descriptors() -> Result<()> {
        use std::os::fd::AsRawFd;

        let root = unique_temp_dir("wisp-import-descriptor-tree");
        std::fs::create_dir_all(&root)?;
        let mut held = Vec::new();
        let mut inputs = Vec::new();
        for (transfer_path, body) in [
            ("photos/trip/cat.jpg", b"meow".as_slice()),
            ("photos/dog.jpg", b"woof!".as_slice()),
        ] {
            let backing = root.join(transfer_path.replace('/', "_"));
            std::fs::write(&backing, body)?;
            let file = std::fs::File::open(&backing)?;
            let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
            if !fd_path.exists() {
                // No procfs (macOS): nothing to assert here.
                let _ = std::fs::remove_dir_all(&root);
                return Ok(());
            }
            inputs.push(SendInput::FileDescriptor {
                path: fd_path,
                transfer_path: transfer_path.to_owned(),
            });
            held.push(file);
        }
        let store: Store = MemStore::new().into();

        let mut handles = DescriptorHandles::new();
        let mut imported = Vec::new();
        for input in inputs {
            imported.extend(import_files(&store, input, &mut handles).await?);
        }

        let paths = imported
            .iter()
            .map(|file| file.transfer_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["photos/trip/cat.jpg", "photos/dog.jpg"]);
        assert_eq!(imported.iter().map(|file| file.size_bytes).sum::<u64>(), 9);

        drop(held);
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A file small enough to be stored inline never becomes a referenced
    /// path, so it is read once at import and through a different branch —
    /// which also has to go through the descriptor when the path will not
    /// open.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_small_unreopenable_descriptor_is_inlined_from_its_descriptor() -> Result<()> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;

        use iroh_blobs::store::fs::FsStore;

        let root = unique_temp_dir("wisp-import-unreopenable-small");
        std::fs::create_dir_all(&root)?;
        let backing = root.join("note.txt");
        let body = b"a note small enough to live inline".to_vec();
        std::fs::write(&backing, &body)?;
        let file = std::fs::File::open(&backing)?;
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        if !fd_path.exists() {
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }

        std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o000))?;
        if std::fs::File::open(&fd_path).is_ok() {
            let _ = std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644));
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }

        let store = FsStore::load(root.join("store")).await?;
        let mut handles = DescriptorHandles::new();
        let imported = import_files(
            store.as_ref(),
            SendInput::FileDescriptor {
                path: fd_path,
                transfer_path: "note.txt".to_owned(),
            },
            &mut handles,
        )
        .await?;

        let file_entry = imported.first().expect("one imported file");
        assert_eq!(file_entry.size_bytes, body.len() as u64);
        let served = store.get_bytes(file_entry.temp_tag.hash()).await?;
        assert_eq!(&served[..], &body[..]);

        drop(imported);
        store.shutdown().await?;
        drop(handles);
        let _ = std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// The case the whole descriptor path exists for.  Android grants a file
    /// by handing over a descriptor, and refuses to open the same file by name
    /// a second time — so the store cannot reopen `/proc/self/fd/<n>` the way
    /// it reopens any other referenced path.  `chmod 000` reproduces that
    /// exactly: the descriptor we are already holding keeps reading, and the
    /// magic link stops opening.
    ///
    /// Uses the on-disk store because that is the one that references a file
    /// in place instead of copying it, and so the one that has to read the
    /// path again later.
    #[cfg(unix)]
    #[tokio::test]
    async fn import_reads_a_descriptor_whose_path_cannot_be_reopened() -> Result<()> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;

        use iroh_blobs::store::fs::FsStore;

        let root = unique_temp_dir("wisp-import-unreopenable");
        std::fs::create_dir_all(&root)?;
        let backing = root.join("granted.bin");
        // Comfortably past `max_data_inlined`, so the store references the file
        // rather than swallowing it whole.
        let body = vec![0xA5u8; 256 * 1024];
        std::fs::write(&backing, &body)?;
        let file = std::fs::File::open(&backing)?;
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        if !fd_path.exists() {
            // No procfs (macOS): nothing to assert here.
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }

        std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o000))?;
        if std::fs::File::open(&fd_path).is_ok() {
            // Running with privileges that ignore the mode (root in a
            // container): the refusal cannot be staged, so there is nothing to
            // prove here.
            let _ = std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644));
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }

        let store = FsStore::load(root.join("store")).await?;
        let mut handles = DescriptorHandles::new();
        let imported = import_files(
            store.as_ref(),
            SendInput::FileDescriptor {
                path: fd_path,
                transfer_path: "granted.bin".to_owned(),
            },
            &mut handles,
        )
        .await?;

        let file_entry = imported.first().expect("one imported file");
        assert_eq!(file_entry.transfer_path, "granted.bin");
        assert_eq!(file_entry.size_bytes, body.len() as u64);
        // And the bytes are actually servable afterwards, which is the part
        // that reopens the path.
        let served = store.get_bytes(file_entry.temp_tag.hash()).await?;
        assert_eq!(served.len(), body.len());
        assert_eq!(&served[..], &body[..]);

        drop(imported);
        store.shutdown().await?;
        drop(handles);
        let _ = std::fs::set_permissions(&backing, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_follows_a_descriptor_path_and_uses_its_carried_name() -> Result<()> {
        let root = unique_temp_dir("wisp-import-descriptor");
        std::fs::create_dir_all(&root)?;
        let backing = root.join("backing-store-name.bin");
        std::fs::write(&backing, b"holiday")?;
        let file = std::fs::File::open(&backing)?;
        let fd_path = PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&file)
        ));
        if !fd_path.exists() {
            // No procfs (macOS): nothing to assert here.
            let _ = std::fs::remove_dir_all(&root);
            return Ok(());
        }
        let store: Store = MemStore::new().into();

        let mut handles = DescriptorHandles::new();
        let imported = import_files(
            &store,
            SendInput::FileDescriptor {
                path: fd_path,
                transfer_path: "holiday.mp4".to_owned(),
            },
            &mut handles,
        )
        .await?;

        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].transfer_path, "holiday.mp4");
        assert_eq!(imported[0].size_bytes, 7);

        drop(file);
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
