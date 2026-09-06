//! Streaming download: verified bytes go straight to their destination.
//!
//! The receiver used to fetch a whole collection into an `FsStore` and only
//! then export each file, so it needed room for two copies at once. Here the
//! get response is consumed as it arrives and each chunk is written where the
//! file actually belongs, so the peak is one copy.
//!
//! Verification is unchanged: the get fsm checks every chunk against the hash
//! before yielding it, so nothing unverified is ever written. What *is*
//! different is that a partial file now exists somewhere real, and its
//! whole-file hash is not confirmed until the last chunk lands. Two shapes
//! handle that:
//!
//! - an ordinary path is built under the record dir ([`PARTS_DIR`]) and renamed
//!   onto its destination at the end, so a file at its real name is finished by
//!   construction and an abandoned transfer leaves nothing in the user's folder;
//! - a platform descriptor is already the final location and is written in
//!   place, with the platform keeping it out of sight until told otherwise
//!   (Android marks the MediaStore entry pending, which also hides it from the
//!   filesystem under a `.pending-…` name).

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bao_tree::io::BaoContentItem;
use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh_blobs::Hash;
use iroh_blobs::format::collection::{Collection, SimpleStore};
use iroh_blobs::get::fsm;
use iroh_blobs::get::request::get_blob;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, trace};

use super::error::{BlobError, BlobTextError, Result};
use super::receive::{BlobDownloadUpdate, PROGRESS_EMIT_INTERVAL, ProgressCoalescer};
use super::telemetry::BlobTransferTelemetry;

/// Bytes per bao chunk group at iroh's block size (`BlockSize::from_chunk_log(4)`
/// - 16 chunks of 1 KiB). A resumed file has to restart on one of these
/// boundaries, because that is the finest granularity a range request can name.
const CHUNK_GROUP_BYTES: u64 = 16 * 1024;

/// Directory under the transfer's record dir where files are built.
///
/// A partial file is only as trustworthy as its verified prefix, and its
/// whole-file hash is not confirmed until the last chunk arrives, so it cannot
/// sit at the destination where the user - or a resumed run - would take it for
/// a finished file. Keeping partials here rather than beside the destination
/// also means an abandoned transfer leaves nothing in the user's folder: the
/// leftovers are inside `.wisp/`, which is where the transfer's other resume
/// state already lives.
pub(crate) const PARTS_DIR: &str = "parts";

/// One file to fetch, and where it goes.
#[derive(Debug, Clone)]
pub(crate) struct StreamTarget {
    /// Path within the transfer, which is the key into the collection.
    pub(crate) transfer_path: String,
    /// Where the finished file goes.
    pub(crate) destination: PathBuf,
    /// Where it is built first. Renamed onto [`Self::destination`] once the
    /// last chunk verifies; both live on the same volume, so that is atomic.
    /// Unused when [`Self::platform_descriptor`] is set.
    pub(crate) partial: PathBuf,
    /// True when [`Self::destination`] is a descriptor the platform opened for
    /// writing. It is already the final location, so the file is written in
    /// place: there is nothing to rename, and the platform is responsible for
    /// keeping it out of sight until the transfer reports success.
    pub(crate) platform_descriptor: bool,
    /// Size from the manifest, used for progress accounting.
    pub(crate) size: u64,
}

/// Reads blobs straight off the connection.
///
/// [`Collection::load`] needs somewhere to read the two small blobs a
/// collection is made of: its hash sequence, and the metadata naming each
/// entry. Without a local store, the provider itself is that somewhere.
struct ConnectionStore(Connection);

impl SimpleStore for ConnectionStore {
    async fn load(&self, hash: Hash) -> n0_error::Result<Bytes> {
        get_blob(self.0.clone(), hash)
            .bytes()
            .await
            .map_err(|source| n0_error::anyerr!("fetching blob {hash}: {source}"))
    }
}

/// Fetches every target over `connection`, writing each file to its destination
/// as the bytes arrive.
///
/// Progress is a cumulative byte count across all targets, matching what the
/// store-backed path emits, so the caller's tracker does not care which path
/// produced it.
pub(super) async fn stream_collection(
    connection: Connection,
    root_hash: Hash,
    targets: Vec<StreamTarget>,
    update_tx: mpsc::UnboundedSender<BlobDownloadUpdate>,
    telemetry: Option<&BlobTransferTelemetry>,
) -> Result<()> {
    let collection = Collection::load(root_hash, &ConnectionStore(connection.clone()))
        .await
        .map_err(|source| {
            BlobError::fetch(
                format!("collection {root_hash}"),
                BlobTextError::new(format!("{source:#}")),
            )
        })?;
    let hashes: HashMap<&str, Hash> = collection
        .iter()
        .map(|(name, hash)| (name.as_str(), *hash))
        .collect();

    let mut done_bytes = 0_u64;
    let mut progress = ProgressCoalescer::new(PROGRESS_EMIT_INTERVAL);
    for target in &targets {
        let hash = *hashes.get(target.transfer_path.as_str()).ok_or_else(|| {
            BlobError::fetch(
                format!("collection {root_hash}"),
                BlobTextError::new(format!(
                    "missing file in collection: {}",
                    target.transfer_path
                )),
            )
        })?;
        stream_one(
            &connection,
            hash,
            target,
            done_bytes,
            &update_tx,
            &mut progress,
            telemetry,
        )
        .await?;
        done_bytes = done_bytes.saturating_add(target.size);
    }

    // Never let throttling hide the final position from the resume record.
    if let Some(bytes_received) = progress.flush_pending(Instant::now()) {
        let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
    }
    let _ = update_tx.send(BlobDownloadUpdate::Progress {
        bytes_received: done_bytes,
    });
    let _ = update_tx.send(BlobDownloadUpdate::Done);
    Ok(())
}

/// Fetches one blob into `target.destination`.
///
/// `done_bytes` is how much of the whole transfer landed before this file, so
/// the progress emitted here stays cumulative.
#[allow(clippy::too_many_arguments)]
async fn stream_one(
    connection: &Connection,
    hash: Hash,
    target: &StreamTarget,
    done_bytes: u64,
    update_tx: &mpsc::UnboundedSender<BlobDownloadUpdate>,
    progress: &mut ProgressCoalescer,
    telemetry: Option<&BlobTransferTelemetry>,
) -> Result<()> {
    let context = || format!("{} ({hash})", target.transfer_path);

    // Where the bytes actually go. A descriptor is written in place; a path is
    // built under the record dir and renamed at the end.
    let sink = if target.platform_descriptor {
        &target.destination
    } else {
        // A file already at its real name is finished: it only got that name
        // after its last chunk verified. (A descriptor always exists, so this
        // can only be asked of a path.)
        if tokio::fs::metadata(&target.destination).await.is_ok() {
            trace!(path = %target.transfer_path, "destination already complete, skipping");
            return Ok(());
        }
        if let Some(parent) = target.destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| BlobError::fetch(context(), source))?;
        }
        &target.partial
    };
    if let Some(parent) = sink.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| BlobError::fetch(context(), source))?;
    }
    let resume_at = resumable_prefix_len(sink).await;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(sink)
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    // Anything past the last whole chunk group cannot be named by a range
    // request, so it is refetched rather than trusted.
    file.set_len(resume_at)
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    if resume_at > 0 {
        debug!(path = %target.transfer_path, resume_at, "resuming partial file");
    }

    // `get_blob` always fetches a whole blob, so a resumed file needs its own
    // ranged request. Byte bounds are converted outward to whole chunks; the
    // offset is already chunk-group aligned, so nothing is skipped.
    let request = if resume_at == 0 {
        GetRequest::blob(hash)
    } else {
        GetRequest::blob_ranges(hash, ChunkRanges::bytes(resume_at..))
    };

    let mut written = resume_at;
    let start = fsm::start(connection.clone(), request, Default::default());
    let connected = start
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    let fsm::ConnectedNext::StartRoot(start_root) = connected
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?
    else {
        // A single-blob request has no children, so the fsm can only be at the
        // root here.
        return Err(BlobError::fetch(
            context(),
            BlobTextError::new("expected the request to start at the root"),
        ));
    };
    let (mut curr, _size) = start_root
        .next()
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    let end = loop {
        match curr.next().await {
            fsm::BlobContentNext::More((next, item)) => {
                // Parent hashes arrive interleaved with the data; they are what
                // let a suffix be verified, and the fsm consumes them itself.
                if let BaoContentItem::Leaf(leaf) =
                    item.map_err(|source| BlobError::fetch(context(), source))?
                {
                    write_leaf(&mut file, leaf.offset, &leaf.data)
                        .await
                        .map_err(|source| BlobError::fetch(context(), source))?;
                    written = written.max(leaf.offset.saturating_add(leaf.data.len() as u64));
                    let now = Instant::now();
                    let cumulative = done_bytes.saturating_add(written.min(target.size));
                    if let Some(telemetry) = telemetry {
                        telemetry.observe_progress(now, cumulative);
                    }
                    if let Some(bytes_received) = progress.observe(now, cumulative) {
                        let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
                    }
                }
                curr = next;
            }
            fsm::BlobContentNext::Done(end) => break end,
        }
    };
    if let fsm::EndBlobNext::Closing(closing) = end.next() {
        closing
            .next()
            .await
            .map_err(|source| BlobError::fetch(context(), source))?;
    }

    file.flush()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    drop(file);

    // Every chunk was verified on arrival and the stream ran to completion, so
    // the file is whole and can take its real name.
    if !target.platform_descriptor {
        tokio::fs::rename(sink, &target.destination)
            .await
            .map_err(|source| BlobError::fetch(context(), source))?;
    }
    Ok(())
}

async fn write_leaf(file: &mut tokio::fs::File, offset: u64, data: &Bytes) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(offset)).await?;
    file.write_all(data).await
}

/// How much of a partial file can be kept: its length rounded down to a whole
/// chunk group, since that is the finest boundary a range request can start on.
async fn resumable_prefix_len(partial: &Path) -> u64 {
    let Ok(metadata) = tokio::fs::metadata(partial).await else {
        return 0;
    };
    metadata.len() / CHUNK_GROUP_BYTES * CHUNK_GROUP_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn a_missing_partial_resumes_from_the_start() {
        assert_eq!(
            resumable_prefix_len(Path::new("/nope/absent.part")).await,
            0
        );
    }

    /// A resumed request can only start on a chunk-group boundary, so a
    /// partial file is trusted only up to the last whole one.
    #[tokio::test]
    async fn a_partial_is_trusted_only_to_the_last_whole_chunk_group()
    -> std::result::Result<(), std::io::Error> {
        let dir = unique_dir("wisp-partial");
        tokio::fs::create_dir_all(&dir).await?;
        let partial = dir.join("f.part");

        tokio::fs::write(&partial, vec![0u8; (CHUNK_GROUP_BYTES + 7) as usize]).await?;
        assert_eq!(resumable_prefix_len(&partial).await, CHUNK_GROUP_BYTES);

        tokio::fs::write(&partial, vec![0u8; (CHUNK_GROUP_BYTES * 3) as usize]).await?;
        assert_eq!(resumable_prefix_len(&partial).await, CHUNK_GROUP_BYTES * 3);

        // Less than one group is worth nothing.
        tokio::fs::write(&partial, vec![0u8; 10]).await?;
        assert_eq!(resumable_prefix_len(&partial).await, 0);

        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(())
    }
}
