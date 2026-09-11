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
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bao_tree::io::BaoContentItem;
use bytes::Bytes;
use bytes::BytesMut;
use futures_buffered::BufferedStreamExt;
use futures_lite::stream::{self, StreamExt};
use iroh_blobs::Hash;
use iroh_blobs::format::collection::{Collection, SimpleStore};
use iroh_blobs::get::fsm;
use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, GetRequest};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, trace};

use super::error::{BlobError, BlobTextError, Result, error_chain};
use super::receive::{BlobDownloadUpdate, PROGRESS_EMIT_INTERVAL, ProgressCoalescer};
use super::source::BlobSource;
use super::telemetry::BlobTransferTelemetry;

/// Ceiling on requests in flight at once, whatever the device.
const MAX_FETCH_SLICES: usize = 4;

/// Files named by one request, and the size at which one is closed early.
///
/// A batch is the unit a worker takes, so it is also the unit of imbalance.
/// Fixed slices of the file list were tried first and were badly wrong: the
/// 1911-file folder has a median file of 33 KB and a largest of 64.8 MB, with
/// 46% of its bytes in five files, so a quarter of the *list* was nowhere near
/// a quarter of the *work*. Whichever slice caught the big files ran on alone
/// and the phase sums fell to 36% of four times the wall clock — a window
/// two-thirds idle — while the wall went from 15.6 s to 37.7 s.
///
/// The byte cap is what keeps a batch from hiding a large file behind
/// sixty-three small ones; the file cap is what keeps a folder of tiny files
/// from paying a round trip each. Neither can split a single file, so the floor
/// is the largest file over the rate one stream sustains, and going finer than
/// that buys nothing.
const FETCH_BATCH_FILES: usize = 64;
const FETCH_BATCH_BYTES: u64 = 8 * 1024 * 1024;

/// How many pieces a collection's files are fetched in.
///
/// A slice is one `GetRequest` naming many children, so it costs one dial and
/// one round trip however many files it carries. This replaced a request per
/// file, which on the LAN transport meant a whole TCP connection and TLS
/// handshake each — `BlobSource::LanTcp` is dialled per request, and a device
/// log counted 3828 dials for two 1911-file transfers.
///
/// [`FetchSplit`] measured what that cost, P4 to P7, 1911 files / 408 MB in
/// 15.6 s, phase sums divided by the eight then in flight:
///
/// | phase | wall-equivalent | per file |
/// |---|---|---|
/// | body (data + verify) | 6.63 s | 27.7 ms |
/// | dial (connect + TLS) | 3.99 s | 16.7 ms |
/// | header (first round trip) | 2.15 s | 9.0 ms |
/// | sink (opening the destination) | 1.68 s | 7.0 ms |
/// | finish | 0.48 s | 2.0 ms |
///
/// Per-request overhead was 8.3 s of the 15.6 — a little over half. Two things
/// made it up, and one request per slice removes both:
///
/// - **Round trips.** `dial` is a TCP connect and a TLS handshake, `header` is
///   the wait for the first response byte (`AtConnected::next` writes the
///   request and reads nothing), so a file costs **three**. 5733 of them for
///   one folder. Loopback on a Pixel 7 dials in 590 us against 16.7 ms on
///   Wi-Fi, so 96.5% of a dial is the link, and the two phases agree on what
///   one round trip costs from opposite directions: 14.0 ms / 2 for the dial,
///   7.6 ms / 1 for the header.
/// - **The shape.** Eight futures polled from one task charge a future's wait
///   for its turn to whichever phase it is in. Running the same 1911 files over
///   loopback, where there is no link to wait for, still read 2.68 ms of dial,
///   1.39 ms of header and 3.55 ms of sink — about 2 s of the 15.6 that was
///   nothing but taking turns.
///
/// What is *not* in it, each refuted by measurement rather than reasoned away:
/// rebuilding the rustls config per dial (2-4 us), the disk (the writer's own
/// seeks and writes total 1.2 s for all 408 MB, 1% of the sum), verification
/// (inside `body`, which runs at 61.6 MB/s — 79% of the rig's raw ceiling), and
/// the destination open being a FUSE path walk (the same four calls cost 322 us
/// against a `/proc/self/fd/<n>` naming a distinct file, so the descriptor-dup
/// trick that fixed the send side does not apply here).
///
/// Four workers, and the transport agrees on the number. Raw throughput
/// between the test phones, fast direction
/// (`baselines::baseline_lan_stream_throughput`):
///
/// | streams | 1 | 2 | 4 | 8 |
/// |---|---|---|---|---|
/// | MiB/s | 63.6 | 77.3 | **79.0** | 77.1 |
///
/// So the link's aggregate ceiling is reached at two to four and falls off
/// after, which is exactly what the app measures: four workers fetch the
/// 1911-file folder in 10.31 s over three runs (10,198 / 10,504 / 10,229 ms)
/// and **eight in 10,973 ms**. Do not raise it on the theory that more
/// parallelism hides latency — the per-file round trips are gone, so there is
/// no latency left there to hide, and the extra streams only divide the same
/// bandwidth while adding contention on the per-file work (`sink` 2.7 s to
/// 2.95 s of wall).
///
/// The 18.7 MiB/s figure this comment used to cite for a single stream is
/// obsolete by 3.4x: it predates the writer task.
///
/// What the workers buy is cover for the per-file work rather than bandwidth —
/// one stream alone (63.6) already beats the 37.8 MiB/s the whole fetch
/// averages. In the *slow* direction concurrency buys nothing at all: 8.5 MiB/s
/// on one stream against 7.8 on four, a link-bound path where four workers are
/// 8% worse. Not enough to change the default, since that direction is bound by
/// the link whatever we do, but it is why this is a ceiling and not a target.
///
/// Each worker pulls [`FETCH_BATCH_FILES`]-sized batches from a shared cursor,
/// so the folder costs three round trips per batch — of the order of 200 for
/// 1911 files, against 5733 for one request each.
///
/// Two things are left, both sized:
///
/// - **`sink`, 2.72 s of the 10.31**, spent opening one destination between one
///   child and the next. Opening a cold distinct path eight at a time rather
///   than serially is 2.6x faster per file on the Pixel 7 and 5.0x on the
///   Pixel 4, so opening a batch's destinations up front — while its request is
///   in flight — is the next thing worth trying. It costs up to
///   [`FETCH_BATCH_FILES`] open handles per worker on top of the descriptors the
///   platform already holds, which wants checking against the receiver's own
///   budget first.
/// - **`body` runs at 58.6 MiB/s, 74% of the 79.0 the transport gives**, so
///   about 1.3 s of its 6.64 is the app's own per-leaf path. Not verification
///   and not the disk: blake3 on these phones is 1245 and 939 MiB/s, and a
///   sequential write with fsync is 239 and 96 — 21x and 4x what `body` needs.
///   What is left in it is the 16 KiB leaf hop through a channel to the writer
///   task, the progress bookkeeping, and the one task all the workers are
///   polled on.
fn fetch_slices() -> usize {
    MAX_FETCH_SLICES
}

/// Where a collection's fetch time went, summed over its files.
///
/// Sums, not wall time: with `slices` requests in flight a phase's sum can
/// reach `slices x` the wall clock, so what compares against the wall is
/// `sum / slices`. That comparison is the point — it says whether a phase
/// is what the transfer waits *on* or merely something it does. A folder of
/// small files can spend nearly all of its time before any byte arrives, and
/// no single throughput number tells that apart from a slow link; asking
/// "where did it go" twice without being able to answer it is what this is
/// for.
///
/// `write` overlaps every other phase deliberately (that is the writer task's
/// whole reason to exist), so it is the one figure that says nothing on its
/// own — compare it against `body`, which contains the backpressure it causes.
#[derive(Default)]
struct FetchSplit {
    files: AtomicU64,
    /// Preparing the destination: `create_dir_all`, `open`, the resume probe.
    sink_nanos: AtomicU64,
    /// [`BlobSource::open`] — on the LAN transport a connect and a TLS
    /// handshake per request, which is what this whole struct exists to size.
    dial_nanos: AtomicU64,
    /// Request written, root header read: the round trips before any data.
    header_nanos: AtomicU64,
    /// First leaf to last: the data, and the verification it passes through.
    body_nanos: AtomicU64,
    /// After the last leaf — draining the writer, closing, renaming.
    finish_nanos: AtomicU64,
    /// Inside the writer task: the seeks, writes and the final flush.
    write_nanos: AtomicU64,
}

impl FetchSplit {
    /// Charges the time since `since` to `counter` and returns the new mark, so
    /// a caller walking its phases never has to name an instant twice.
    fn charge(counter: &AtomicU64, since: Instant) -> Instant {
        let now = Instant::now();
        counter.fetch_add(
            u64::try_from(now.duration_since(since).as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        now
    }

    fn log(&self, wall: Duration, slices: usize, bytes: u64) {
        let ms = |counter: &AtomicU64| counter.load(Ordering::Relaxed) / 1_000_000;
        let files = self.files.load(Ordering::Relaxed);
        // Per file in microseconds: a mean dial in whole milliseconds rounds
        // the interesting range (tens of ms) to one significant figure.
        let mean_us = |counter: &AtomicU64| counter.load(Ordering::Relaxed) / 1_000 / files.max(1);
        debug!(
            files,
            slices,
            bytes,
            wall_ms = wall.as_millis(),
            sink_ms = ms(&self.sink_nanos),
            dial_ms = ms(&self.dial_nanos),
            header_ms = ms(&self.header_nanos),
            body_ms = ms(&self.body_nanos),
            finish_ms = ms(&self.finish_nanos),
            write_ms = ms(&self.write_nanos),
            dial_mean_us = mean_us(&self.dial_nanos),
            header_mean_us = mean_us(&self.header_nanos),
            "fetch.split"
        );
    }
}

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
            // The whole chain: `{:#}` renders one level, which for a nested
            // error means the context and not the cause. See `error_chain`.
            BlobError::fetch(
                format!("collection {root_hash}"),
                BlobTextError::new(error_chain(&source)),
            )
        })?;

    // Name to (request offset, hash).
    //
    // The root blob is a hash sequence whose entry 0 is the metadata blob and
    // whose entry `i + 1` is file `i`, and the provider resolves request offset
    // `k` as hash sequence entry `k - 1` (`provider::handle_get_impl`). So file
    // `i` is at offset `i + 2`, which `Collection::read_fsm_all` agrees with
    // from the other direction. The hash travels with the offset because
    // `AtStartChild::next` takes it, and validates the blob against it.
    let located: HashMap<&str, (u64, Hash)> = collection
        .iter()
        .enumerate()
        .map(|(index, (name, hash))| (name.as_str(), (index as u64 + 2, *hash)))
        .collect();

    // Resolved up front so the fetching below cannot fail on a lookup, which
    // keeps its only failure mode the fetch itself.
    let mut planned: Vec<PlannedFile> = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let (offset, hash) = *located.get(target.transfer_path.as_str()).ok_or_else(|| {
            BlobError::fetch(
                format!("collection {root_hash}"),
                BlobTextError::new(format!(
                    "missing file in collection: {}",
                    target.transfer_path
                )),
            )
        })?;
        planned.push(PlannedFile {
            offset,
            hash,
            index,
        });
    }
    // Ascending, because a provider walks a request's offsets in order and so
    // hands the children back in order. A slice of consecutive offsets is
    // therefore a slice the provider can serve without seeking about.
    planned.sort_unstable_by_key(|file| file.offset);
    let total_bytes: u64 = targets.iter().map(|target| target.size).sum();

    // Shared, because the slices do not finish in step.
    let received = AtomicU64::new(0);
    let progress = Mutex::new(ProgressCoalescer::new(PROGRESS_EMIT_INTERVAL));

    // Batched by size as well as by count; see [`FETCH_BATCH_BYTES`].
    let mut batches: Vec<Vec<PlannedFile>> = Vec::new();
    let mut batch: Vec<PlannedFile> = Vec::new();
    let mut batch_bytes = 0u64;
    for file in planned {
        batch_bytes = batch_bytes.saturating_add(targets[file.index].size);
        batch.push(file);
        if batch.len() >= FETCH_BATCH_FILES || batch_bytes >= FETCH_BATCH_BYTES {
            batches.push(std::mem::take(&mut batch));
            batch_bytes = 0;
        }
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    let workers = fetch_slices().min(batches.len()).max(1);
    trace!(
        files = targets.len(),
        batches = batches.len(),
        workers,
        "streaming collection"
    );
    let split = Arc::new(FetchSplit::default());
    let started = Instant::now();

    // A shared cursor rather than a batch each: a worker that draws a batch of
    // small files comes straight back for another, so no worker can be left
    // holding the transfer up while the others have nothing to do.
    let next_batch = AtomicUsize::new(0);
    let mut fetches = stream::iter(0..workers)
        .map(|_| async {
            loop {
                let index = next_batch.fetch_add(1, Ordering::Relaxed);
                let Some(batch) = batches.get(index) else {
                    return Ok(());
                };
                stream_slice(
                    &source, root_hash, &targets, batch, &received, &update_tx, &progress,
                    telemetry, &split,
                )
                .await?;
            }
        })
        .buffered_ordered(workers);
    // The first error is held rather than returned, so the split is logged for
    // a failed transfer too — which is when the question of where the time went
    // is usually being asked.
    let mut outcome = Ok(());
    while let Some(result) = fetches.next().await {
        if let Err(error) = result {
            outcome = Err(error);
            break;
        }
    }
    split.log(started.elapsed(), workers, total_bytes);
    outcome?;

    // Never let throttling hide the final position from the resume record.
    if let Some(bytes_received) = progress
        .lock()
        .expect("progress mutex is never held across a panic")
        .flush_pending(Instant::now())
    {
        let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
    }
    let _ = update_tx.send(BlobDownloadUpdate::Progress {
        bytes_received: total_bytes,
    });
    let _ = update_tx.send(BlobDownloadUpdate::Done);
    Ok(())
}

/// One file's place in the request that will carry it.
#[derive(Debug, Clone, Copy)]
struct PlannedFile {
    /// Offset in the get request; see `stream_collection` for how it is derived.
    offset: u64,
    /// The child's hash, which `AtStartChild::next` validates the blob against.
    hash: Hash,
    /// Index into the caller's `targets`.
    index: usize,
}

/// Fetches one slice of a collection over a single connection.
///
/// Every file in `slice` is named by one `GetRequest`, so the whole slice costs
/// one dial and one round trip rather than three per file, and the fsm walks
/// from child to child without returning to the network in between.
#[allow(clippy::too_many_arguments)]
async fn stream_slice(
    source: &BlobSource,
    root_hash: Hash,
    targets: &[StreamTarget],
    slice: &[PlannedFile],
    received: &AtomicU64,
    update_tx: &mpsc::UnboundedSender<BlobDownloadUpdate>,
    progress: &Mutex<ProgressCoalescer>,
    telemetry: Option<&BlobTransferTelemetry>,
    split: &Arc<FetchSplit>,
) -> Result<()> {
    let mut mark = Instant::now();
    let context = || format!("collection {root_hash}");

    // A request has to name every child's ranges before any of it is sent, so
    // the resume probes happen here rather than lazily. They are in the slice
    // rather than in `stream_collection` so the slices probe in parallel: a
    // folder's worth of sequential stats ahead of the first byte would be a
    // visible pause before anything moved.
    let mut wanted: HashMap<u64, (usize, Hash, u64)> = HashMap::with_capacity(slice.len());
    let mut builder = GetRequest::builder();
    for file in slice {
        let target = &targets[file.index];
        if !target.platform_descriptor && tokio::fs::metadata(&target.destination).await.is_ok() {
            // A file already at its real name is finished: it only got that
            // name after its last chunk verified. Left out of the request
            // entirely, and still contributing its bytes, because a skipped
            // file is a finished one and the total has to add up either way.
            trace!(path = %target.transfer_path, "destination already complete, skipping");
            received.fetch_add(target.size, Ordering::Relaxed);
            split.files.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let resume_at = resumable_prefix_len(sink_path(target)).await;
        if resume_at > 0 {
            debug!(path = %target.transfer_path, resume_at, "resuming partial file");
        }
        // Anything past the last whole chunk group cannot be named by a range
        // request, so it is refetched rather than trusted.
        builder = builder.offset(
            file.offset,
            if resume_at == 0 {
                ChunkRanges::all()
            } else {
                ChunkRanges::bytes(resume_at..)
            },
        );
        wanted.insert(file.offset, (file.index, file.hash, resume_at));
    }
    if wanted.is_empty() {
        FetchSplit::charge(&split.sink_nanos, mark);
        return Ok(());
    }
    let request = builder.build(root_hash);
    mark = FetchSplit::charge(&split.sink_nanos, mark);

    // One connection for the whole slice. On QUIC this is a bi-stream on the
    // existing connection; on the LAN transport it is a connect and a TLS
    // handshake, which is why there is one of these per slice and not per file.
    let (recv, send) = source.open().await?;
    mark = FetchSplit::charge(&split.dial_nanos, mark);

    let connected = fsm::start_with_streams(recv, send, request, Default::default());
    let mut pending = match connected
        .next()
        .await
        .map_err(|source| BlobError::fetch(context(), source))?
    {
        fsm::ConnectedNext::StartChild(child) => Some(child),
        // Offset 0 is the collection itself, which is already loaded and never
        // requested, so the provider has no reason to offer it.
        fsm::ConnectedNext::StartRoot(_) => {
            return Err(BlobError::fetch(
                context(),
                BlobTextError::new("the provider started at the collection root, unrequested"),
            ));
        }
        fsm::ConnectedNext::Closing(_) => None,
    };
    mark = FetchSplit::charge(&split.header_nanos, mark);

    while let Some(child) = pending {
        // Keyed by the offset the provider actually reached rather than by the
        // order asked for, which is what `AtStartChild::offset` is documented
        // to be for.
        let offset = child.offset();
        let Some(&(index, hash, resume_at)) = wanted.get(&offset) else {
            return Err(BlobError::fetch(
                context(),
                BlobTextError::new(format!("the provider offered offset {offset}, unrequested")),
            ));
        };
        let target = &targets[index];
        let file_context = || format!("{} ({hash})", target.transfer_path);

        let sink = sink_path(target);
        let file = open_sink(target, sink, resume_at).await?;
        mark = FetchSplit::charge(&split.sink_nanos, mark);

        let (mut curr, _size) = child
            .next(hash)
            .next()
            .await
            .map_err(|source| BlobError::fetch(file_context(), source))?;
        mark = FetchSplit::charge(&split.header_nanos, mark);

        let mut written = resume_at;
        // What this file has already added to `received`. Resumed bytes count
        // as received, exactly as they did when the caller accumulated them.
        let mut contributed = resume_at.min(target.size);
        received.fetch_add(contributed, Ordering::Relaxed);

        // The file is written by its own task, so a leaf's disk write overlaps
        // the next leaf's arrival. Serialised, the two costs simply added: at
        // 16 KiB a leaf the loop paid a seek and a write - each a hop through
        // tokio's blocking pool - for every ~0.5 ms of network time, which
        // pinned app throughput near 19 MiB/s no matter how fast the link was.
        // Measured phone to phone: 67.7 MiB/s raw TCP, 33.3 raw QUIC reading
        // the same file, 18.7 through here. Same mistake and same fix as
        // `quic_baseline`'s file source in `324e7bd`, which was never carried
        // back to this path.
        let (leaf_tx, leaf_rx) = mpsc::channel::<(u64, Bytes)>(WRITE_QUEUE_LEAVES);
        let writer = tokio::spawn(write_leaves(file, leaf_rx, Arc::clone(split)));

        // `None` means the writer stopped before the stream did, which only a
        // write error causes; joining below names it.
        let end = loop {
            match curr.next().await {
                fsm::BlobContentNext::More((next, item)) => {
                    // Parent hashes arrive interleaved with the data; they are
                    // what let a suffix be verified, and the fsm consumes them
                    // itself.
                    //
                    // An error here returns without joining the writer. That is
                    // deliberate: dropping `leaf_tx` on the way out closes the
                    // channel, so the task flushes what it already has and
                    // exits, and a longer verified prefix is exactly what a
                    // resume wants. Nothing renames the partial on this path.
                    if let BaoContentItem::Leaf(leaf) =
                        item.map_err(|source| BlobError::fetch(file_context(), source))?
                    {
                        let leaf_end = leaf.offset.saturating_add(leaf.data.len() as u64);
                        if leaf_tx.send((leaf.offset, leaf.data)).await.is_err() {
                            break None;
                        }
                        // Progress now counts bytes *received* rather than bytes
                        // already durable, and may lead the file by up to the
                        // queue plus the buffer. Safe for both consumers: the UI
                        // only draws it, and a resume re-derives its own start
                        // from the file's whole chunk groups rather than from
                        // this number.
                        written = written.max(leaf_end);
                        let now = Instant::now();
                        let reached = written.min(target.size);
                        let cumulative = if reached > contributed {
                            let delta = reached - contributed;
                            contributed = reached;
                            received.fetch_add(delta, Ordering::Relaxed) + delta
                        } else {
                            received.load(Ordering::Relaxed)
                        };
                        if let Some(telemetry) = telemetry {
                            telemetry.observe_progress(now, cumulative);
                        }
                        let emit = progress
                            .lock()
                            .expect("progress mutex is never held across a panic")
                            .observe(now, cumulative);
                        if let Some(bytes_received) = emit {
                            let _ = update_tx.send(BlobDownloadUpdate::Progress { bytes_received });
                        }
                    }
                    curr = next;
                }
                fsm::BlobContentNext::Done(end) => break Some(end),
            }
        };
        mark = FetchSplit::charge(&split.body_nanos, mark);

        // Closing the channel is what tells the writer to flush and finish, so
        // it has to happen before the join.
        drop(leaf_tx);
        match writer.await {
            Ok(Ok(())) => {}
            Ok(Err(source)) => return Err(BlobError::fetch(file_context(), source)),
            Err(join) => {
                return Err(BlobError::fetch(
                    file_context(),
                    BlobTextError::new(format!("file writer task failed: {join}")),
                ));
            }
        }
        let Some(end) = end else {
            return Err(BlobError::fetch(
                file_context(),
                BlobTextError::new("file writer stopped before the stream ended"),
            ));
        };

        // Every chunk was verified on arrival and the stream ran to completion,
        // so the file is whole and can take its real name.
        if !target.platform_descriptor {
            tokio::fs::rename(sink, &target.destination)
                .await
                .map_err(|source| BlobError::fetch(file_context(), source))?;
        }
        // Topped up so the invariant the caller relies on holds by
        // construction: a finished file has contributed exactly its manifest
        // size, whatever the leaves added up to. Without it a blob whose real
        // length disagreed with the manifest would leave the shared counter
        // short, and a progress bar that stops at 98% is a bug report.
        if target.size > contributed {
            received.fetch_add(target.size - contributed, Ordering::Relaxed);
        }
        split.files.fetch_add(1, Ordering::Relaxed);

        pending = match end.next() {
            fsm::EndBlobNext::MoreChildren(more) => Some(more),
            fsm::EndBlobNext::Closing(closing) => {
                closing
                    .next()
                    .await
                    .map_err(|source| BlobError::fetch(context(), source))?;
                None
            }
        };
        mark = FetchSplit::charge(&split.finish_nanos, mark);
    }

    Ok(())
}

/// Where a target's bytes are written.
///
/// A descriptor is already the final location and is written in place; a path
/// is built under the record dir and renamed onto its destination at the end,
/// so a file at its real name is finished by construction.
fn sink_path(target: &StreamTarget) -> &Path {
    if target.platform_descriptor {
        &target.destination
    } else {
        &target.partial
    }
}

/// Opens `sink` for writing and trims it back to `resume_at`.
async fn open_sink(target: &StreamTarget, sink: &Path, resume_at: u64) -> Result<tokio::fs::File> {
    let context = || target.transfer_path.clone();
    if !target.platform_descriptor {
        // Only a path needs its parents; a descriptor's is not a directory that
        // can be created, and the destination's parent is what the rename at
        // the end lands in.
        if let Some(parent) = target.destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| BlobError::fetch(context(), source))?;
        }
        if let Some(parent) = sink.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| BlobError::fetch(context(), source))?;
        }
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(sink)
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    file.set_len(resume_at)
        .await
        .map_err(|source| BlobError::fetch(context(), source))?;
    Ok(file)
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
    split: Arc<FetchSplit>,
) -> std::io::Result<()> {
    let mut file = tokio::io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
    // `open` leaves the cursor at 0 whatever the resume offset is, so start
    // unknown and let the first leaf seek.
    let mut cursor: Option<u64> = None;
    while let Some((offset, data)) = rx.recv().await {
        // Timed from the leaf in hand, so the wait for the next one is not
        // charged to the disk: this task is idle most of a fast transfer, and
        // counting that idleness as write time would make disk look like the
        // bottleneck on every link.
        let mark = Instant::now();
        if cursor != Some(offset) {
            file.seek(SeekFrom::Start(offset)).await?;
        }
        file.write_all(&data).await?;
        cursor = Some(offset.saturating_add(data.len() as u64));
        FetchSplit::charge(&split.write_nanos, mark);
    }
    let mark = Instant::now();
    let flushed = file.flush().await;
    FetchSplit::charge(&split.write_nanos, mark);
    flushed
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

    /// The whole streaming path, over a real LAN provider, with more files
    /// than the concurrency window.
    ///
    /// This is the first end-to-end test of it, added with the change that
    /// made the fetch concurrent — the part worth pinning is not that files
    /// arrive but that the *accounting* survives them arriving out of order.
    /// Sequentially the caller kept a running `done_bytes` and each file added
    /// its own progress on top; concurrently that is a shared counter fed by
    /// deltas, which is exactly the kind of bookkeeping that silently
    /// double-counts or loses a file and shows up as a progress bar that
    /// overshoots or never reaches the end.
    #[tokio::test]
    async fn a_collection_streams_concurrently_and_accounts_for_every_byte() {
        use crate::blobs::source::LanTarget;
        use iroh::SecretKey;
        use iroh_blobs::format::collection::Collection;
        use iroh_blobs::store::mem::MemStore;
        use std::net::{Ipv4Addr, SocketAddr};

        // Deliberately above MAX_FETCH_CONCURRENCY so the window is actually
        // full, and of differing sizes so the files cannot finish in step.
        const FILES: usize = 20;

        let store = MemStore::new();
        let sender = SecretKey::generate();
        let receiver = SecretKey::generate();

        let mut collection = Collection::default();
        let mut expected: Vec<(String, Vec<u8>)> = Vec::with_capacity(FILES);
        for index in 0..FILES {
            // Sizes straddle the 16 KiB leaf boundary so some files take one
            // content item and others several.
            let len = 1 + (index * 4096) % 70_000;
            let body = vec![b'a' + (index % 26) as u8; len];
            let tag = store
                .add_bytes(bytes::Bytes::from(body.clone()))
                .temp_tag()
                .await
                .expect("adding a blob");
            let name = format!("dir{}/file{index:02}.bin", index % 3);
            collection.extend([(name.clone(), tag.hash())]);
            expected.push((name, body));
        }
        let root = collection
            .store(store.as_ref())
            .await
            .expect("storing the collection");

        let provider = super::super::lan_provider::LanBlobProvider::start(
            store.as_ref().clone(),
            sender.clone(),
            receiver.public(),
        )
        .await
        .expect("the provider should bind");
        let source = BlobSource::LanTcp(Box::new(LanTarget {
            target: SocketAddr::from((Ipv4Addr::LOCALHOST, provider.port())),
            secret: receiver.clone(),
            peer: sender.public(),
        }));

        let root_dir = unique_dir("wisp-stream-concurrent");
        let parts = root_dir.join("parts");
        std::fs::create_dir_all(&parts).expect("parts dir");
        let targets = expected
            .iter()
            .map(|(name, body)| StreamTarget {
                transfer_path: name.clone(),
                destination: root_dir.join(name),
                partial: parts.join(name),
                platform_descriptor: false,
                size: body.len() as u64,
            })
            .collect::<Vec<_>>();
        let total: u64 = targets.iter().map(|target| target.size).sum();

        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        stream_collection(source, root.hash(), targets, update_tx, None)
            .await
            .expect("the collection should stream");

        // Every file, byte for byte, at its real name.
        for (name, body) in &expected {
            let landed = std::fs::read(root_dir.join(name))
                .unwrap_or_else(|error| panic!("{name} should exist: {error}"));
            assert_eq!(landed, *body, "{name} should match what was sent");
        }

        // And the accounting.
        let mut progress = Vec::new();
        let mut done = false;
        while let Ok(update) = update_rx.try_recv() {
            match update {
                BlobDownloadUpdate::Progress { bytes_received } => {
                    // Double counting shows up here: two files adding the same
                    // bytes would carry the running total past the end.
                    assert!(
                        bytes_received <= total,
                        "progress {bytes_received} ran past the total {total}"
                    );
                    progress.push(bytes_received);
                }
                BlobDownloadUpdate::Done => done = true,
                BlobDownloadUpdate::Failed { error } => {
                    panic!("unexpected failure: {error}")
                }
            }
        }
        assert!(done, "the stream should report Done");
        // The last Progress is the closing announcement, sent unconditionally,
        // so it would read as the total even if the counter had lost a file.
        // The one before it is the counter's own value: reaching the total
        // there means every file ran and contributed.
        assert!(
            progress.len() >= 2,
            "expected accounted progress, got {progress:?}"
        );
        assert_eq!(
            progress[progress.len() - 2],
            total,
            "the counter must reach the total on its own, got {progress:?}"
        );

        let _ = std::fs::remove_dir_all(&root_dir);
    }

    /// A slice carries full blobs and resumed ones in the same request.
    ///
    /// Resume used to be a ranged request of its own, one per file; now every
    /// file's ranges ride in the slice's single `GetRequest`, so a wrong offset
    /// no longer fails loudly on its own connection — it writes the wrong bytes
    /// into a file that then passes as finished. The unaligned prefixes are the
    /// point: `resumable_prefix_len` rounds down to a whole chunk group, so the
    /// bytes past that boundary must be refetched *and* the stale tail
    /// truncated away.
    #[tokio::test]
    async fn a_slice_resumes_partial_files_alongside_whole_ones() {
        use crate::blobs::source::LanTarget;
        use iroh::SecretKey;
        use iroh_blobs::format::collection::Collection;
        use iroh_blobs::store::mem::MemStore;
        use std::net::{Ipv4Addr, SocketAddr};

        // More than `MAX_FETCH_SLICES` so resumed and whole files land in the
        // same slice as well as in different ones.
        const FILES: usize = 16;

        let store = MemStore::new();
        let sender = SecretKey::generate();
        let receiver = SecretKey::generate();

        let mut collection = Collection::default();
        let mut expected: Vec<(String, Vec<u8>)> = Vec::with_capacity(FILES);
        for index in 0..FILES {
            // Straddling the chunk group, so some files have a resumable prefix
            // and some are too small to have one at all.
            let len = 1 + index * (CHUNK_GROUP_BYTES as usize) / 2;
            let body: Vec<u8> = (0..len).map(|byte| (byte % 251) as u8).collect();
            let tag = store
                .add_bytes(bytes::Bytes::from(body.clone()))
                .temp_tag()
                .await
                .expect("adding a blob");
            let name = format!("dir{}/file{index:02}.bin", index % 3);
            collection.extend([(name.clone(), tag.hash())]);
            expected.push((name, body));
        }
        let root = collection
            .store(store.as_ref())
            .await
            .expect("storing the collection");

        let provider = super::super::lan_provider::LanBlobProvider::start(
            store.as_ref().clone(),
            sender.clone(),
            receiver.public(),
        )
        .await
        .expect("the provider should bind");
        let source = BlobSource::LanTcp(Box::new(LanTarget {
            target: SocketAddr::from((Ipv4Addr::LOCALHOST, provider.port())),
            secret: receiver.clone(),
            peer: sender.public(),
        }));

        // Written in place, as an Android receive is: the destination exists
        // before the transfer and keeps whatever a previous attempt left.
        let root_dir = unique_dir("wisp-stream-resume");
        let mut partials = 0usize;
        let targets = expected
            .iter()
            .enumerate()
            .map(|(index, (name, body))| {
                let destination = root_dir.join(name);
                std::fs::create_dir_all(destination.parent().expect("a parent"))
                    .expect("destination dir");
                // Every third file starts with a prefix a previous attempt
                // wrote, deliberately not on a chunk group boundary.
                let prefix = if index % 3 == 0 {
                    let unaligned = (CHUNK_GROUP_BYTES as usize + 1_000).min(body.len());
                    if unaligned > CHUNK_GROUP_BYTES as usize {
                        partials += 1;
                        unaligned
                    } else {
                        0
                    }
                } else {
                    0
                };
                // Only the whole chunk group is the *true* prefix; the bytes
                // past it are garbage. `resumable_prefix_len` trusts the group
                // and nothing after, so a correct resume refetches from the
                // boundary and overwrites this — and a resume that trusted the
                // whole file, or never happened, leaves it visible.
                let mut seeded = body[..prefix].to_vec();
                for byte in seeded.iter_mut().skip(CHUNK_GROUP_BYTES as usize) {
                    *byte = !*byte;
                }
                std::fs::write(&destination, &seeded).expect("seed destination");
                StreamTarget {
                    transfer_path: name.clone(),
                    destination,
                    partial: root_dir.join("parts").join(name),
                    platform_descriptor: true,
                    size: body.len() as u64,
                }
            })
            .collect::<Vec<_>>();
        assert!(
            partials > 0,
            "the fixture must actually produce partial files"
        );
        let total: u64 = targets.iter().map(|target| target.size).sum();

        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        stream_collection(source, root.hash(), targets, update_tx, None)
            .await
            .expect("the collection should stream");

        for (name, body) in &expected {
            let landed = std::fs::read(root_dir.join(name))
                .unwrap_or_else(|error| panic!("{name} should exist: {error}"));
            assert_eq!(
                landed.len(),
                body.len(),
                "{name} should be exactly its own length, not a resumed prefix plus a tail"
            );
            assert_eq!(landed, *body, "{name} should match what was sent");
        }

        // The accounting has to hold across a resume too: a file that only
        // needed its tail still contributes its whole manifest size.
        let mut progress = Vec::new();
        while let Ok(update) = update_rx.try_recv() {
            match update {
                BlobDownloadUpdate::Progress { bytes_received } => {
                    assert!(
                        bytes_received <= total,
                        "progress {bytes_received} ran past the total {total}"
                    );
                    progress.push(bytes_received);
                }
                BlobDownloadUpdate::Done => {}
                BlobDownloadUpdate::Failed { error } => panic!("unexpected failure: {error}"),
            }
        }
        assert_eq!(
            progress.last().copied(),
            Some(total),
            "the transfer should finish accounted for, got {progress:?}"
        );

        let _ = std::fs::remove_dir_all(&root_dir);
    }

    /// Runs a real collection fetch over loopback and prints its [`FetchSplit`].
    ///
    /// The point is the `sink` phase. On the phones it measured 7.0 ms a file,
    /// and every isolated explanation was refuted by
    /// `baselines::baseline_sink_open_blocking_ops`: the four `tokio::fs` calls
    /// cost 168 us on ext4, 337 us on FUSE, 322 us when the path is a
    /// `/proc/self/fd/<n>` naming a distinct file. What that baseline does not
    /// reproduce is the *shape* — it spawns each open as its own task, while
    /// `stream_collection` polls all eight futures from one, so a future waiting
    /// for its turn is charged to whatever phase it is in.
    ///
    /// Loopback and tiny files leave almost nothing else in the measurement: no
    /// Wi-Fi round trips, barely any data. A `sink` still near 7 ms here means
    /// the shape; a `sink` near the baseline means it takes the real transfer's
    /// concurrent network and verification load to appear.
    ///
    /// ```text
    /// cargo test --release -p wisp-core --lib -- --ignored --nocapture \
    ///     blobs::stream::tests::measure_a_collection_fetch_split
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "measurement; run explicitly under --release"]
    async fn measure_a_collection_fetch_split() {
        use crate::blobs::source::LanTarget;
        use iroh::SecretKey;
        use iroh_blobs::format::collection::Collection;
        use iroh_blobs::store::mem::MemStore;
        use std::net::{Ipv4Addr, SocketAddr};

        // The count the phones ran, so `sink`'s per-file mean is comparable.
        const FILES: usize = 1911;

        // Sizes with the real folder's *shape*, scaled to a tenth so this stays
        // a second of work rather than 408 MB of it. That folder's median file
        // is 33 KB and its largest 64.8 MB, with 46% of every byte in five
        // files, and an even split of the file *list* is therefore nowhere near
        // an even split of the work — which is exactly what a uniform fixture
        // cannot show. The five are adjacent on purpose: consecutive offsets
        // are the worst case for anything that carves the list into runs.
        const HUGE: [usize; 5] = [2_018_186, 2_251_491, 2_409_257, 5_552_903, 6_481_895];
        const TAIL: usize = 600_000;
        const SMALL: usize = 3_300;
        fn body_len(index: usize) -> usize {
            match index {
                900..905 => HUGE[index - 900],
                // The p99 tier, spread through the list.
                _ if index % 73 == 0 => TAIL,
                _ => SMALL,
            }
        }

        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(
                "warn,wisp_core::blobs::stream=debug",
            ))
            .with_test_writer()
            .with_ansi(false)
            .try_init();

        let store = MemStore::new();
        let sender = SecretKey::generate();
        let receiver = SecretKey::generate();

        let mut collection = Collection::default();
        let mut names: Vec<(String, u64)> = Vec::with_capacity(FILES);
        for index in 0..FILES {
            let len = body_len(index);
            let body = vec![b'a' + (index % 26) as u8; len];
            let tag = store
                .add_bytes(bytes::Bytes::from(body))
                .temp_tag()
                .await
                .expect("adding a blob");
            // Spread over directories the way a real folder is, so the
            // destination opens are not all in one hot directory.
            let name = format!("dir{}/file{index:04}.bin", index % 32);
            collection.extend([(name.clone(), tag.hash())]);
            names.push((name, len as u64));
        }
        let root = collection
            .store(store.as_ref())
            .await
            .expect("storing the collection");

        let provider = super::super::lan_provider::LanBlobProvider::start(
            store.as_ref().clone(),
            sender.clone(),
            receiver.public(),
        )
        .await
        .expect("the provider should bind");
        let source = BlobSource::LanTcp(Box::new(LanTarget {
            target: SocketAddr::from((Ipv4Addr::LOCALHOST, provider.port())),
            secret: receiver.clone(),
            peer: sender.public(),
        }));

        // `platform_descriptor`, because that is what an Android receive is:
        // the destination already exists and is written in place, with no
        // rename at the end. Pre-created here the way `createReceiveDestinations`
        // does it on the device.
        let root_dir = unique_dir("wisp-stream-measure");
        let targets = names
            .iter()
            .map(|(name, len)| {
                let destination = root_dir.join(name);
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).expect("destination dir");
                }
                std::fs::write(&destination, b"").expect("seed destination");
                StreamTarget {
                    transfer_path: name.clone(),
                    destination,
                    partial: root_dir.join("parts").join(name),
                    platform_descriptor: true,
                    size: *len,
                }
            })
            .collect::<Vec<_>>();
        let total: u64 = targets.iter().map(|target| target.size).sum();

        let (update_tx, _update_rx) = mpsc::unbounded_channel();
        let started = Instant::now();
        stream_collection(source, root.hash(), targets, update_tx, None)
            .await
            .expect("the collection should stream");
        let wall = started.elapsed();

        println!(
            "loopback collection: {FILES} files, {total} bytes, wall {} ms \
             (see the fetch.split line above for the phases)",
            wall.as_millis()
        );

        let _ = std::fs::remove_dir_all(&root_dir);
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
