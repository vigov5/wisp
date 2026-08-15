//! Raw QUIC transport baseline for the transfer performance plan (B1).
//!
//! Source/sink over the same iroh, encryption and path a real transfer uses,
//! with the blob store, BAO verification, record writes and export removed. The
//! gap between this and app payload throughput is what the layers above the
//! transport cost:
//!
//! ```text
//! transport_utilization = app_payload_throughput / raw_quic_throughput
//! ```
//!
//! Sink (prints a ticket, then waits):
//!
//! ```text
//! cargo run --release -p wisp-core --example quic_baseline -- sink
//! ```
//!
//! Source (sends `--mib` megabytes over one uni stream; `--from-file PATH`
//! reads from storage instead of memory, adding the sending side's read):
//!
//! ```text
//! cargo run --release -p wisp-core --example quic_baseline -- source <ticket> --mib 256
//! ```
//!
//! Both ends bind with `RelayMode::Disabled`, so the number is the direct-path
//! ceiling and compares against the app's relay-disabled arm. The transport
//! config mirrors `wisp_app::quic_keepalive::build_transport_config` — a
//! baseline built on quinn's 1.25 MB default stream window would be slower than
//! the app it is meant to bound, which would make it useless.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{QuicTransportConfig, VarInt};
use iroh::{Endpoint, RelayMode, endpoint::presets};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use wisp_core::util::{decode_ticket, make_ticket_offline};

const ALPN: &[u8] = b"wisp/quic-baseline/0";
const CHUNK: usize = 1024 * 1024;

/// Mirrors `wisp_app::quic_keepalive`: 16 MiB on desktop, 8 MiB on Android.
#[cfg(target_os = "android")]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 8 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 16 * 1024 * 1024;

fn transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        .stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW_BYTES))
        .send_window(8u64 * u64::from(STREAM_RECEIVE_WINDOW_BYTES))
        .build()
}

/// Prints the same stability shape the analyzer reports for a real transfer:
/// p10, p50 and the ratio the acceptance criteria use.
fn report_windows(windows: &[u64]) {
    if windows.len() < 3 {
        println!("windows: {} (too few for p10/p50)", windows.len());
        return;
    }
    let mut sorted: Vec<f64> = windows
        .iter()
        .map(|b| *b as f64 / (1024.0 * 1024.0))
        .collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in byte counts"));
    let pick = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let p10 = pick(0.10);
    let p50 = pick(0.50);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / sorted.len() as f64;
    println!(
        "  windows={} p10={p10:.1} p50={p50:.1} MiB/s p10/p50={:.0}% CV={:.2} idle_windows={}",
        sorted.len(),
        if p50 > 0.0 { p10 / p50 * 100.0 } else { 0.0 },
        if mean > 0.0 {
            variance.sqrt() / mean
        } else {
            0.0
        },
        sorted.iter().filter(|v| **v < 0.0625).count(),
    );
}

async fn bind() -> Result<Endpoint> {
    Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .transport_config(transport_config())
        .bind()
        .await
        .context("bind endpoint")
}

async fn run_sink() -> Result<()> {
    let endpoint = bind().await?;
    let ticket = make_ticket_offline(&endpoint).context("build ticket")?;
    println!("TICKET {ticket}");
    println!("waiting for one connection");

    let incoming = endpoint.accept().await.context("accept")?;
    let connection = incoming.await.context("handshake")?;
    let mut stream = connection.accept_uni().await.context("accept uni stream")?;

    let mut total = 0usize;
    let mut buf = vec![0u8; CHUNK];
    // Clock starts at the first byte, not at accept: connection setup is a
    // separate cost and the plan measures it separately.
    let mut start: Option<Instant> = None;
    // One-second windows, so this reports the same p10/p50 shape the analyzer
    // computes for a real transfer. An average alone cannot tell a steady link
    // apart from one that stalls and then catches up.
    let mut windows: Vec<u64> = Vec::new();
    let mut window_start: Option<Instant> = None;
    let mut window_bytes = 0u64;
    while let Some(n) = stream.read(&mut buf).await.context("read")? {
        let now = Instant::now();
        if start.is_none() {
            start = Some(now);
            window_start = Some(now);
        }
        total += n;
        window_bytes += n as u64;
        if let Some(w) = window_start
            && now.duration_since(w) >= Duration::from_secs(1)
        {
            windows.push(window_bytes);
            window_bytes = 0;
            window_start = Some(now);
        }
    }
    let secs = start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    let mib = total as f64 / (1024.0 * 1024.0);
    println!(
        "sink: {total} bytes ({mib:.0} MiB) in {secs:.2}s = {:.1} MiB/s",
        if secs > 0.0 { mib / secs } else { 0.0 }
    );
    report_windows(&windows);

    connection.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(())
}

async fn run_source(ticket: &str, mib: usize, from_file: Option<&str>) -> Result<()> {
    let addr = decode_ticket(ticket).context("decode ticket")?;
    let endpoint = bind().await?;
    let connection = endpoint.connect(addr, ALPN).await.context("connect")?;
    let mut stream = connection.open_uni().await.context("open uni stream")?;

    let start = Instant::now();
    match from_file {
        // Memory source: the transport is the only thing that can go idle.
        None => {
            let chunk = vec![0x5au8; CHUNK];
            for _ in 0..mib {
                stream.write_all(&chunk).await.context("write")?;
            }
        }
        // File source: adds the sending side's storage read. Comparing the two
        // is what separates "the phone's storage stalls the send" from "the
        // blob provider or the app runtime does" — the app shows occasional
        // `transport_idle` stalls that the memory source never reproduces.
        //
        // The read runs in its own task feeding a bounded channel, so a read
        // overlaps the previous chunk's send. A straight read-then-send loop
        // measures its own lack of pipelining instead: on a Pixel reading at
        // ~280 MiB/s — 13x the transfer rate, so nowhere near storage-bound —
        // serialising the two still cost 21%, which would have been misread as
        // the cost of touching storage at all.
        Some(path) => {
            let path = path.to_owned();
            let target = mib * CHUNK;
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
            let reader = tokio::spawn(async move {
                let mut file = tokio::fs::File::open(&path)
                    .await
                    .with_context(|| format!("open {path}"))?;
                let mut sent = 0usize;
                while sent < target {
                    let mut buf = vec![0u8; CHUNK.min(target - sent)];
                    let n = file.read(&mut buf).await.context("read source file")?;
                    if n == 0 {
                        // Wrap so a file smaller than --mib still fills the run.
                        file.seek(std::io::SeekFrom::Start(0))
                            .await
                            .context("rewind source file")?;
                        continue;
                    }
                    buf.truncate(n);
                    sent += n;
                    if tx.send(buf).await.is_err() {
                        break;
                    }
                }
                anyhow::Ok(())
            });
            while let Some(buf) = rx.recv().await {
                stream.write_all(&buf).await.context("write")?;
            }
            reader.await.context("source reader task")??;
        }
    }
    stream.finish().context("finish")?;
    // The sink's read loop ends when the stream closes; wait for the peer so
    // the source does not report a time that only measures filling buffers.
    connection.closed().await;
    let secs = start.elapsed().as_secs_f64();
    println!(
        "source: {mib} MiB in {secs:.2}s = {:.1} MiB/s",
        mib as f64 / secs
    );

    endpoint.close().await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("sink") => run_sink().await,
        Some("source") => {
            let ticket = args
                .get(2)
                .context("usage: quic_baseline source <ticket>")?;
            let mib = args
                .iter()
                .position(|a| a == "--mib")
                .and_then(|i| args.get(i + 1))
                .map(|v| v.parse::<usize>())
                .transpose()
                .context("--mib expects a number")?
                .unwrap_or(256);
            let from_file = args
                .iter()
                .position(|a| a == "--from-file")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str);
            run_source(ticket, mib, from_file).await
        }
        _ => bail!(
            "usage: quic_baseline sink | quic_baseline source <ticket> [--mib N] [--from-file PATH]"
        ),
    }
}
