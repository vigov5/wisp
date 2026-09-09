use std::path::PathBuf;

use iroh_blobs::{
    BlobFormat,
    api::{
        Store, TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
};
use tokio::task::JoinSet;
use tracing::{instrument, trace};

use super::descriptor::{self, DescriptorHandles};
use super::error::{BlobError, BlobTextError, Result as BlobResult};
use crate::{
    fs_plan::{FsPlanError, SendInput},
    transfer::path::{input_root_name, normalize_transfer_path},
};

/// A file the walk found, waiting to be read and hashed.
#[derive(Debug)]
pub(super) struct PendingImport {
    /// The input this file was discovered under, for the error message. Every
    /// file of one input reports the input's path, not its own, which is what
    /// the serial version did.
    pub(super) input_display: String,
    pub(super) transfer_path: String,
    pub(super) local_path: PathBuf,
}

#[derive(Debug)]
pub(super) struct ImportedFile {
    pub(super) transfer_path: String,
    pub(super) temp_tag: TempTag,
    pub(super) size_bytes: u64,
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

/// Ceiling on files read and hashed at once, whatever the core count.
///
/// Past a handful the work stops being CPU-bound and starts queueing on the
/// flash, and every extra slot is one more tokio worker held by a synchronous
/// read (see [`import_pending`]).
const MAX_IMPORT_CONCURRENCY: usize = 8;

/// Overrides [`import_concurrency`]. `1` restores the old serial behaviour,
/// which is how the two are compared on a device without rebuilding.
const IMPORT_CONCURRENCY_ENV: &str = "WISP_IMPORT_CONCURRENCY";

/// How many files to hash at once: half the cores, at least one.
///
/// Half rather than all because the read-and-hash inside iroh-blobs is
/// synchronous — `init_outboard` drives a `std::io::BufReader`, there is no
/// `spawn_blocking` under it — so each in-flight import occupies a runtime
/// worker outright. The runtime is `new_multi_thread` with the default worker
/// count, so filling it would stall mDNS, the pairing keepalive and the FRB
/// event pump for the whole hash, which on 6.2 GB is 66 seconds.
///
/// Half the cores is a starting point, not a measured optimum: the serial
/// import ran at 94 MB/s where the same phone reads sequentially at 160, so
/// the ceiling is disk, not cores, and where between 1 and 8 that ceiling is
/// reached has to be measured per device. [`IMPORT_CONCURRENCY_ENV`] is how.
pub(super) fn import_concurrency() -> usize {
    if let Some(raw) = std::env::var_os(IMPORT_CONCURRENCY_ENV) {
        if let Some(parsed) = raw
            .to_str()
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            let clamped = parsed.clamp(1, MAX_IMPORT_CONCURRENCY);
            trace!(
                requested = parsed,
                concurrency = clamped,
                "{IMPORT_CONCURRENCY_ENV} set"
            );
            return clamped;
        }
    }
    let cores = std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(1);
    (cores / 2).clamp(1, MAX_IMPORT_CONCURRENCY)
}

/// Reads and hashes every pending file, up to `concurrency` at once, and
/// returns them in the order the walk found them.
///
/// Why this is worth concurrency at all: the store's actor answers an
/// `ImportPath` command by spawning it as its own task, so N commands in
/// flight really do occupy N runtime workers rather than queueing behind one
/// actor loop. The hash is 39% of a large send's wall clock — 66 s of the
/// 6.2 GB case, before a single byte leaves the device — and it cannot be
/// overlapped with the transfer itself, because the collection hash needs
/// every file's hash before there is a ticket to send. Across files is
/// therefore the only axis available.
///
/// Order and errors are deliberately identical to the serial version: results
/// land in their original slots, and the error reported is the lowest-indexed
/// one, not whichever task happened to fail first. On the first failure no new
/// file is started, so at most `concurrency` extra files are read before the
/// call returns.
pub(super) async fn import_pending(
    store: &Store,
    pending: Vec<PendingImport>,
    concurrency: usize,
) -> BlobResult<Vec<ImportedFile>> {
    let concurrency = concurrency.max(1);
    trace!(
        files = pending.len(),
        concurrency, "importing files into blob store"
    );

    let mut results: Vec<Option<BlobResult<ImportedFile>>> =
        (0..pending.len()).map(|_| None).collect();
    let mut tasks: JoinSet<(usize, BlobResult<ImportedFile>)> = JoinSet::new();
    let mut next = 0usize;
    let mut failed = false;

    while next < pending.len() && tasks.len() < concurrency {
        tasks.spawn(import_one(store.clone(), &pending[next], next));
        next += 1;
    }

    while let Some(joined) = tasks.join_next().await {
        let (index, result) = match joined {
            Ok(pair) => pair,
            // The import task panicked. Reporting it beats leaving the send
            // hanging on a slot that will never be filled.
            Err(source) => {
                return Err(BlobError::import_files(
                    "an import task".to_owned(),
                    BlobTextError::new(format!("task failed: {source}")),
                ));
            }
        };
        failed |= result.is_err();
        results[index] = Some(result);
        if !failed && next < pending.len() {
            tasks.spawn(import_one(store.clone(), &pending[next], next));
            next += 1;
        }
    }

    let mut imported = Vec::with_capacity(results.len());
    for slot in results {
        match slot {
            Some(result) => imported.push(result?),
            // Only reachable once a failure stopped the queue, and then only
            // after the `?` above has already returned.
            None => break,
        }
    }
    trace!(imported_count = imported.len(), "finished importing files");
    Ok(imported)
}

/// One file's read + hash, owning everything so it can be spawned.
fn import_one(
    store: Store,
    pending: &PendingImport,
    index: usize,
) -> impl std::future::Future<Output = (usize, BlobResult<ImportedFile>)> + Send + 'static {
    let input_display = pending.input_display.clone();
    let transfer_path = pending.transfer_path.clone();
    let local_path = pending.local_path.clone();
    async move {
        let result = async {
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
                        input_display.clone(),
                        BlobTextError::new(format!("importing {}: {source}", local_path.display())),
                    )
                })?;
            let size_bytes = descriptor::metadata(&local_path)
                .map_err(|source| {
                    BlobError::import_files(
                        input_display.clone(),
                        BlobTextError::new(format!(
                            "reading metadata for {}: {source}",
                            local_path.display()
                        )),
                    )
                })?
                .len();
            Ok(ImportedFile {
                transfer_path,
                temp_tag: tag,
                size_bytes,
            })
        }
        .await;
        (index, result)
    }
}

/// Registers a descriptor input and walks it into its files, in walk order.
///
/// The serial half of an import: `stat` and `read_dir` only, so it stays
/// serial — the order of the files, and of any error, is exactly what it was.
pub(super) fn walk_input(
    input: SendInput,
    handles: &mut DescriptorHandles,
) -> BlobResult<Vec<PendingImport>> {
    let input_display = input.path().display().to_string();
    // Before anything looks at the path.  Everything downstream — the stat in
    // the walk, the store's own open, and every read while serving — goes
    // through the descriptor from here on.
    if input.is_file_descriptor() {
        handles.register(input.path());
    }
    let files = walk_files(input)
        .map_err(|source| BlobError::import_files(input_display.clone(), source))?;
    Ok(files
        .into_iter()
        .map(|(transfer_path, local_path)| PendingImport {
            input_display: input_display.clone(),
            transfer_path,
            local_path,
        })
        .collect())
}

/// One input's walk and import.
///
/// Test-only: [`super::send::PreparedStore::prepare`] walks every input before
/// importing any of them, so that all the files of a multi-input send share one
/// concurrency window rather than one per input — which matters most on
/// Android, where a folder arrives as one descriptor input *per file*.
#[instrument(skip(store), fields(input_path = %input.path().display()))]
#[cfg(test)]
pub(super) async fn import_files(
    store: &Store,
    input: SendInput,
    handles: &mut DescriptorHandles,
) -> BlobResult<Vec<ImportedFile>> {
    let pending = walk_input(input, handles)?;
    import_pending(store, pending, import_concurrency()).await
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use iroh_blobs::{api::Store, store::mem::MemStore};

    use super::{
        DescriptorHandles, MAX_IMPORT_CONCURRENCY, PendingImport, import_concurrency, import_files,
        import_pending, walk_files,
    };
    use crate::fs_plan::SendInput;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    /// The whole error chain as one string: `{:#}` renders only the outermost
    /// message, so the file that actually failed lives in a source below it.
    fn error_chain(error: &dyn std::error::Error) -> String {
        let mut parts = vec![error.to_string()];
        let mut current = error.source();
        while let Some(source) = current {
            parts.push(source.to_string());
            current = source.source();
        }
        parts.join(": ")
    }

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

    /// Prints how import wall clock scales with concurrency. Asserts nothing.
    ///
    /// ```text
    /// cargo test -p wisp-core --release -- --ignored --nocapture import_scaling
    /// ```
    ///
    /// Measures the CPU half only: the files are in the page cache after the
    /// warm-up pass, so this shows how far BLAKE3 + outboard writing scale
    /// across cores and nothing about flash. The disk half is what decides the
    /// default on a phone, and that has to be measured on the phone — set
    /// `WISP_IMPORT_CONCURRENCY` and send the same folder twice.
    ///
    /// A fresh store root per run on purpose: a store that already holds a
    /// hash can skip the work, which would make every run after the first
    /// look free.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "measurement, not a check"]
    async fn import_scaling() -> Result<()> {
        use iroh_blobs::store::fs::FsStore;
        use std::time::Instant;

        const FILES: usize = 400;
        const FILE_BYTES: usize = 2 * 1024 * 1024;

        let root = unique_temp_dir("wisp-import-scaling");
        let input = root.join("input");
        std::fs::create_dir_all(&input)?;
        for index in 0..FILES {
            // Varied content so nothing dedups into one blob.
            let mut data = vec![0u8; FILE_BYTES];
            data[..8].copy_from_slice(&(index as u64).to_le_bytes());
            std::fs::write(input.join(format!("f{index:04}.bin")), &data)?;
        }
        let total_mib = (FILES * FILE_BYTES) as f64 / (1024.0 * 1024.0);
        println!("{FILES} files, {total_mib:.0} MiB total");

        let run = |concurrency: usize, label: &'static str| {
            let input = input.clone();
            let store_root = root.join(format!("store-{label}"));
            async move {
                let store = FsStore::load(&store_root).await.expect("store");
                let mut handles = DescriptorHandles::new();
                let pending =
                    super::walk_input(SendInput::from(input), &mut handles).expect("walk");
                let started = Instant::now();
                let imported = import_pending(store.as_ref(), pending, concurrency)
                    .await
                    .expect("import");
                let elapsed = started.elapsed();
                assert_eq!(imported.len(), FILES);
                elapsed
            }
        };

        // Warm the page cache so the comparison is CPU, not first-read I/O.
        let _ = run(1, "warmup").await;

        let mut baseline = None;
        for concurrency in [1usize, 2, 4, 8] {
            let label: &'static str = match concurrency {
                1 => "c1",
                2 => "c2",
                4 => "c4",
                _ => "c8",
            };
            let elapsed = run(concurrency, label).await;
            let seconds = elapsed.as_secs_f64();
            let speedup = baseline.map(|base: f64| base / seconds).unwrap_or(1.0);
            println!(
                "concurrency {concurrency}: {seconds:6.2} s  {:6.1} MiB/s  {speedup:.2}x",
                total_mib / seconds
            );
            if baseline.is_none() {
                baseline = Some(seconds);
            }
        }

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A folder of many files imports in walk order whatever the concurrency,
    /// and reports the same sizes.
    ///
    /// The point of the concurrency is that files finish out of order; the
    /// point of this test is that nothing downstream can tell. Order decides
    /// the collection's contents, the transfer plan's file ids and therefore
    /// which file the receiver's progress bar calls active, so a result that
    /// depended on completion order would be a silent wire-visible bug.
    #[tokio::test]
    async fn concurrent_import_keeps_walk_order() -> Result<()> {
        let root = unique_temp_dir("wisp-concurrent-import");
        let input = root.join("input");
        std::fs::create_dir_all(&input)?;
        // Deliberately uneven: a uniform set could come back in order by luck.
        for index in 0..40u32 {
            let size = if index % 4 == 0 { 64 * 1024 } else { 16 };
            std::fs::write(
                input.join(format!("f{index:03}.bin")),
                vec![index as u8; size],
            )?;
        }

        let serial = {
            let store: Store = MemStore::new().into();
            let mut handles = DescriptorHandles::new();
            let pending = super::walk_input(SendInput::from(input.clone()), &mut handles)?;
            import_pending(&store, pending, 1).await?
        };
        let concurrent = {
            let store: Store = MemStore::new().into();
            let mut handles = DescriptorHandles::new();
            let pending = super::walk_input(SendInput::from(input.clone()), &mut handles)?;
            import_pending(&store, pending, MAX_IMPORT_CONCURRENCY).await?
        };

        assert_eq!(serial.len(), 40);
        let paths = |files: &[super::ImportedFile]| {
            files
                .iter()
                .map(|file| (file.transfer_path.clone(), file.size_bytes))
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&serial), paths(&concurrent));
        // And the same bytes: identical hashes, not just identical names.
        let hashes = |files: &[super::ImportedFile]| {
            files
                .iter()
                .map(|file| file.temp_tag.hash())
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(&serial), hashes(&concurrent));

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// With two unreadable files in flight at once, the error reported is the
    /// earlier one in walk order — not whichever task happened to fail first.
    #[tokio::test]
    async fn concurrent_import_reports_the_first_failure_in_order() -> Result<()> {
        let root = unique_temp_dir("wisp-concurrent-import-error");
        std::fs::create_dir_all(&root)?;
        let good = root.join("good.bin");
        std::fs::write(&good, b"ok")?;

        // Missing files fail the store's own open. Two of them, far enough
        // apart that a serial run would stop at the first.
        let pending = vec![
            PendingImport {
                input_display: root.display().to_string(),
                transfer_path: "good.bin".to_owned(),
                local_path: good.clone(),
            },
            PendingImport {
                input_display: root.display().to_string(),
                transfer_path: "early-missing.bin".to_owned(),
                local_path: root.join("early-missing.bin"),
            },
            PendingImport {
                input_display: root.display().to_string(),
                transfer_path: "late-missing.bin".to_owned(),
                local_path: root.join("late-missing.bin"),
            },
        ];

        let store: Store = MemStore::new().into();
        let error = import_pending(&store, pending, MAX_IMPORT_CONCURRENCY)
            .await
            .expect_err("a missing file must fail the import");
        let message = error_chain(&error);
        assert!(
            message.contains("early-missing.bin"),
            "expected the earlier failure, got: {message}"
        );
        assert!(
            !message.contains("late-missing.bin"),
            "expected only the earlier failure, got: {message}"
        );

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn import_concurrency_stays_within_its_bounds() {
        let concurrency = import_concurrency();
        assert!(
            (1..=MAX_IMPORT_CONCURRENCY).contains(&concurrency),
            "concurrency {concurrency} outside 1..={MAX_IMPORT_CONCURRENCY}"
        );
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
