//! Prices the LAN TCP transport before anyone builds it.
//!
//! The Pixel 4 sends at 18.7 MiB/s through the app and 33-41 MiB/s through raw
//! QUIC, on a link that carries 67.7 over TCP, because its 4.14 kernel has no
//! `UDP_SEGMENT` and quinn therefore issues one `sendmsg` per datagram. TCP
//! gets segmentation offload from any kernel, which is the whole of LocalSend's
//! advantage on that phone. What that is worth *to this codebase* depends on
//! whether the blob layer can feed a TCP socket faster than it can feed a QUIC
//! one, and that is what this measures.
//!
//! It runs the real `iroh-blobs` get protocol — the same request, the same BAO
//! verified stream, the same provider handler — over a plain `TcpStream`
//! instead of a QUIC connection. Almost nothing had to change to allow it:
//! every get state after `AtConnected` and the whole provider side are already
//! generic over `RecvStream`/`SendStream`, so the only addition was
//! `fsm::start_with_streams`, an entry point that takes a stream pair instead
//! of a connection to open one on.
//!
//! Provider (prints the hash, then serves until killed):
//!
//! ```text
//! cargo run --release -p wisp-core --example blob_over_tcp -- provide --file PATH [--port 5300]
//! ```
//!
//! Getter (writes to `--out`, or discards):
//!
//! ```text
//! cargo run --release -p wisp-core --example blob_over_tcp -- fetch HOST:PORT HASH [--out PATH]
//! ```
//!
//! **Not a transport.** The socket is unencrypted and unauthenticated, so this
//! is a measurement tool and nothing else. A real LAN transport has to carry
//! TLS over the TCP socket, pinned to the peer identity Wisp already has — the
//! shape LocalSend uses — and the AEAD it adds back is affordable: ring's
//! AES-GCM was 3.1% of the sender's cycles, against 59% in the kernel.

use std::io::SeekFrom;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use bao_tree::io::BaoContentItem;
use iroh_blobs::api::blobs::{AddPathOptions, ImportMode};
use iroh_blobs::get::fsm;
use iroh_blobs::protocol::GetRequest;
use iroh_blobs::provider::events::EventSender;
use iroh_blobs::provider::{StreamPair, handle_stream};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::util::{AsyncReadRecvStream, AsyncWriteSendStream};
use iroh_blobs::{BlobFormat, Hash};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use wisp_core::lan_transport::{LanRecvStream, LanSendStream};

/// Buffered file writes on the getter, so a 16 KiB leaf is not a syscall.
///
/// The receiver's write path was measured not to be the constraint over QUIC,
/// but this harness must not become the constraint itself: it is meant to bound
/// the transport, not to reproduce the app's I/O shape.
const WRITE_BUFFER_BYTES: usize = 512 * 1024;

// --- TCP as a RecvStream / SendStream -------------------------------------
//
// `wisp_core::lan_transport` already decides how a TCP half answers the three
// questions `iroh-blobs`' adapters ask, two of which have no TCP equivalent.
// Reusing it keeps that decision in one place; this example differs from the
// real transport only in having no TLS, which is why it is a measurement tool
// and not one.

type Recv = AsyncReadRecvStream<LanRecvStream<OwnedReadHalf>>;
type Send = AsyncWriteSendStream<LanSendStream<OwnedWriteHalf>>;

fn split(stream: TcpStream) -> (Recv, Send) {
    stream.set_nodelay(true).ok();
    let (read, write) = stream.into_split();
    (
        AsyncReadRecvStream::new(LanRecvStream::new(read)),
        AsyncWriteSendStream::new(LanSendStream::new(write)),
    )
}

fn mib_per_sec(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    bytes as f64 / (1024.0 * 1024.0) / secs
}

// --- provider -------------------------------------------------------------

async fn provide(file: PathBuf, port: u16) -> Result<()> {
    let store_dir = std::env::temp_dir().join("blob_over_tcp_store");
    tokio::fs::create_dir_all(&store_dir).await?;
    let store = FsStore::load(&store_dir)
        .await
        .context("loading the blob store")?;

    // `TryReference` so the import does not copy the payload; the sender in the
    // app imports the same way, and a copy would price the wrong thing.
    let tag = store
        .add_path_with_opts(AddPathOptions {
            path: file.clone(),
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .with_context(|| format!("importing {}", file.display()))?;
    let hash = tag.hash();
    let size = tokio::fs::metadata(&file).await?.len();
    println!("HASH {hash}");
    println!("SIZE {size}");

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("provider listening on 0.0.0.0:{port}");
    let mut connection_id = 0u64;
    loop {
        let (stream, peer) = listener.accept().await?;
        connection_id += 1;
        let store = (*store).clone();
        let id = connection_id;
        tokio::spawn(async move {
            let (reader, writer) = split(stream);
            let pair = StreamPair::new(id, reader, writer, EventSender::DEFAULT);
            let started = Instant::now();
            match handle_stream(pair, store).await {
                Ok(()) => println!("served {peer} in {:.3} s", started.elapsed().as_secs_f64()),
                Err(error) => eprintln!("serving {peer} failed: {error}"),
            }
        });
    }
}

// --- getter ---------------------------------------------------------------

async fn fetch(addr: String, hash: Hash, out: Option<PathBuf>) -> Result<()> {
    let stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connecting to {addr}"))?;
    let (reader, writer) = split(stream);

    let mut sink: Option<tokio::io::BufWriter<tokio::fs::File>> = match &out {
        Some(path) => Some(tokio::io::BufWriter::with_capacity(
            WRITE_BUFFER_BYTES,
            tokio::fs::File::create(path).await?,
        )),
        None => None,
    };

    let connected =
        fsm::start_with_streams(reader, writer, GetRequest::blob(hash), Default::default());
    let started = Instant::now();
    let fsm::ConnectedNext::StartRoot(start_root) = connected.next().await? else {
        bail!("a single-blob request can only start at the root");
    };
    let (mut curr, size) = start_root.next().next().await?;
    let mut received = 0u64;
    let mut cursor = None;
    let end = loop {
        match curr.next().await {
            fsm::BlobContentNext::More((next, item)) => {
                if let BaoContentItem::Leaf(leaf) = item? {
                    received += leaf.data.len() as u64;
                    if let Some(file) = sink.as_mut() {
                        if cursor != Some(leaf.offset) {
                            file.seek(SeekFrom::Start(leaf.offset)).await?;
                        }
                        file.write_all(&leaf.data).await?;
                        cursor = Some(leaf.offset + leaf.data.len() as u64);
                    }
                }
                curr = next;
            }
            fsm::BlobContentNext::Done(end) => break end,
        }
    };
    if let fsm::EndBlobNext::Closing(closing) = end.next() {
        closing.next().await?;
    }
    if let Some(mut file) = sink {
        file.flush().await?;
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "FETCH {received} bytes (declared {size}) {secs:.3} s {:.2} MiB/s",
        mib_per_sec(received, secs)
    );
    Ok(())
}

// --- main -----------------------------------------------------------------

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("provide") => {
            let file = arg_value(&args, "--file")
                .context("usage: provide --file PATH [--port N]")?
                .into();
            let port = arg_value(&args, "--port")
                .map(|p| p.parse())
                .transpose()
                .context("--port must be a u16")?
                .unwrap_or(5300);
            provide(file, port).await
        }
        Some("fetch") => {
            let addr = args.get(2).context("usage: fetch HOST:PORT HASH")?.clone();
            let hash: Hash = args
                .get(3)
                .context("usage: fetch HOST:PORT HASH")?
                .parse()
                .context("HASH must be a blake3 hash")?;
            fetch(addr, hash, arg_value(&args, "--out").map(PathBuf::from)).await
        }
        _ => bail!(
            "usage: blob_over_tcp provide --file PATH [--port N] \
             | blob_over_tcp fetch HOST:PORT HASH [--out PATH]"
        ),
    }
}
