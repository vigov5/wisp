//! Browser (wasm32) file receiver for drift.
//!
//! A relay-only iroh endpoint playing the receiver half of the
//! `wisp/transfer/v1` v4 control protocol (shared schema from `wisp-wire`). All
//! file bytes ride n0 public relays end-to-end; the static page that loads this
//! wasm module carries none of them.
//!
//! Registers with the rendezvous server, displays a code, and accepts inbound
//! control connections. For each: surfaces the sender's identity, gates the
//! offer behind an Accept/Decline decision from the UI, then (on accept) streams
//! the collection straight into a browser download — see [`stream`] for why
//! nothing is held on the way through.

mod send;
mod sink;
mod stream;
mod wire;
mod zip;

pub use send::WebSender;

use anyhow::{Context, Result, anyhow, bail};
use iroh::Endpoint;
use iroh::endpoint::Connection;
use iroh_blobs::ALPN as BLOBS_ALPN;
use iroh_blobs::ticket::BlobTicket;
use js_sys::Function;
use serde::Serialize;
use sink::Downloads;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use wisp_wire::message::{
    Accept, Cancel, CancelPhase, Decline, DeviceType, Hello, Identity, ManifestItem,
    PROTOCOL_VERSION, ReceiverMessage, SenderMessage, TransferCompleted, TransferProgress,
    TransferProgressPayload, TransferResult, TransferRole, TransferStatus,
};
use wisp_wire::plan::TransferPhase;
use wisp_wire::rendezvous::RendezvousClient;

/// Soft ceiling for a transfer the page has to hold before it can offer it,
/// which is what happens when the browser can't stream a download to disk. We
/// warn the user past this — they can still choose to try. A streaming sink
/// accumulates nothing, so the ceiling doesn't apply to it.
const MAX_TRANSFER_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

/// How often the idle poller checks whether the code was claimed/expired.
const CODE_POLL_SECS: u64 = 15;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The control-protocol version this browser receiver speaks (must match the
/// native sender's `PROTOCOL_VERSION`).
#[wasm_bindgen]
pub fn protocol_version() -> u32 {
    PROTOCOL_VERSION
}

/// Events emitted to the JS `on_event` callback (serialized to plain objects).
///
/// The enum-level `rename_all` only camelCases the variant *tags*; the
/// per-variant `rename_all` is what camelCases the *fields* (so JS sees
/// `totalBytes`/`bytesReceived`, not the snake_case Rust names).
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Event {
    #[serde(rename_all = "camelCase")]
    Registered {
        code: String,
        /// RFC3339 expiry of the code, for a UI countdown.
        expires_at: String,
    },
    /// A sender dialed in; identity is known before the offer arrives.
    #[serde(rename_all = "camelCase")]
    Connecting {
        sender_name: String,
        sender_device_type: String,
        sender_pubkey: String,
    },
    #[serde(rename_all = "camelCase")]
    Offer {
        files: Vec<OfferItem>,
        total_bytes: u64,
        inline_text: Option<String>,
        /// True when the transfer likely won't fit in browser tab memory.
        too_large: bool,
    },
    TransferStarted,
    #[serde(rename_all = "camelCase")]
    Progress {
        bytes_received: u64,
        total_bytes: u64,
    },
    /// A file finished. `url` is set only when the page buffered the bytes
    /// instead of streaming them to disk, in which case the UI has to offer the
    /// download itself.
    FileReady {
        path: String,
        size: u64,
        url: Option<String>,
    },
    /// A text/link payload that rode inline in the offer (no file download).
    TextReady {
        text: String,
    },
    Completed,
    /// The user declined this offer; the receiver stays live for the next one.
    Declined,
    /// The user cancelled an in-flight transfer.
    Cancelled,
    Error {
        message: String,
    },
}

#[derive(Serialize)]
struct OfferItem {
    path: String,
    size: u64,
}

/// The user's decision on a pending offer, sent from `accept()`/`decline()`
/// back into the in-flight handshake task.
enum Decision {
    Accept,
    Decline,
}

/// Shared slot holding the decision sender for the currently-pending offer.
/// `!Send` (Rc) — fine because everything runs on the single browser task.
type PendingDecision = Rc<RefCell<Option<async_channel::Sender<Decision>>>>;

#[wasm_bindgen]
pub struct WebReceiver {
    code: String,
    pending: PendingDecision,
    cancel: Rc<RefCell<bool>>,
    // Kept so `refreshCode()` can mint a fresh code on demand.
    endpoint: Endpoint,
    client: RendezvousClient,
    current_code: Rc<RefCell<String>>,
    on_event: Function,
}

#[wasm_bindgen]
impl WebReceiver {
    /// Bind a relay-only endpoint, register with the rendezvous server, and
    /// start accepting inbound transfers in the background. Resolves once the
    /// 6-char code is known; `on_event` streams progress thereafter.
    ///
    /// `downloads` is the page's download factory — see [`sink`] for the shape
    /// it has to have. Received bytes go straight to it as they arrive, so what
    /// the page does with them decides whether a transfer is bounded by the
    /// tab's memory or by the user's disk.
    #[wasm_bindgen(js_name = start)]
    pub async fn start(
        rendezvous_url: String,
        on_event: Function,
        downloads: JsValue,
    ) -> Result<WebReceiver, JsValue> {
        run_start(rendezvous_url, on_event, Downloads::new(downloads))
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// The 6-char pairing code the sender enters.
    #[wasm_bindgen(js_name = code)]
    pub fn code(&self) -> String {
        self.code.clone()
    }

    /// Accept the currently-pending offer. No-op if nothing is pending.
    #[wasm_bindgen(js_name = accept)]
    pub fn accept(&self) {
        self.resolve(Decision::Accept);
    }

    /// Decline the currently-pending offer. No-op if nothing is pending.
    #[wasm_bindgen(js_name = decline)]
    pub fn decline(&self) {
        self.resolve(Decision::Decline);
    }

    /// Request cancellation of the in-flight transfer. Takes effect at the next
    /// progress tick; also declines a still-pending offer.
    #[wasm_bindgen(js_name = cancel)]
    pub fn cancel(&self) {
        *self.cancel.borrow_mut() = true;
        self.resolve(Decision::Decline);
    }

    /// Mint a fresh pairing code now, without waiting for the 15s poll. Fire-and-
    /// forget: re-registers in the background and pushes a `Registered` event
    /// that repaints the code.
    #[wasm_bindgen(js_name = refreshCode)]
    pub fn refresh_code(&self) {
        let endpoint = self.endpoint.clone();
        let client = self.client.clone();
        let current_code = self.current_code.clone();
        let on_event = self.on_event.clone();
        spawn_local(async move {
            if let Err(err) = reregister(&endpoint, &client, &current_code, &on_event).await {
                emit(
                    &on_event,
                    &Event::Error {
                        message: format!("refreshing code: {err:#}"),
                    },
                );
            }
        });
    }

    fn resolve(&self, decision: Decision) {
        if let Some(tx) = self.pending.borrow_mut().take() {
            let _ = tx.try_send(decision);
        }
    }
}

async fn run_start(
    rendezvous_url: String,
    on_event: Function,
    downloads: Downloads,
) -> Result<WebReceiver> {
    // Relay-only endpoint advertising only the control ALPN (blobs are dialed
    // out, not accepted). Ephemeral identity — fine for a no-install recipient.
    let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![wisp_wire::message::ALPN.to_vec()])
        .relay_mode(iroh::RelayMode::Default)
        .bind()
        .await?;
    // Ensure a relay home is assigned so our registered ticket is dialable.
    endpoint.online().await;

    let ticket = wisp_wire::ticket::encode_ticket(endpoint.addr())?;
    let client = RendezvousClient::new(rendezvous_url);
    let registration = client
        .register_peer(ticket)
        .await
        .context("registering with rendezvous")?;
    let code = registration.code.clone();
    emit(
        &on_event,
        &Event::Registered {
            code: code.clone(),
            expires_at: registration.expires_at.clone(),
        },
    );

    let pending: PendingDecision = Rc::new(RefCell::new(None));
    let cancel: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let busy: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let current_code: Rc<RefCell<String>> = Rc::new(RefCell::new(code.clone()));

    // Handles kept on the returned struct so `refreshCode()` can re-register on
    // demand (cloned before `endpoint`/`on_event` are moved into the accept loop).
    let recv_endpoint = endpoint.clone();
    let recv_client = client.clone();
    let recv_code = current_code.clone();
    let recv_on_event = on_event.clone();

    // Idle poller: keep a live code available. When the current code is claimed
    // or expires and we're not mid-transfer, mint a fresh one. (The already-
    // claimed ticket still reaches us endpoint-to-endpoint, so rotating is safe.)
    {
        let poll_endpoint = endpoint.clone();
        let poll_client = client.clone();
        let poll_on_event = on_event.clone();
        let poll_busy = busy.clone();
        let poll_code = current_code.clone();
        spawn_local(async move {
            loop {
                n0_future::time::sleep(Duration::from_secs(CODE_POLL_SECS)).await;
                if *poll_busy.borrow() {
                    continue;
                }
                let code_now = poll_code.borrow().clone();
                if let Ok(None) = poll_client.pair_status(&code_now).await {
                    if *poll_busy.borrow() {
                        continue;
                    }
                    let _ =
                        reregister(&poll_endpoint, &poll_client, &poll_code, &poll_on_event).await;
                }
            }
        });
    }

    let task_pending = pending.clone();
    let task_cancel = cancel.clone();
    let task_busy = busy.clone();
    let loop_client = client.clone();
    let loop_code = current_code.clone();

    spawn_local(async move {
        loop {
            let Some(incoming) = endpoint.accept().await else {
                break;
            };
            let conn = match incoming.await {
                Ok(conn) => conn,
                Err(err) => {
                    emit(
                        &on_event,
                        &Event::Error {
                            message: format!("accepting connection: {err}"),
                        },
                    );
                    continue;
                }
            };
            *task_cancel.borrow_mut() = false;
            *task_busy.borrow_mut() = true;
            // The sender just claimed the displayed code, so it's dead now. Mint a
            // fresh one immediately (matching native) so the next sender always has
            // a live code, without waiting out the 15s poll. Safe mid-transfer: the
            // claimed ticket still reaches us endpoint-to-endpoint, and the JS side
            // won't overwrite an in-flight offer's status on the 'registered' event.
            let _ = reregister(&endpoint, &loop_client, &loop_code, &on_event).await;
            if let Err(err) = handle_connection(
                &endpoint,
                &downloads,
                &conn,
                &on_event,
                &task_pending,
                &task_cancel,
            )
            .await
            {
                emit(
                    &on_event,
                    &Event::Error {
                        message: format!("{err:#}"),
                    },
                );
            }
            *task_busy.borrow_mut() = false;
            // Drop any stale decision slot between offers.
            task_pending.borrow_mut().take();
        }
    });

    Ok(WebReceiver {
        code,
        pending,
        cancel,
        endpoint: recv_endpoint,
        client: recv_client,
        current_code: recv_code,
        on_event: recv_on_event,
    })
}

/// Re-register the endpoint with the rendezvous server, mint a fresh code, and
/// push a `Registered` event so the UI repaints. Shared by the idle poller, the
/// claim-on-connect rotation, and the manual `refreshCode()`.
async fn reregister(
    endpoint: &Endpoint,
    client: &RendezvousClient,
    current_code: &Rc<RefCell<String>>,
    on_event: &Function,
) -> Result<()> {
    let ticket = wisp_wire::ticket::encode_ticket(endpoint.addr())?;
    let reg = client
        .register_peer(ticket)
        .await
        .context("re-registering with rendezvous")?;
    *current_code.borrow_mut() = reg.code.clone();
    emit(
        on_event,
        &Event::Registered {
            code: reg.code,
            expires_at: reg.expires_at,
        },
    );
    Ok(())
}

fn device_type_label(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::Phone => "phone",
        DeviceType::Laptop => "laptop",
    }
}

/// Name the bundled download. When every file sits under one top-level folder
/// (a folder send), name the zip after it (`album/…` → `album.zip`); otherwise
/// use a neutral name.
fn zip_archive_name(paths: &[String]) -> String {
    match paths.first().and_then(|path| top_segment(path)) {
        Some(top)
            if paths
                .iter()
                .all(|path| top_segment(path).as_deref() == Some(top.as_str())) =>
        {
            format!("{top}.zip")
        }
        _ => "wisp-files.zip".to_owned(),
    }
}

/// The first path segment when `path` lives inside a folder (`a/b.txt` → `a`);
/// `None` for a bare top-level file (no separator).
fn top_segment(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    let norm = norm.trim_start_matches('/');
    norm.find('/').map(|idx| norm[..idx].to_owned())
}

async fn handle_connection(
    endpoint: &Endpoint,
    downloads: &Downloads,
    conn: &Connection,
    on_event: &Function,
    pending: &PendingDecision,
    cancel: &Rc<RefCell<bool>>,
) -> Result<()> {
    let (mut control_send, mut control_recv) = conn.accept_bi().await?;

    // Handshake: read sender Hello, reply with our Hello.
    let hello = match wire::read_sender_message(&mut control_recv).await? {
        SenderMessage::Hello(hello) => hello,
        other => bail!("expected Hello, got {:?}", other.kind()),
    };
    let session_id = hello.session_id.clone();
    emit(
        on_event,
        &Event::Connecting {
            sender_name: hello.identity.device_name.clone(),
            sender_device_type: device_type_label(hello.identity.device_type).to_owned(),
            sender_pubkey: hello.identity.endpoint_id.to_string(),
        },
    );
    let our_identity = Identity {
        role: TransferRole::Receiver,
        endpoint_id: endpoint.id(),
        device_name: "Browser".to_owned(),
        device_type: DeviceType::Laptop,
        web: true,
        ephemeral: true,
    };
    wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::Hello(Hello {
            version: PROTOCOL_VERSION,
            session_id: session_id.clone(),
            identity: our_identity,
        }),
    )
    .await?;

    // Offer: surface files/sizes to the UI.
    let offer = match wire::read_sender_message(&mut control_recv).await? {
        SenderMessage::Offer(offer) => offer,
        other => bail!("expected Offer, got {:?}", other.kind()),
    };
    // Acknowledge the offer at once so the sender can leave its send/connecting
    // phase for "waiting for decision" instead of stalling on a wedged stream.
    wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::OfferAck(wisp_wire::message::OfferAck {
            session_id: session_id.clone(),
        }),
    )
    .await?;
    let files: Vec<OfferItem> = offer
        .manifest
        .items
        .iter()
        .map(|item| match item {
            ManifestItem::File { path, size } => OfferItem {
                path: path.clone(),
                size: *size,
            },
        })
        .collect();
    let total_bytes = offer.manifest.total_size();
    emit(
        on_event,
        &Event::Offer {
            files,
            total_bytes,
            inline_text: offer.inline_text.clone(),
            // Only a page that has to buffer the whole payload has a size it
            // can't cope with; a streaming sink is bounded by the disk instead.
            too_large: total_bytes > MAX_TRANSFER_BYTES && !downloads.streams_to_disk(),
        },
    );

    // Guard against a malicious/buggy peer shipping an unbounded inline frame.
    if let Some(text) = &offer.inline_text {
        if text.len() > wisp_wire::message::INLINE_TEXT_HARD_MAX_BYTES {
            bail!("inline text exceeds the maximum size");
        }
    }

    // Wait for the user's Accept/Decline (racing a sender disconnect so we don't
    // hang if they give up while the prompt is open).
    let (tx, rx) = async_channel::bounded::<Decision>(1);
    *pending.borrow_mut() = Some(tx);
    let decision = {
        let choose = async { rx.recv().await.ok() };
        let closed = async {
            conn.closed().await;
            None
        };
        futures_lite::future::or(choose, closed).await
    };
    pending.borrow_mut().take();

    match decision {
        None => return Ok(()), // connection closed while awaiting decision
        Some(Decision::Decline) => {
            wire::write_receiver_message(
                &mut control_send,
                &ReceiverMessage::Decline(Decline {
                    session_id: session_id.clone(),
                    reason: "declined by recipient".to_owned(),
                }),
            )
            .await?;
            emit(on_event, &Event::Declined);
            return Ok(());
        }
        Some(Decision::Accept) => {}
    }

    wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::Accept(Accept {
            session_id: session_id.clone(),
        }),
    )
    .await?;

    // Inline text/link: the payload already arrived in the offer — no blobs, no
    // uni stream. Hand it to the UI, then close out the control exchange exactly
    // as the sender expects (TransferResult -> read TransferAck).
    if let Some(text) = offer.inline_text.clone() {
        wire::write_receiver_message(
            &mut control_send,
            &ReceiverMessage::TransferResult(TransferResult {
                session_id: session_id.clone(),
                status: TransferStatus::Ok,
            }),
        )
        .await?;
        emit(on_event, &Event::TextReady { text });
        match wire::read_sender_message(&mut control_recv).await {
            Ok(SenderMessage::TransferAck(_)) => {}
            Ok(other) => tracing::warn!("expected TransferAck, got {:?}", other.kind()),
            Err(err) => tracing::warn!("reading TransferAck: {err:#}"),
        }
        emit(on_event, &Event::Completed);
        return Ok(());
    }

    emit(on_event, &Event::TransferStarted);

    // MUST open the uni progress stream even if we never write to it, or the
    // sender's `accept_uni()` deadlocks.
    let mut progress_send = conn.open_uni().await?;

    // BlobTicket → dial the sender on the blobs ALPN and stream the collection.
    let ticket_message = match wire::read_sender_message(&mut control_recv).await? {
        SenderMessage::BlobTicket(ticket) => ticket,
        other => bail!("expected BlobTicket, got {:?}", other.kind()),
    };
    let blob_ticket: BlobTicket = ticket_message
        .ticket
        .parse()
        .context("parsing blob ticket")?;

    let connection = endpoint
        .connect(blob_ticket.addr().clone(), BLOBS_ALPN)
        .await?;
    let events = stream::spawn_collection(connection.clone(), blob_ticket.hash());

    // One file is downloaded under its own name. Anything more is packed into a
    // single zip as it streams, because a browser can't rebuild a directory tree
    // on disk and per-file `<a download>`s flatten the paths away.
    let paths: Vec<String> = offer
        .manifest
        .items
        .iter()
        .map(|item| match item {
            ManifestItem::File { path, .. } => path.clone(),
        })
        .collect();
    let mut bundle = (paths.len() > 1).then(zip::ZipStream::new);
    let bundle_name = zip_archive_name(&paths);

    // Each step races the next stream item against (a) either connection
    // dropping — so a sender that dies mid-transfer can't hang us forever — and
    // (b) a short poll tick, so we make progress even while no bytes are flowing.
    // Re-polling `events.recv()` after a tick is cancel-safe: the fetch's state
    // lives on its own task, not in the dropped future.
    enum Step {
        Item(Option<stream::StreamEvent>),
        // A framed message that arrived on the control channel mid-transfer.
        Control(Result<SenderMessage>),
        Disconnected,
        Tick,
    }
    /// Why the streaming loop stopped.
    enum Outcome {
        Complete,
        Cancelled,
        Failed(anyhow::Error),
    }
    /// What handling one stream item decided about the loop.
    enum Flow {
        Continue,
        Complete,
        /// The user cancelled while a call into the sink was parked.
        Cancelled,
    }

    // The open download, and the file currently going into it. Held out here so
    // every exit path — including the ones that fail mid-file — can tell the
    // browser to drop a partial download instead of leaving it stuck at "in
    // progress" forever.
    let mut sink: Option<sink::Sink> = None;
    let mut current: Option<(String, u64)> = None;
    let mut received: u64 = 0;

    // Listen on the control channel for a sender-initiated Cancel *while* the
    // blob fetch runs. The sender writes `Cancel` on the control stream before
    // it tears the connection down (sender.rs), so watching here reacts in
    // ~1 RTT instead of waiting out the connection drop / QUIC idle timeout —
    // and lets us show a clean "cancelled" instead of a "sender disconnected"
    // error. One long-lived read future is polled across iterations; recreating
    // it each turn could drop a partially-read frame, so once it resolves we
    // stop racing it. The block scopes the `&mut control_recv` borrow so the
    // completion handshake below can reuse the stream.
    let outcome = {
        let mut control_read = Some(Box::pin(wire::read_sender_message(&mut control_recv)));
        let mut since_yield: u32 = 0;
        // Report progress back to the sender on its uni stream. Two reasons this
        // matters: (1) a QUIC uni stream isn't observable to the peer until bytes
        // are written to it, so the *first* frame is what unblocks the sender's
        // `accept_uni().await` — without it a native sender parks at "Accepted"
        // for the whole transfer and its Cancel (checked only after accept_uni)
        // does nothing; (2) subsequent frames drive the sender's progress bar.
        // Throttled to ~512 KiB so a big transfer doesn't flood the stream.
        const PROGRESS_FRAME_BYTES: u64 = 512 * 1024;
        // Chunks now arrive every 16 KiB — one repaint each would cost more than
        // it shows, so the UI gets its own, coarser throttle.
        const UI_PROGRESS_BYTES: u64 = 256 * 1024;
        let mut reported_at: u64 = 0;
        let mut reported_any = false;
        let mut painted_at: u64 = 0;
        let mut outcome = Outcome::Complete;
        loop {
            // Cancel is checked every iteration, not just on the idle tick: a fast,
            // steady stream keeps `recv` immediately ready, so the tick branch would
            // otherwise starve and the flag would never be observed.
            if *cancel.borrow() {
                let _ = wire::write_receiver_message(
                    &mut control_send,
                    &ReceiverMessage::Cancel(Cancel {
                        session_id: session_id.clone(),
                        by: TransferRole::Receiver,
                        phase: CancelPhase::Transferring,
                        reason: "cancelled by recipient".to_owned(),
                    }),
                )
                .await;
                outcome = Outcome::Cancelled;
                break;
            }

            // Yield to the browser event loop periodically. The fetch is CPU-bound
            // (relay AEAD + blake3 verification) and wasm is single-threaded, so a
            // burst of already-buffered chunks would run to completion without ever
            // returning control — freezing the tab. While frozen, queued input never
            // dispatches (the Cancel click is dropped) and the DOM never repaints
            // (Accept/Decline stay on screen mid-transfer). A 0-delay timer hands the
            // event loop one macrotask turn to process both. Every ~8 items keeps the
            // throughput cost negligible.
            since_yield += 1;
            if since_yield >= 8 {
                since_yield = 0;
                n0_future::time::sleep(Duration::from_millis(0)).await;
            }

            let next = async { Step::Item(events.recv().await.ok()) };
            let gone = async {
                futures_lite::future::or(conn.closed(), connection.closed()).await;
                Step::Disconnected
            };
            let tick = async {
                n0_future::time::sleep(Duration::from_millis(200)).await;
                Step::Tick
            };
            let step = match control_read.as_mut() {
                Some(cr) => {
                    let ctrl = async { Step::Control(cr.await) };
                    futures_lite::future::or(
                        next,
                        futures_lite::future::or(ctrl, futures_lite::future::or(gone, tick)),
                    )
                    .await
                }
                None => futures_lite::future::or(next, futures_lite::future::or(gone, tick)).await,
            };
            match step {
                Step::Item(Some(item)) => {
                    // Handling an item is fallible at every turn, and a failure
                    // has to unwind through the sink rather than straight out of
                    // the function, so it runs in its own block and reports back.
                    let handled: Result<Flow> = async {
                        match item {
                            stream::StreamEvent::FileStart { path, size } => {
                                match bundle.as_mut() {
                                    Some(zip) => {
                                        // One sink for the whole archive, opened on
                                        // the first file. Its final size isn't known
                                        // until the last entry is written, so the
                                        // browser gets no length to show progress
                                        // against.
                                        if sink.is_none() {
                                            let opened = or_cancelled(
                                                downloads.open(&bundle_name, None),
                                                cancel,
                                            )
                                            .await;
                                            let Some(opened) = opened else {
                                                return Ok(Flow::Cancelled);
                                            };
                                            sink = Some(opened?);
                                        }
                                        let header = zip.begin(&path, size);
                                        let open = sink.as_ref().expect("just opened");
                                        let Some(written) =
                                            or_cancelled(open.write(&header), cancel).await
                                        else {
                                            return Ok(Flow::Cancelled);
                                        };
                                        written?;
                                    }
                                    None => {
                                        // The manifest said one file, so a second
                                        // would silently replace the first
                                        // download and leave it hanging.
                                        if sink.is_some() {
                                            bail!(
                                                "the collection holds more files than the offer did"
                                            );
                                        }
                                        let opened =
                                            or_cancelled(downloads.open(&path, Some(size)), cancel)
                                                .await;
                                        let Some(opened) = opened else {
                                            return Ok(Flow::Cancelled);
                                        };
                                        sink = Some(opened?);
                                        current = Some((path, size));
                                    }
                                }
                            }
                            stream::StreamEvent::Chunk(bytes) => {
                                let open = sink
                                    .as_ref()
                                    .context("bytes arrived before the file they belong to")?;
                                let Some(written) = or_cancelled(open.write(&bytes), cancel).await
                                else {
                                    return Ok(Flow::Cancelled);
                                };
                                written?;
                                if let Some(zip) = bundle.as_mut() {
                                    zip.data(&bytes);
                                }
                                received = received.saturating_add(bytes.len() as u64);

                                if received.saturating_sub(painted_at) >= UI_PROGRESS_BYTES {
                                    painted_at = received;
                                    emit(
                                        on_event,
                                        &Event::Progress {
                                            bytes_received: received,
                                            total_bytes,
                                        },
                                    );
                                }
                                if !reported_any || received >= reported_at + PROGRESS_FRAME_BYTES {
                                    reported_any = true;
                                    reported_at = received;
                                    let snapshot = TransferProgressPayload {
                                        phase: TransferPhase::Transferring,
                                        completed_files: 0,
                                        total_files: offer.manifest.count() as u32,
                                        bytes_transferred: received.min(total_bytes),
                                        total_bytes,
                                        active_file_id: None,
                                        active_file_bytes: None,
                                    };
                                    let _ = wire::write_receiver_message(
                                        &mut progress_send,
                                        &ReceiverMessage::TransferProgress(TransferProgress {
                                            session_id: session_id.clone(),
                                            snapshot,
                                        }),
                                    )
                                    .await;
                                }
                            }
                            stream::StreamEvent::FileEnd => match bundle.as_mut() {
                                Some(zip) => {
                                    let descriptor = zip.end();
                                    let open =
                                        sink.as_ref().context("a file ended before it started")?;
                                    let Some(written) =
                                        or_cancelled(open.write(&descriptor), cancel).await
                                    else {
                                        return Ok(Flow::Cancelled);
                                    };
                                    written?;
                                }
                                None => {
                                    let (path, size) =
                                        current.take().context("a file ended before it started")?;
                                    let open =
                                        sink.take().context("a file ended before it started")?;
                                    let Some(url) = or_cancelled(open.close(), cancel).await else {
                                        return Ok(Flow::Cancelled);
                                    };
                                    emit(
                                        on_event,
                                        &Event::FileReady {
                                            path,
                                            size,
                                            url: url?,
                                        },
                                    );
                                }
                            },
                            stream::StreamEvent::Done => {
                                if let (Some(zip), Some(open)) = (bundle.take(), sink.take()) {
                                    let (trailer, size) = zip.finish();
                                    let Some(written) =
                                        or_cancelled(open.write(&trailer), cancel).await
                                    else {
                                        return Ok(Flow::Cancelled);
                                    };
                                    written?;
                                    let Some(url) = or_cancelled(open.close(), cancel).await else {
                                        return Ok(Flow::Cancelled);
                                    };
                                    emit(
                                        on_event,
                                        &Event::FileReady {
                                            path: bundle_name.clone(),
                                            size,
                                            url: url?,
                                        },
                                    );
                                }
                                return Ok(Flow::Complete);
                            }
                            stream::StreamEvent::Failed(message) => bail!("{message}"),
                        }
                        Ok(Flow::Continue)
                    }
                    .await;
                    match handled {
                        Ok(Flow::Continue) => {}
                        Ok(Flow::Complete) => break,
                        Ok(Flow::Cancelled) => {
                            outcome = Outcome::Cancelled;
                            break;
                        }
                        Err(err) => {
                            outcome = Outcome::Failed(err);
                            break;
                        }
                    }
                }
                // The task always signals Done or Failed before it closes the
                // channel, so a bare close means it went away unannounced.
                Step::Item(None) => {
                    outcome = Outcome::Failed(anyhow!("the download stopped unexpectedly"));
                    break;
                }
                // Sender asked to cancel mid-transfer — stop and show it cleanly.
                Step::Control(Ok(SenderMessage::Cancel(_))) => {
                    outcome = Outcome::Cancelled;
                    break;
                }
                // Any other control message (or a read error) mid-transfer is
                // unexpected; stop listening and let the stream / disconnect
                // branches drive the outcome.
                Step::Control(_) => control_read = None,
                Step::Disconnected => {
                    outcome = Outcome::Failed(anyhow!("sender disconnected"));
                    break;
                }
                // Idle tick: loop back so the cancel check at the top runs.
                Step::Tick => {}
            }
        }
        outcome
    };

    match outcome {
        Outcome::Complete => {}
        // A browser can't take back bytes it has already written, but it can be
        // told to stop — which leaves a download the user can see failed, rather
        // than a short file that looks whole.
        Outcome::Cancelled => {
            if let Some(open) = sink.take() {
                open.abort().await;
            }
            emit(on_event, &Event::Cancelled);
            return Ok(());
        }
        Outcome::Failed(err) => {
            if let Some(open) = sink.take() {
                open.abort().await;
            }
            return Err(err);
        }
    }

    // Never let the UI throttle hide the final byte count.
    emit(
        on_event,
        &Event::Progress {
            bytes_received: received,
            total_bytes,
        },
    );

    // Completion handshake: TransferCompleted (uni) → TransferResult (control)
    // → wait for the sender's TransferAck.
    let snapshot = TransferProgressPayload {
        phase: TransferPhase::Completed,
        completed_files: offer.manifest.count() as u32,
        total_files: offer.manifest.count() as u32,
        bytes_transferred: total_bytes,
        total_bytes,
        active_file_id: None,
        active_file_bytes: None,
    };
    let _ = wire::write_receiver_message(
        &mut progress_send,
        &ReceiverMessage::TransferCompleted(TransferCompleted {
            session_id: session_id.clone(),
            snapshot,
        }),
    )
    .await;
    let _ = progress_send.finish();

    // The files are already downloaded, so the transfer is complete from the
    // recipient's point of view. Send the TransferResult and mark the UI
    // complete immediately — everything past this point is best-effort cleanup
    // that must NOT be able to wedge the UI at "in progress" (e.g. a sender that
    // closes the connection right after its own success instead of sending a
    // clean TransferAck, which is exactly what a graceful-close native sender
    // does).
    let _ = wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::TransferResult(TransferResult {
            session_id: session_id.clone(),
            status: TransferStatus::Ok,
        }),
    )
    .await;

    emit(on_event, &Event::Completed);

    // Best-effort, bounded drain of the sender's final ack so a sender that
    // never sends it (or drops the connection) can't block the accept loop.
    let ack = async {
        let _ = wire::read_sender_message(&mut control_recv).await;
    };
    futures_lite::future::or(ack, n0_future::time::sleep(Duration::from_secs(3))).await;
    Ok(())
}

/// Await `work`, giving up if the user cancels while it is still parked.
///
/// Every call into the page's sink can block for as long as the browser likes: a
/// download it hasn't decided to accept is never drained, and the write behind it
/// never resolves. The transfer loop checks the cancel flag between items, so
/// without this it would sit inside that await and the Cancel button would do
/// nothing at all. `None` means the user gave up first; the abandoned future is
/// just a pending JS promise, and the sink is aborted straight after.
async fn or_cancelled<T>(
    work: impl std::future::Future<Output = T>,
    cancel: &Rc<RefCell<bool>>,
) -> Option<T> {
    let watch = async {
        loop {
            if *cancel.borrow() {
                return None;
            }
            n0_future::time::sleep(Duration::from_millis(150)).await;
        }
    };
    futures_lite::future::or(async { Some(work.await) }, watch).await
}

fn emit(on_event: &Function, event: &Event) {
    if let Ok(value) = serde_wasm_bindgen::to_value(event) {
        let _ = on_event.call1(&JsValue::NULL, &value);
    }
}
