//! Streaming receive: verified bytes go straight to the browser's download sink.
//!
//! The receiver used to pull a whole collection into an in-memory blob store,
//! copy every file back out of it, pack those copies into a zip, and hand the
//! zip to the browser - four live copies of the payload, three of them in wasm
//! linear memory, which cannot spill to disk and takes the tab down with it. So
//! a transfer was capped by the tab's RAM, not by the user's disk.
//!
//! Here the get response is consumed as it arrives and each chunk is forwarded
//! straight to the sink, so nothing larger than a chunk group is ever resident.
//!
//! Verification is unchanged: the get fsm checks every chunk against the hash
//! before yielding it, so nothing unverified is ever written. What is different
//! is that bytes reach the sink before the whole file is confirmed, and the
//! browser cannot un-download them - a failed transfer therefore leaves a short
//! file in the download folder rather than nothing at all.
//!
//! The fsm is driven on its own task and hands items over a bounded channel.
//! Two reasons it cannot be inlined into the caller's loop: advancing the fsm
//! consumes it by value, so the caller's `select` would destroy the transfer
//! state every time another branch won; and the bound is what applies
//! backpressure - the sink accepting bytes is what lets the next ones be read
//! off the wire.

use anyhow::{Result, anyhow, bail};
use bao_tree::io::BaoContentItem;
use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh_blobs::Hash;
use iroh_blobs::format::collection::{Collection, SimpleStore};
use iroh_blobs::get::fsm;
use iroh_blobs::get::request::get_blob;
use iroh_blobs::protocol::GetRequest;
use wasm_bindgen_futures::spawn_local;

/// Chunk groups the fetch may run ahead of the sink. Enough to keep the wire
/// busy while a write is in flight, small enough that "ahead of the sink" stays
/// a rounding error against the payload.
const PIPELINE_DEPTH: usize = 8;

/// What the fetch task reports as it walks the collection.
pub(crate) enum StreamEvent {
    /// A file is about to start. Its size is the one the sender proved, taken
    /// from the blob itself rather than the manifest.
    FileStart {
        path: String,
        size: u64,
    },
    /// Verified bytes, in order, for the file most recently started.
    Chunk(Bytes),
    /// The file's last chunk verified.
    FileEnd,
    /// Every file in the collection arrived.
    Done,
    Failed(String),
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

/// Start fetching `root_hash` over `connection`, reporting on the returned
/// channel.
///
/// Dropping the receiver cancels the fetch: the task stops at its next send.
pub(crate) fn spawn_collection(
    connection: Connection,
    root_hash: Hash,
) -> async_channel::Receiver<StreamEvent> {
    let (tx, rx) = async_channel::bounded(PIPELINE_DEPTH);
    spawn_local(async move {
        let last = match stream_collection(connection, root_hash, &tx).await {
            Ok(()) => StreamEvent::Done,
            Err(err) => StreamEvent::Failed(format!("{err:#}")),
        };
        let _ = tx.send(last).await;
        tx.close();
    });
    rx
}

async fn stream_collection(
    connection: Connection,
    root_hash: Hash,
    tx: &async_channel::Sender<StreamEvent>,
) -> Result<()> {
    let collection = Collection::load(root_hash, &ConnectionStore(connection.clone()))
        .await
        .map_err(|source| anyhow!("loading collection {root_hash}: {source:#}"))?;
    for (path, hash) in collection.into_iter() {
        stream_one(&connection, hash, path, tx).await?;
    }
    Ok(())
}

/// Fetches one blob, forwarding its leaves in order.
async fn stream_one(
    connection: &Connection,
    hash: Hash,
    path: String,
    tx: &async_channel::Sender<StreamEvent>,
) -> Result<()> {
    let failed =
        |stage: &str, err: &dyn std::fmt::Display| anyhow!("{path} ({hash}): {stage}: {err}");

    let start = fsm::start(
        connection.clone(),
        GetRequest::blob(hash),
        Default::default(),
    );
    let connected = start
        .next()
        .await
        .map_err(|err| failed("opening the request", &err))?;
    let fsm::ConnectedNext::StartRoot(start_root) = connected
        .next()
        .await
        .map_err(|err| failed("reading the response", &err))?
    else {
        // A single-blob request has no children, so the fsm can only be at the
        // root here.
        bail!("{path} ({hash}): expected the request to start at the root");
    };
    let (mut curr, size) = start_root
        .next()
        .next()
        .await
        .map_err(|err| failed("reading the size header", &err))?;

    if tx
        .send(StreamEvent::FileStart {
            path: path.clone(),
            size,
        })
        .await
        .is_err()
    {
        return Ok(()); // receiver gone: the transfer was cancelled
    }

    let mut written = 0_u64;
    let end = loop {
        match curr.next().await {
            fsm::BlobContentNext::More((next, item)) => {
                // Parent hashes arrive interleaved with the data; they are what
                // let each chunk be verified, and the fsm consumes them itself.
                if let BaoContentItem::Leaf(leaf) =
                    item.map_err(|err| failed("reading content", &err))?
                {
                    // The sink is append-only, so a leaf that is not the next
                    // one would silently shift the rest of the file. A full-blob
                    // request is answered in tree order, i.e. left to right, so
                    // this only fires if that ever stops being true.
                    if leaf.offset != written {
                        bail!(
                            "{path} ({hash}): chunk at {} arrived out of order, expected {written}",
                            leaf.offset
                        );
                    }
                    written = written.saturating_add(leaf.data.len() as u64);
                    if tx.send(StreamEvent::Chunk(leaf.data)).await.is_err() {
                        return Ok(());
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
            .map_err(|err| failed("closing the request", &err))?;
    }
    if written != size {
        bail!("{path} ({hash}): expected {size} bytes, got {written}");
    }

    let _ = tx.send(StreamEvent::FileEnd).await;
    Ok(())
}
