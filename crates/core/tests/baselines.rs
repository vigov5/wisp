//! Local baselines for the transfer performance plan (B1).
//!
//! These are measurements, not assertions: they print a number and pass. They
//! are `#[ignore]`d so a normal `cargo test` run stays fast and deterministic,
//! and they only mean anything under `--release`:
//!
//! ```text
//! cargo test --release -p wisp-core --test baselines -- --ignored --nocapture
//! ```
//!
//! The plan compares app payload throughput against these to decide whether a
//! transfer is limited by the link, the disk or the CPU. A number below 70% of
//! the relevant baseline is the threshold for investigating.

use std::io::Write;
use std::time::Instant;

use wisp_core::fs_plan::ConflictPolicy;
use wisp_core::protocol::message::{ManifestItem, TransferManifest};
use wisp_core::transfer::record::TransferRecord;

/// Payload size per baseline. Large enough to swamp setup costs, small enough
/// to stay friendly on a nearly-full disk.
const BYTES: usize = 512 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

fn mib_per_sec(bytes: usize, secs: f64) -> f64 {
    (bytes as f64 / secs) / (1024.0 * 1024.0)
}

/// BLAKE3 hashing throughput — the receiver verifies every byte it stores, so
/// this is a hard ceiling on transfer throughput regardless of the link.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_blake3_hash_throughput() {
    let chunk = vec![0x5au8; CHUNK];
    let mut hasher = blake3::Hasher::new();
    let start = Instant::now();
    for _ in 0..(BYTES / CHUNK) {
        hasher.update(&chunk);
    }
    let hash = hasher.finalize();
    let secs = start.elapsed().as_secs_f64();
    println!(
        "blake3 hash: {} MiB in {:.2}s = {:.1} MiB/s (digest {}...)",
        BYTES / (1024 * 1024),
        secs,
        mib_per_sec(BYTES, secs),
        &hash.to_hex()[..8]
    );
}

/// Sequential write to the real filesystem, including the `fsync` the record
/// writer pays. This is what "the disk cannot keep up" would look like.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_sequential_disk_write() {
    let dir = std::env::temp_dir().join("wisp-baseline-write");
    std::fs::create_dir_all(&dir).expect("create baseline dir");
    let path = dir.join("payload.bin");
    let chunk = vec![0x5au8; CHUNK];

    let start = Instant::now();
    {
        let mut file = std::fs::File::create(&path).expect("create baseline file");
        for _ in 0..(BYTES / CHUNK) {
            file.write_all(&chunk).expect("write chunk");
        }
        file.sync_all().expect("fsync");
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "disk write: {} MiB in {:.2}s = {:.1} MiB/s (fsync included)",
        BYTES / (1024 * 1024),
        secs,
        mib_per_sec(BYTES, secs)
    );

    // Read it straight back. The page cache is still warm, so treat this as an
    // upper bound on read throughput rather than a cold-cache figure.
    let start = Instant::now();
    let read = std::fs::read(&path).expect("read back");
    let secs = start.elapsed().as_secs_f64();
    println!(
        "disk read (warm cache): {} MiB in {:.2}s = {:.1} MiB/s",
        read.len() / (1024 * 1024),
        secs,
        mib_per_sec(read.len(), secs)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Cost of one resume-record checkpoint, which is what P0.2's throttle avoids
/// paying per progress event and what P0.3's atomic replacement adds.
///
/// Two shapes are timed on the same record: the shipped one (compact JSON into
/// a `create_new` temp file, then rename within the directory) and the
/// pre-P0.3 one (pretty JSON written straight over the destination). The
/// difference is what atomicity costs; the absolute number times the event rate
/// is what throttling saves.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_record_checkpoint_write() {
    const ITERATIONS: u32 = 200;
    /// A manifest big enough to be realistic for a folder transfer — record
    /// size drives serialisation cost, so a one-file manifest would flatter it.
    const MANIFEST_ITEMS: u32 = 200;

    let dir = std::env::temp_dir().join("wisp-baseline-record");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create baseline dir");

    let manifest = TransferManifest {
        items: (0..MANIFEST_ITEMS)
            .map(|i| ManifestItem::File {
                path: format!("some/nested/directory/file-{i:04}.bin"),
                size: 1024 * 1024,
            })
            .collect(),
    };
    let mut record = TransferRecord::new(
        [7u8; 32].into(),
        dir.join("out"),
        ConflictPolicy::Rename,
        manifest,
    );

    let start = Instant::now();
    for i in 0..ITERATIONS {
        record.bytes_received = u64::from(i) * 1024 * 1024;
        record.save(&dir).expect("atomic save");
    }
    let atomic_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);

    let direct_path = dir.join("record-direct.json");
    let start = Instant::now();
    for i in 0..ITERATIONS {
        record.bytes_received = u64::from(i) * 1024 * 1024;
        let content = serde_json::to_vec_pretty(&record).expect("serialize");
        let mut file = std::fs::File::create(&direct_path).expect("create");
        file.write_all(&content).expect("write");
    }
    let direct_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);

    let bytes = std::fs::metadata(dir.join("record.json"))
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "record checkpoint ({MANIFEST_ITEMS} items, {bytes} bytes compact): \
         atomic {atomic_us:.0} us/save, direct pretty {direct_us:.0} us/save"
    );
    println!(
        "  at 1 checkpoint/s a transfer pays {:.1} ms/min; \
         per progress event at 640 events/s it would be {:.1} ms/s",
        atomic_us * 60.0 / 1000.0,
        atomic_us * 640.0 / 1000.0
    );

    let _ = std::fs::remove_dir_all(&dir);
}
