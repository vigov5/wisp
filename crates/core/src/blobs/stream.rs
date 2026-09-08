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
use bytes::BytesMut;
use iroh_blobs::Hash;
use iroh_blobs::format::collection::{Collection, SimpleStore};
use iroh_blobs::get::fsm;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, trace};

use super::error::{BlobError, BlobTextError, Result};
use super::receive::{BlobDownloadUpdate, PROGRESS_EMIT_INTERVAL, ProgressCoalescer};
use super::source::BlobSource;
use super::telemetry::BlobTransferTelemetry;

/// Bytes per bao chunk group at iroh's block size (`BlockSize::from_chunk_log(4)`
/// - 16 chunks of 1 KiB). A resumed file has to restart on one of these
/// boundaries, because that is the finest granularity a range request can name.
const CHUNK_GROUP_BYTES: u64 = 16 * 1024;

/// Bytes of buffered file writes, coalescing the 16 KiB bao leaves the get fsm
/// yields into writes large enough that a syscall stops being the unit of work.
const WRITE_BUFFER_BYTES: usize = 512 * 1024;

/// Leaves allowed to sit between the network and the file.
///
/// Bounded so a fast link cannot outrun a slow disk into unbounded memory. At
/// 16 KiB a leaf this is 1 MiB in flight, which is ample to keep the writer fed
/// across a scheduling hiccup without being worth accounting for against the
/// receiver's memory budget.
const WRITE_QUEUE_LEAVES: usize = 64;

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

/// Largest blob [`SourceStore`] will hold in memory.
///
/// It only ever fetches the two small blobs a collection is made of, so this is
/// a bound on a manifest and not on a payload. It exists because the size comes
/// from the peer: without a cap, a provider claiming a huge blob would have us
/// allocate for it before a single byte was verified.
const MAX_COLLECTION_BLOB_BYTES: u64 = 1 << 20;

/// Reads blobs straight off whichever transport this transfer is using.
///
/// [`Collection::load`] needs somewhere to read the two small blobs a
/// collection is made of: its hash sequence, and the metadata naming each
/// entry. Without a local store, the provider itself is that somewhere.
struct SourceStore<'a>(&'a BlobSource);

impl SimpleStore for SourceStore<'_> {
    async fn load(&self, hash: Hash) -> n0_error::Result<Bytes> {
        collect_blob(self.0, hash)
            .await
            .map_err(|source| n0_error::anyerr!("fetching blob {hash}: {source}"))
    }
}

/// Fetches one whole blob into memory over a fresh stream pair.
///
/// `iroh-blobs` has `get_blob` for this, but it takes a QUIC connection — the
/// one thing a transport abstraction cannot hand it.
async fn collect_blob(source: &BlobSource, hash: Hash) -> Result<Bytes> {
    let context = || format!("collection blob {hash} over {}", source.label());
    let (recv, send) = source.open().await?;
    let connected = fsm::start_with_streams(recv, send, GetRequest::blob(hash), Default::default());
    let fsm::ConnectedNext::StartRoot(start_root) = connected
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?
    else {
        return Err(BlobError::fetch(
            context(),
            BlobTextError::new("expected the request to start at the root"),
        ));
    };
    let (mut curr, size) = start_root
        .next()
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    if size > MAX_COLLECTION_BLOB_BYTES {
        return Err(BlobError::fetch(
            context(),
            BlobTextError::new(format!(
                "collection blob claims {size} bytes, over the {MAX_COLLECTION_BLOB_BYTES} limit"
            )),
        ));
    }
    // Placed by offset rather than appended: a linear stream arrives in order,
    // but relying on that silently would corrupt the manifest if it ever did
    // not, and this is the blob that names every file.
    let mut out = BytesMut::zeroed(size as usize);
    let end = loop {
        match curr.next().await {
            fsm::BlobContentNext::More((next, item)) => {
                if let BaoContentItem::Leaf(leaf) =
                    item.map_err(|source| BlobError::fetch(context(), source))?
                {
                    let start = leaf.offset as usize;
                    let stop = start.saturating_add(leaf.data.len());
                    if stop > out.len() {
                        return Err(BlobError::fetch(
                            context(),
                            BlobTextError::new("collection blob wrote past its declared size"),
                        ));
                    }
                    out[start..stop].copy_from_slice(&leaf.data);
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
    Ok(out.freeze())
}

/// Fetches every target over `connection`, writing each file to its destination
/// as the bytes arrive.
///
/// Progress is a cumulative byte count across all targets, matching what the
/// store-backed path emits, so the caller's tracker does not care which path
/// produced it.
pub(super) async fn stream_collection(
    source: BlobSource,
    root_hash: Hash,
    targets: Vec<StreamTarget>,
    update_tx: mpsc::UnboundedSender<BlobDownloadUpdate>,
    telemetry: Option<&BlobTransferTelemetry>,
) -> Result<()> {
    let collection = Collection::load(root_hash, &SourceStore(&source))
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
            &source,
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
    source: &BlobSource,
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

    let file = tokio::fs::OpenOptions::new()
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
    // One pair per request. On QUIC that is a bi-stream on the existing
    // connection; on the LAN transport it is a fresh connection and handshake,
    // which is the trade `super::source` documents.
    let (recv, send) = source.open().await?;
    let connected = fsm::start_with_streams(recv, send, request, Default::default());
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
    // The file is written by its own task, so a leaf's disk write overlaps the
    // next leaf's arrival. Serialised, the two costs simply added: at 16 KiB a
    // leaf the loop paid a seek and a write - each a hop through tokio's
    // blocking pool - for every ~0.5 ms of network time, which pinned app
    // throughput near 19 MiB/s no matter how fast the link was. Measured phone
    // to phone: 67.7 MiB/s raw TCP, 33.3 raw QUIC reading the same file, 18.7
    // through here. Same mistake and same fix as `quic_baseline`'s file source
    // in `324e7bd`, which was never carried back to this path.
    let (leaf_tx, leaf_rx) = mpsc::channel::<(u64, Bytes)>(WRITE_QUEUE_LEAVES);
    let writer = tokio::spawn(write_leaves(file, leaf_rx));

    // `None` means the writer stopped before the stream did, which only a write
    // error causes; joining below names it.
    let end = loop {
        match curr.next().await {
            fsm::BlobContentNext::More((next, item)) => {
                // Parent hashes arrive interleaved with the data; they are what
                // let a suffix be verified, and the fsm consumes them itself.
                //
                // An error here returns without joining the writer. That is
                // deliberate: dropping `leaf_tx` on the way out closes the
                // channel, so the task flushes what it already has and exits,
                // and a longer verified prefix is exactly what a resume wants.
                // Nothing renames the partial on this path.
                if let BaoContentItem::Leaf(leaf) =
                    item.map_err(|source| BlobError::fetch(context(), source))?
                {
                    let leaf_end = leaf.offset.saturating_add(leaf.data.len() as u64);
                    if leaf_tx.send((leaf.offset, leaf.data)).await.is_err() {
                        break None;
                    }
                    // Progress now counts bytes *received* rather than bytes
                    // already durable, and may lead the file by up to the queue
                    // plus the buffer. Safe for both consumers: the UI only
                    // draws it, and a resume re-derives its own start from the
                    // file's whole chunk groups rather than from this number.
                    written = written.max(leaf_end);
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
            fsm::BlobContentNext::Done(end) => break Some(end),
        }
    };

    // Closing the channel is what tells the writer to flush and finish, so it
    // has to happen before the join.
    drop(leaf_tx);
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(source)) => return Err(BlobError::fetch(context(), source)),
        Err(join) => {
            return Err(BlobError::fetch(
                context(),
                BlobTextError::new(format!("file writer task failed: {join}")),
            ));
        }
    }
    let Some(end) = end else {
        return Err(BlobError::fetch(
            context(),
            BlobTextError::new("file writer stopped before the stream ended"),
        ));
    };
    if let fsm::EndBlobNext::Closing(closing) = end.next() {
        closing
            .next()
            .await
            .map_err(|source| BlobError::fetch(context(), source))?;
    }

    // Every chunk was verified on arrival and the stream ran to completion, so
    // the file is whole and can take its real name.
    if !target.platform_descriptor {
        tokio::fs::rename(sink, &target.destination)
            .await
            .map_err(|source| BlobError::fetch(context(), source))?;
    }
    Ok(())
}

/// Writes leaves to `file` until the channel closes, coalescing them through a
/// [`tokio::io::BufWriter`].
///
/// Leaves arrive in offset order for a linear stream, so the cursor is tracked
/// and a seek is issued only where an offset actually jumps — a ranged request
/// for a resumed file is the only thing that makes it jump, and then just once,
/// on the first leaf. Skipping the no-op seek is the point: seeking through a
/// `BufWriter` flushes it, so a seek per leaf would defeat the buffer entirely
/// and leave the syscall count exactly where it was.
async fn write_leaves(
    file: tokio::fs::File,
    mut rx: mpsc::Receiver<(u64, Bytes)>,
) -> std::io::Result<()> {
    let mut file = tokio::io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
    // `open` leaves the cursor at 0 whatever the resume offset is, so start
    // unknown and let the first leaf seek.
    let mut cursor: Option<u64> = None;
    while let Some((offset, data)) = rx.recv().await {
        if cursor != Some(offset) {
            file.seek(SeekFrom::Start(offset)).await?;
        }
        file.write_all(&data).await?;
        cursor = Some(offset.saturating_add(data.len() as u64));
    }
    file.flush().await
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
