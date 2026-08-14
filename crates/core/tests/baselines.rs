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
