//! Browser (wasm32) sender for drift — the send half of `wisp/transfer/v1`.
//!
//! Mirrors the native sender's control exchange (`crates/core/src/transfer/
//! sender.rs`): claim a code from the rendezvous server, dial the receiver on
//! the control ALPN over an n0 relay, exchange Hellos, send an offer, and wait
//! for the receiver's Accept/Decline.
//!
//! Two payload paths:
//! - **Text/link** (protocol v4 `Offer.inline_text`): the text rides the offer
//!   frame itself — no blobs, no `MemStore`. Low-risk, always available.
//! - **File** (D2 spike): the bytes are added to an in-memory blob store, served
//!   from the browser over the blobs ALPN via an `iroh::protocol::Router`, and
//!   the receiver dials back to fetch them — the same pull model native uses,
//!   just with the provider running in the tab. RAM-bound (everything lives in
//!   `MemStore`).

use anyhow::{Context, Result, anyhow, bail};
use iroh::Endpoint;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::Router;
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::format::collection::Collection;
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::{ALPN as BLOBS_ALPN, BlobFormat, BlobsProtocol, Hash};
use js_sys::{Array, Function, Uint8Array};
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use wisp_wire::message::{
    ALPN, BlobTicketMessage, Cancel, CancelPhase, DeviceType, Hello, INLINE_TEXT_MAX_BYTES,
    Identity, ManifestItem, Offer, PROTOCOL_VERSION, ReceiverMessage, SenderMessage, TransferAck,
    TransferManifest, TransferRole, TransferStatus,
};
use wisp_wire::rendezvous::RendezvousClient;

use crate::wire;

/// How long to wait for the receiver's decision once the offer is on the wire.
/// Matches the native sender's generous human-in-the-loop window so the browser
/// doesn't give up before the recipient taps.
const DECISION_WAIT_SECS: u64 = 130;

/// Per-attempt dial timeout and retry budget. A relay path can take a beat to
/// come up on the first try; a couple of bounded retries let that self-heal.
const CONNECT_ATTEMPTS: usize = 3;
const CONNECT_ATTEMPT_SECS: u64 = 8;

/// Events emitted to the JS `on_event` callback for a send (camelCased to match
/// the receiver's event surface).
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SendEvent {
    /// Dialing the receiver over the relay.
    Connecting,
    /// The receiver answered our Hello; identity is known, awaiting the decision.
    #[serde(rename_all = "camelCase")]
    WaitingForDecision {
        receiver_name: String,
        receiver_device_type: String,
        receiver_pubkey: String,
        receiver_web: bool,
    },
    /// The receiver accepted; the payload is being delivered.
    Accepted,
    /// A file transfer is streaming; bytes flow until `completed`.
    TransferStarted,
    #[serde(rename_all = "camelCase")]
    Progress {
        bytes_sent: u64,
        total_bytes: u64,
    },
    /// The receiver declined the offer.
    Declined {
        reason: String,
    },
    /// The payload was delivered and acknowledged.
    Completed,
    /// The send was cancelled (by us, before/while awaiting the decision).
    Cancelled,
    Error {
        message: String,
    },
}

/// The receiver's answer to our offer.
enum Decision {
    Accept,
    Decline(String),
    /// Cancelled locally, or the peer vanished — the caller just returns; the
    /// helper has already emitted the terminal event.
    Aborted,
}

#[wasm_bindgen]
pub struct WebSender {
    cancel: Rc<RefCell<bool>>,
}

#[wasm_bindgen]
impl WebSender {
    /// Send a text/link payload to a receiver identified by its 6-char code.
    ///
    /// Claims the code (rejecting the returned promise if it's unknown/expired),
    /// then runs the handshake in the background, streaming progress through
    /// `on_event`. Resolves with a handle whose [`cancel`](Self::cancel) aborts
    /// a send still waiting on the recipient's decision.
    #[wasm_bindgen(js_name = sendText)]
    pub async fn send_text(
        rendezvous_url: String,
        code: String,
        text: String,
        device_name: String,
        on_event: Function,
    ) -> Result<WebSender, JsValue> {
        if text.is_empty() {
            return Err(JsValue::from_str("nothing to send"));
        }
        if text.len() > INLINE_TEXT_MAX_BYTES {
            return Err(JsValue::from_str(&format!(
                "text is too long for a browser text send ({} bytes; max {INLINE_TEXT_MAX_BYTES}). \
                 Send it as a file instead.",
                text.len()
            )));
        }
        start_send(
            rendezvous_url,
            code,
            device_name,
            on_event,
            Payload::Text(text),
        )
        .await
        .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Send one or more files (a folder send passes each file with its relative
    /// path, e.g. `folder/sub/a.txt`) to a receiver identified by its 6-char
    /// code. `paths[i]` names `blobs[i]` (a `Uint8Array` of that file's bytes);
    /// everything is served from an in-memory store in the tab, so the whole
    /// batch is bounded by tab RAM.
    #[wasm_bindgen(js_name = sendFiles)]
    pub async fn send_files(
        rendezvous_url: String,
        code: String,
        paths: Vec<String>,
        blobs: Array,
        device_name: String,
        on_event: Function,
    ) -> Result<WebSender, JsValue> {
        if paths.is_empty() || blobs.length() == 0 {
            return Err(JsValue::from_str("no files to send"));
        }
        if paths.len() as u32 != blobs.length() {
            return Err(JsValue::from_str("file path/byte count mismatch"));
        }
        let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
        for (i, path) in paths.into_iter().enumerate() {
            let bytes = Uint8Array::new(&blobs.get(i as u32)).to_vec();
            files.push((path, bytes));
        }
        if files.iter().all(|(_, b)| b.is_empty()) {
            return Err(JsValue::from_str("the files are empty"));
        }
        start_send(
            rendezvous_url,
            code,
            device_name,
            on_event,
            Payload::Files(files),
        )
        .await
        .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// Request cancellation of an in-flight send. Takes effect while awaiting the
    /// receiver's decision (a no-op once delivery has started).
    #[wasm_bindgen(js_name = cancel)]
    pub fn cancel(&self) {
        *self.cancel.borrow_mut() = true;
    }
}

enum Payload {
    Text(String),
    Files(Vec<(String, Vec<u8>)>),
}

/// Claim the code (rejecting up front on a bad/expired one), bind the endpoint,
/// then spawn the background send task and return the cancel handle.
async fn start_send(
    rendezvous_url: String,
    code: String,
    device_name: String,
    on_event: Function,
    payload: Payload,
) -> Result<WebSender> {
    // Resolve the code to the receiver's ticket up front so a bad/expired code
    // surfaces as a rejected promise the form can show inline.
    let client = RendezvousClient::new(rendezvous_url);
    let claim = client
        .claim_peer(&code)
        .await
        .context("no such code, or it has expired")?;
    let addr =
        wisp_wire::ticket::decode_ticket(&claim.ticket).context("decoding receiver ticket")?;

    // Relay-only endpoint. For a file send it must also accept the receiver's
    // inbound blobs-ALPN fetch, so advertise that ALPN; a text send never
    // accepts anything but advertising it is harmless.
    let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![BLOBS_ALPN.to_vec()])
        .relay_mode(iroh::RelayMode::Default)
        .bind()
        .await?;
    endpoint.online().await;

    let identity = Identity {
        role: TransferRole::Sender,
        endpoint_id: endpoint.id(),
        device_name: if device_name.trim().is_empty() {
            "Browser".to_owned()
        } else {
            device_name
        },
        device_type: DeviceType::Laptop,
        web: true,
        ephemeral: true,
    };

    let cancel: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let task_cancel = cancel.clone();

    spawn_local(async move {
        let result = match payload {
            Payload::Text(text) => {
                drive_send_text(&endpoint, addr, identity, text, &on_event, &task_cancel).await
            }
            Payload::Files(files) => {
                drive_send_files(&endpoint, addr, identity, files, &on_event, &task_cancel).await
            }
        };
        if let Err(err) = result {
            emit(
                &on_event,
                &SendEvent::Error {
                    message: format!("{err:#}"),
                },
            );
        }
        // Flush the CONNECTION_CLOSE so the receiver sees the session end in
        // ~1 RTT rather than waiting out the QUIC idle timeout.
        endpoint.close().await;
    });

    Ok(WebSender { cancel })
}

/// Dial + handshake + offer + await decision, shared by both payloads. Returns
/// the live control streams and the receiver's decision. Emits `Connecting` and
/// `WaitingForDecision` (and, on a local cancel / peer drop, the terminal
/// `Cancelled` before returning [`Decision::Aborted`]).
async fn handshake(
    endpoint: &Endpoint,
    addr: iroh::EndpointAddr,
    identity: &Identity,
    session_id: &str,
    manifest: TransferManifest,
    collection_hash: Hash,
    inline_text: Option<String>,
    on_event: &Function,
    cancel: &Rc<RefCell<bool>>,
) -> Result<(Connection, SendStream, RecvStream, Decision)> {
    emit(on_event, &SendEvent::Connecting);
    let conn = connect_with_retry(endpoint, addr).await?;
    let (mut control_send, mut control_recv) = conn.open_bi().await?;

    wire::write_sender_message(
        &mut control_send,
        &SenderMessage::Hello(Hello {
            version: PROTOCOL_VERSION,
            session_id: session_id.to_owned(),
            identity: identity.clone(),
        }),
    )
    .await?;
    let peer_hello = match wire::read_receiver_message(&mut control_recv).await? {
        ReceiverMessage::Hello(hello) => hello,
        other => bail!("expected receiver Hello, got {:?}", other.kind()),
    };

    wire::write_sender_message(
        &mut control_send,
        &SenderMessage::Offer(Offer {
            session_id: session_id.to_owned(),
            manifest,
            collection_hash,
            inline_text,
        }),
    )
    .await?;

    // Wait for the receiver to confirm it read the offer before declaring
    // "waiting for decision" — a stalled offer keeps us in the send/connecting
    // phase instead of falsely reporting the receiver is deciding.
    match wire::read_receiver_message(&mut control_recv).await? {
        ReceiverMessage::OfferAck(_) => {}
        other => bail!("expected offer ack, got {:?}", other.kind()),
    }

    emit(
        on_event,
        &SendEvent::WaitingForDecision {
            receiver_name: peer_hello.identity.device_name.clone(),
            receiver_device_type: device_type_label(peer_hello.identity.device_type).to_owned(),
            receiver_pubkey: peer_hello.identity.endpoint_id.to_string(),
            receiver_web: peer_hello.identity.web,
        },
    );

    // Wait for Accept/Decline, honouring a local cancel and a receiver
    // disconnect. A long-lived read future is polled across ticks so a
    // partially-read frame is never dropped (cancel-safety).
    let decision = {
        let mut read = Box::pin(wire::read_receiver_message(&mut control_recv));
        let mut ticks_left: u32 = (DECISION_WAIT_SECS * 1000 / 150) as u32;
        let msg = loop {
            if *cancel.borrow() {
                let _ = wire::write_sender_message(
                    &mut control_send,
                    &SenderMessage::Cancel(Cancel {
                        session_id: session_id.to_owned(),
                        by: TransferRole::Sender,
                        phase: CancelPhase::WaitingForDecision,
                        reason: "cancelled by sender".to_owned(),
                    }),
                )
                .await;
                emit(on_event, &SendEvent::Cancelled);
                // `read` still borrows control_recv; drop it before we move the
                // streams into the return value.
                drop(read);
                return Ok((conn, control_send, control_recv, Decision::Aborted));
            }

            enum W {
                Msg(Result<ReceiverMessage>),
                Closed,
                Tick,
            }
            let recv = async { W::Msg((&mut read).await) };
            let closed = async {
                conn.closed().await;
                W::Closed
            };
            let tick = async {
                n0_future::time::sleep(Duration::from_millis(150)).await;
                W::Tick
            };
            match futures_lite::future::or(recv, futures_lite::future::or(closed, tick)).await {
                W::Msg(m) => break m?,
                W::Closed => bail!("receiver disconnected before deciding"),
                W::Tick => {
                    ticks_left = ticks_left.saturating_sub(1);
                    if ticks_left == 0 {
                        bail!("timed out waiting for the receiver to accept");
                    }
                }
            }
        };
        drop(read);
        match msg {
            ReceiverMessage::Accept(_) => Decision::Accept,
            ReceiverMessage::Decline(d) => Decision::Decline(d.reason),
            other => bail!("expected Accept/Decline, got {:?}", other.kind()),
        }
    };

    Ok((conn, control_send, control_recv, decision))
}

async fn drive_send_text(
    endpoint: &Endpoint,
    addr: iroh::EndpointAddr,
    identity: Identity,
    text: String,
    on_event: &Function,
    cancel: &Rc<RefCell<bool>>,
) -> Result<()> {
    let session_id = new_session_id();
    let (_conn, mut control_send, mut control_recv, decision) = handshake(
        endpoint,
        addr,
        &identity,
        &session_id,
        TransferManifest { items: Vec::new() },
        [0u8; 32].into(),
        Some(text),
        on_event,
        cancel,
    )
    .await?;

    match decision {
        Decision::Accept => emit(on_event, &SendEvent::Accepted),
        Decision::Decline(reason) => {
            emit(on_event, &SendEvent::Declined { reason });
            return Ok(());
        }
        Decision::Aborted => return Ok(()),
    }

    // Inline completion: the text already reached the receiver in the offer, so
    // it replies TransferResult(Ok); we ack and finish.
    read_result_and_ack(&mut control_send, &mut control_recv, &session_id, on_event).await
}

async fn drive_send_files(
    endpoint: &Endpoint,
    addr: iroh::EndpointAddr,
    identity: Identity,
    files: Vec<(String, Vec<u8>)>,
    on_event: &Function,
    cancel: &Rc<RefCell<bool>>,
) -> Result<()> {
    let total_bytes: u64 = files.iter().map(|(_, data)| data.len() as u64).sum();

    // Stage each file into an in-memory blob store and wrap them in a collection
    // (path → hash), exactly like the native `PreparedStore` — just MemStore
    // instead of FsStore. Keep every TempTag alive so nothing is GC'd from under
    // the receiver mid-fetch.
    let store: Store = MemStore::new().into();
    let mut collection = Collection::default();
    let mut manifest_items = Vec::with_capacity(files.len());
    let mut tags: Vec<TempTag> = Vec::with_capacity(files.len() + 1);
    for (path, data) in files {
        let size = data.len() as u64;
        let tag = store
            .add_bytes(data)
            .temp_tag()
            .await
            .map_err(|e| anyhow!("staging file bytes: {e}"))?;
        collection.push(path.clone(), tag.hash());
        tags.push(tag);
        manifest_items.push(ManifestItem::File { path, size });
    }
    let collection_tag = collection
        .store(&store)
        .await
        .map_err(|e| anyhow!("storing collection: {e}"))?;
    let collection_hash = collection_tag.hash();
    // Held until the transfer completes (see `_tags` below).
    tags.push(collection_tag);
    let _tags = tags;

    // Serve the blobs ALPN from the tab so the receiver can dial back and fetch.
    let router = Router::builder(endpoint.clone())
        .accept(BLOBS_ALPN, BlobsProtocol::new(&store, None))
        .spawn();

    let manifest = TransferManifest {
        items: manifest_items,
    };
    let session_id = new_session_id();
    let (conn, mut control_send, mut control_recv, decision) = handshake(
        endpoint,
        addr,
        &identity,
        &session_id,
        manifest,
        collection_hash,
        None,
        on_event,
        cancel,
    )
    .await?;

    let outcome = async {
        match decision {
            Decision::Accept => emit(on_event, &SendEvent::Accepted),
            Decision::Decline(reason) => {
                emit(on_event, &SendEvent::Declined { reason });
                return Ok(());
            }
            Decision::Aborted => return Ok(()),
        }

        // Hand the receiver a ticket to our served collection.
        let ticket = BlobTicket::new(endpoint.addr(), collection_hash, BlobFormat::HashSeq);
        wire::write_sender_message(
            &mut control_send,
            &SenderMessage::BlobTicket(BlobTicketMessage {
                session_id: session_id.clone(),
                ticket: ticket.to_string(),
            }),
        )
        .await?;
        emit(on_event, &SendEvent::TransferStarted);

        // The receiver opens a uni stream for progress; read it until it signals
        // TransferCompleted (or ends), then close out on the control stream.
        let mut progress_recv = conn.accept_uni().await?;
        loop {
            let read = async { wire::read_receiver_message(&mut progress_recv).await };
            let gone = async {
                conn.closed().await;
                Err(anyhow!("receiver disconnected mid-transfer"))
            };
            match futures_lite::future::or(read, gone).await {
                Ok(ReceiverMessage::TransferProgress(p)) => emit(
                    on_event,
                    &SendEvent::Progress {
                        bytes_sent: p.snapshot.bytes_transferred.min(total_bytes),
                        total_bytes,
                    },
                ),
                Ok(ReceiverMessage::TransferCompleted(_)) => break,
                Ok(ReceiverMessage::Cancel(_)) => {
                    emit(on_event, &SendEvent::Cancelled);
                    return Ok(());
                }
                // Stream ended without an explicit Completed — the control
                // TransferResult below is the source of truth, so stop reading.
                Ok(_) | Err(_) => break,
            }
        }

        read_result_and_ack(&mut control_send, &mut control_recv, &session_id, on_event).await
    }
    .await;

    // Tear the provider down before the endpoint closes (see start_send).
    let _ = router.shutdown().await;
    drop(_tags);
    outcome
}

/// Read the receiver's terminal `TransferResult`, ack it, and emit `Completed`.
/// Shared tail of both the text and file paths.
async fn read_result_and_ack(
    control_send: &mut SendStream,
    control_recv: &mut RecvStream,
    session_id: &str,
    on_event: &Function,
) -> Result<()> {
    let result = {
        let read = async { wire::read_receiver_message(control_recv).await };
        let timeout = async {
            n0_future::time::sleep(Duration::from_secs(30)).await;
            Err(anyhow!("timed out waiting for the receiver's result"))
        };
        futures_lite::future::or(read, timeout).await?
    };
    match result {
        ReceiverMessage::TransferResult(r) => match r.status {
            TransferStatus::Ok => {}
            TransferStatus::Error { code, message } => {
                bail!("receiver reported an error ({code:?}): {message}")
            }
        },
        ReceiverMessage::Cancel(_) => {
            emit(on_event, &SendEvent::Cancelled);
            return Ok(());
        }
        other => bail!("expected TransferResult, got {:?}", other.kind()),
    }

    let _ = wire::write_sender_message(
        control_send,
        &SenderMessage::TransferAck(TransferAck {
            session_id: session_id.to_owned(),
        }),
    )
    .await;
    let _ = control_send.finish();
    emit(on_event, &SendEvent::Completed);
    Ok(())
}

/// Dial the receiver on the control ALPN with a bounded retry (see
/// [`CONNECT_ATTEMPTS`]).
async fn connect_with_retry(endpoint: &Endpoint, addr: iroh::EndpointAddr) -> Result<Connection> {
    let mut last_err = None;
    for _ in 0..CONNECT_ATTEMPTS {
        let connect = async {
            endpoint
                .connect(addr.clone(), ALPN)
                .await
                .map_err(anyhow::Error::from)
        };
        let timeout = async {
            n0_future::time::sleep(Duration::from_secs(CONNECT_ATTEMPT_SECS)).await;
            Err(anyhow!("connect attempt timed out"))
        };
        match futures_lite::future::or(connect, timeout).await {
            Ok(conn) => return Ok(conn),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("could not connect to the receiver")))
}

fn new_session_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn device_type_label(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::Phone => "phone",
        DeviceType::Laptop => "laptop",
    }
}

fn emit(on_event: &Function, event: &SendEvent) {
    if let Ok(value) = serde_wasm_bindgen::to_value(event) {
        let _ = on_event.call1(&JsValue::NULL, &value);
    }
}
