//! Raw TCP link baseline for the transfer performance plan (B1's denominator).
//!
//! `quic_baseline` bounds the app against the *transport*; this bounds the
//! transport against the *link*. Together they split a shortfall three ways:
//!
//! ```text
//! link_utilization      = app_payload_throughput / tcp_throughput
//! transport_utilization = app_payload_throughput / raw_quic_throughput
//! quic_tax              = raw_quic_throughput   / tcp_throughput
//! ```
//!
//! That last term is the one the plan kept assuming rather than measuring, and
//! it is not a constant: on a USB tether raw QUIC reached 74% of TCP, and on a
//! 74 MiB/s Wi-Fi link the same code reached 56%. Userspace QUIC costs CPU per
//! byte, so the tax grows with the rate — a baseline taken on a slow link
//! understates it.
//!
//! Sink (thread per connection, reports each one, runs until killed):
//!
//! ```text
//! cargo run --release -p wisp-core --example tcp_baseline -- sink [PORT]
//! ```
//!
//! Source (one stream) and `multi` (N parallel streams sharing MIB):
//!
//! ```text
//! cargo run --release -p wisp-core --example tcp_baseline -- source HOST:PORT 512
//! cargo run --release -p wisp-core --example tcp_baseline -- multi HOST:PORT 4 512
//! ```
//!
//! `multi` separates a **per-flow** limit from an **aggregate** one. A link
//! whose single-stream rate is bound by loss or latency (a congestion-window
//! limit) multiplies with parallel flows; a genuinely saturated link does not.
//! The distinction matters when comparing against a peer that opens several
//! connections — LocalSend uploads two files at a time, so on a per-flow-limited
//! path it would read as twice as fast as a single-stream transfer without
//! moving any more air.
//!
//! **Do not substitute `nc` for this.** Android's toybox netcat silently stops
//! after 512 KiB when fed a large stream, reporting no error: it makes a slow
//! link look fast and a broken measurement look finished. Every "TCP through
//! `nc`" number predating this file is worth re-taking.
//!
//! Deliberately dependency-free beyond `std` and blocking rather than async:
//! the point is a ceiling the harness itself cannot depress, and a threaded
//! read/write loop leaves nothing to tune. Cross-compiles for
//! `aarch64-linux-android` with only the NDK linker set — no `CC`/`AR`, unlike
//! `quic_baseline`, whose `ring` and `blake3` need a C toolchain.

use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};

/// Read and write granularity. Large enough that the syscall rate is never the
/// thing being measured, small enough to stay well inside a socket buffer.
const CHUNK: usize = 256 * 1024;

fn mib_per_sec(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    bytes as f64 / (1024.0 * 1024.0) / secs
}

/// Sends `bytes` on one fresh connection and returns what it managed to write.
fn send_stream(addr: &str, bytes: u64) -> Result<u64> {
    let buf = vec![0u8; CHUNK];
    let mut stream = TcpStream::connect(addr).with_context(|| format!("connecting to {addr}"))?;
    stream.set_nodelay(true)?;
    let mut sent = 0u64;
    while sent < bytes {
        let n = ((bytes - sent) as usize).min(CHUNK);
        stream.write_all(&buf[..n])?;
        sent += n as u64;
    }
    stream.flush()?;
    Ok(sent)
}

/// Drains one connection to EOF and reports what it carried.
fn drain_connection(mut stream: TcpStream) -> Result<()> {
    stream.set_nodelay(true)?;
    let peer = stream.peer_addr()?;
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    // The clock starts on the first byte, not on accept, so a slow handshake or
    // a connection parked before its payload does not depress the rate.
    let mut started: Option<Instant> = None;
    loop {
        match stream.read(&mut buf)? {
            0 => break,
            n => {
                started.get_or_insert_with(Instant::now);
                total += n as u64;
            }
        }
    }
    let secs = started.map_or(0.0, |t| t.elapsed().as_secs_f64());
    println!(
        "RECV {peer} {total} bytes {secs:.3} s {:.2} MiB/s",
        mib_per_sec(total, secs)
    );
    Ok(())
}

fn run_sink(port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .with_context(|| format!("binding sink on port {port}"))?;
    println!("sink listening on 0.0.0.0:{port}");
    for stream in listener.incoming() {
        let stream = stream?;
        // A thread per connection so `multi` runs are actually concurrent; a
        // sequential accept loop would serialize them and report a per-flow
        // rate as if it were the aggregate.
        thread::spawn(move || {
            if let Err(error) = drain_connection(stream) {
                eprintln!("sink connection failed: {error:#}");
            }
        });
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("sink") => {
            let port = match args.get(2) {
                Some(p) => p.parse().context("PORT must be a u16")?,
                None => 5201,
            };
            run_sink(port)
        }
        Some("source") => {
            let addr = args.get(2).context("usage: source HOST:PORT [MIB]")?;
            let mib: u64 = match args.get(3) {
                Some(m) => m.parse().context("MIB must be a number")?,
                None => 512,
            };
            let start = Instant::now();
            let sent = send_stream(addr, mib * 1024 * 1024)?;
            let secs = start.elapsed().as_secs_f64();
            println!(
                "SENT {sent} bytes {secs:.3} s {:.2} MiB/s",
                mib_per_sec(sent, secs)
            );
            Ok(())
        }
        Some("multi") => {
            let addr = args
                .get(2)
                .context("usage: multi HOST:PORT STREAMS [MIB]")?
                .clone();
            let streams: u64 = args
                .get(3)
                .context("usage: multi HOST:PORT STREAMS [MIB]")?
                .parse()
                .context("STREAMS must be a number")?;
            if streams == 0 {
                bail!("STREAMS must be at least 1");
            }
            let mib: u64 = match args.get(4) {
                Some(m) => m.parse().context("MIB must be a number")?,
                None => 512,
            };
            // Splitting a fixed total (rather than sending it N times) keeps the
            // aggregate comparable across stream counts.
            let per_stream = mib * 1024 * 1024 / streams;
            let start = Instant::now();
            let handles: Vec<_> = (0..streams)
                .map(|_| {
                    let addr = addr.clone();
                    thread::spawn(move || send_stream(&addr, per_stream))
                })
                .collect();
            let mut sent = 0u64;
            for handle in handles {
                match handle.join() {
                    Ok(Ok(n)) => sent += n,
                    Ok(Err(error)) => eprintln!("stream failed: {error:#}"),
                    Err(_) => eprintln!("stream panicked"),
                }
            }
            let secs = start.elapsed().as_secs_f64();
            println!(
                "MULTI streams={streams} {sent} bytes {secs:.3} s {:.2} MiB/s aggregate",
                mib_per_sec(sent, secs)
            );
            Ok(())
        }
        _ => {
            bail!(
                "usage: tcp_baseline sink [PORT] \
                 | tcp_baseline source HOST:PORT [MIB] \
                 | tcp_baseline multi HOST:PORT STREAMS [MIB]"
            )
        }
    }
}
