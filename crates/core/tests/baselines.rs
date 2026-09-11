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

// --- where a per-file fetch spends its time ---------------------------------
//
// `blobs::stream::FetchSplit` measured a 1911-file folder on the phones and
// found 8.3 s of 15.6 s going to per-request overhead: 16.7 ms dialling, 9.0 ms
// waiting for the first response byte, 7.0 ms opening the destination. None of
// those numbers say *why*. Idle RTT between the two test phones is min 3.9 ms
// but averages 12-47 ms with an 80 ms mdev — Wi-Fi power save — so the network
// could account for all of it, or none. These three separate the two: they run
// the same work over loopback, where the only cost left is this device's CPU
// and its syscalls.

/// Cost of building the per-dial rustls config.
///
/// `lan_transport::dial_with_cap` calls `lan_tls::client_config` on every dial,
/// which rebuilds the whole `ClientConfig` — provider, cipher suites, verifier,
/// and a `CertifiedKey` that clones the secret and re-derives its SPKI. A
/// transfer dials once per file, so if this is milliseconds it is a per-file
/// tax that caching the config would simply delete.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_lan_tls_config_construction() {
    use wisp_core::lan_tls;

    const ITERATIONS: u32 = 2_000;
    let secret = iroh::SecretKey::generate();
    let peer = iroh::SecretKey::generate().public();

    // Once outside the loop: the first call pays for lazily initialised crypto
    // provider state, which no later dial pays again.
    let _ = lan_tls::client_config(&secret, peer).expect("client config");

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = lan_tls::client_config(&secret, peer).expect("client config");
    }
    let client_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = lan_tls::server_config(&secret).expect("server config");
    }
    let server_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(ITERATIONS);

    println!("lan tls config: client {client_us:.0} us, server {server_us:.0} us");
    println!(
        "  at one dial per file, 1911 files pay {:.0} ms of client config alone",
        client_us * 1911.0 / 1000.0
    );
}

/// TCP connect plus the TLS 1.3 handshake, over loopback.
///
/// Loopback removes the link, so what is left is this device's cost for a dial:
/// two syscalls, the handshake's ed25519 sign and verify on both sides, and the
/// config construction above. Subtract this from the 16.7 ms measured on Wi-Fi
/// and the remainder is the network's.
///
/// Measured both serially and eight at a time, because eight is what
/// `blobs::stream` actually does and a handshake is CPU work that contends.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_lan_tcp_dial_loopback() {
    use tokio::net::TcpListener;
    use wisp_core::lan_transport;

    const SERIAL: u32 = 200;
    const CONCURRENT_ROUNDS: u32 = 25;
    const WINDOW: u32 = 8;

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let server_secret = iroh::SecretKey::generate();
        let server_peer = server_secret.public();
        let client_secret = iroh::SecretKey::generate();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let target = listener.local_addr().expect("local addr");
        let accept_secret = server_secret.clone();
        // Mirrors `blobs::lan_provider`: accept, then handshake off the accept
        // loop, so a slow handshake never holds up the next connection.
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let secret = accept_secret.clone();
                tokio::spawn(async move {
                    let _ = lan_transport::accept(tcp, &secret).await;
                });
            }
        });

        // Warm: the first dial initialises crypto state the rest reuse.
        let _ = lan_transport::dial(target, &client_secret, server_peer).await;

        let start = Instant::now();
        for _ in 0..SERIAL {
            lan_transport::dial(target, &client_secret, server_peer)
                .await
                .expect("loopback dial");
        }
        let serial_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(SERIAL);

        let start = Instant::now();
        for _ in 0..CONCURRENT_ROUNDS {
            let mut dials = Vec::with_capacity(WINDOW as usize);
            for _ in 0..WINDOW {
                let secret = client_secret.clone();
                dials.push(tokio::spawn(async move {
                    lan_transport::dial(target, &secret, server_peer)
                        .await
                        .expect("loopback dial");
                }));
            }
            for dial in dials {
                dial.await.expect("dial task");
            }
        }
        let concurrent_us =
            start.elapsed().as_secs_f64() * 1e6 / f64::from(CONCURRENT_ROUNDS * WINDOW);

        println!(
            "lan tcp dial (loopback): serial {serial_us:.0} us/dial, \
             {WINDOW} at a time {concurrent_us:.0} us/dial"
        );
        println!(
            "  on Wi-Fi the same dial measured 16687 us, so the link accounts \
             for {:.0} us of it",
            16687.0 - serial_us
        );
    });
}

/// The four filesystem calls `blobs::stream` makes before its first request.
///
/// Each is a `tokio::fs` call, so each is a hop onto the blocking pool and back.
/// The phone measured 7.0 ms for the set, which for four stats and an open is
/// implausible as disk work — the question is whether it is pool scheduling
/// latency instead, and the answer is the gap between these two numbers.
#[test]
#[ignore = "baseline measurement; run explicitly under --release"]
fn baseline_sink_open_blocking_ops() {
    const SERIAL: u32 = 400;
    const CONCURRENT_ROUNDS: u32 = 50;
    const WINDOW: u32 = 8;

    let dir = std::env::temp_dir().join(format!(
        "wisp-sink-baseline-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create baseline dir");

    // Two populations, because the difference between them turned out to be
    // the whole answer. `hot` is one file reopened, which is what this test
    // measured first and why it read 40x faster than the phone: every lookup
    // after the first is cached. `cold` is a distinct file per open, which is
    // what a transfer actually does — 1911 files, each opened once, each a
    // fresh path resolution.
    let hot = dir.join("sink");
    std::fs::write(&hot, b"").expect("seed sink");
    let cold: Vec<std::path::PathBuf> = (0..SERIAL)
        .map(|i| {
            let path = dir.join(format!("cold-{i}"));
            std::fs::write(&path, b"").expect("seed cold sink");
            path
        })
        .collect();

    // The third population is the one the receiver actually opens. Android
    // hands each destination over as `/proc/self/fd/<n>`, and opening that
    // *path* is a fresh walk into MediaProvider's FUSE daemon which re-checks
    // permission against our uid — the cost `blobs::descriptor` exists to
    // avoid on the send side by duplicating the descriptor instead. The files
    // are held open for the length of the test so the numbers stay valid.
    #[cfg(unix)]
    let (held, descriptor_paths): (Vec<std::fs::File>, Vec<std::path::PathBuf>) = {
        use std::os::fd::AsRawFd;
        // Writable, because `open_once` reopens the path for writing exactly as
        // `stream_one` does, and reopening a read-only descriptor's path for
        // write would fail for a reason that has nothing to do with the cost
        // being measured.
        let held: Vec<std::fs::File> = cold
            .iter()
            .map(|path| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .expect("open cold sink")
            })
            .collect();
        let paths = held
            .iter()
            .map(|file| std::path::PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())))
            .collect();
        (held, paths)
    };

    async fn open_once(dir: &std::path::Path, path: &std::path::Path) {
        // Exactly what `stream_one` does for a platform descriptor.
        tokio::fs::create_dir_all(dir)
            .await
            .expect("create_dir_all");
        let resume_at = tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .await
            .expect("open");
        file.set_len(resume_at).await.expect("set_len");
    }

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        open_once(&dir, &hot).await;

        let start = Instant::now();
        for _ in 0..SERIAL {
            open_once(&dir, &hot).await;
        }
        let hot_serial_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(SERIAL);

        let start = Instant::now();
        for _ in 0..CONCURRENT_ROUNDS {
            let mut opens = Vec::with_capacity(WINDOW as usize);
            for _ in 0..WINDOW {
                let dir = dir.clone();
                let path = hot.clone();
                opens.push(tokio::spawn(async move { open_once(&dir, &path).await }));
            }
            for open in opens {
                open.await.expect("open task");
            }
        }
        let hot_concurrent_us =
            start.elapsed().as_secs_f64() * 1e6 / f64::from(CONCURRENT_ROUNDS * WINDOW);

        // Each path opened exactly once, in windows of `WINDOW`, which is how
        // `blobs::stream` meets its destinations.
        let start = Instant::now();
        for window in cold.chunks(WINDOW as usize) {
            let mut opens = Vec::with_capacity(window.len());
            for path in window {
                let dir = dir.clone();
                let path = path.clone();
                opens.push(tokio::spawn(async move { open_once(&dir, &path).await }));
            }
            for open in opens {
                open.await.expect("open task");
            }
        }
        let cold_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(SERIAL);

        println!(
            "sink open (4 tokio::fs calls): hot serial {hot_serial_us:.0} us, \
             hot {WINDOW}-at-a-time {hot_concurrent_us:.0} us, \
             cold {WINDOW}-at-a-time {cold_us:.0} us"
        );

        #[cfg(unix)]
        {
            let start = Instant::now();
            for window in descriptor_paths.chunks(WINDOW as usize) {
                let mut opens = Vec::with_capacity(window.len());
                for path in window {
                    let dir = dir.clone();
                    let path = path.clone();
                    opens.push(tokio::spawn(async move { open_once(&dir, &path).await }));
                }
                for open in opens {
                    open.await.expect("open task");
                }
            }
            let fd_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(SERIAL);
            println!("  via /proc/self/fd/<n>, {WINDOW} at a time: {fd_us:.0} us");
        }

        println!("  the phone measured 7016 us per file for this set");
    });
    // Held until here so every `/proc/self/fd/<n>` above named a live entry.
    #[cfg(unix)]
    drop(held);

    let _ = std::fs::remove_dir_all(&dir);
}
