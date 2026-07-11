//! Browser (wasm32) file receiver for drift.
//!
//! A relay-only iroh endpoint + iroh-blobs `MemStore` that plays the receiver
//! half of the `wisp/transfer/v1` v4 control protocol (shared schema from
//! `wisp-wire`). All file bytes ride n0 public relays end-to-end; the static
//! page that loads this wasm module carries none of them.
//!
//! Spike 1: hard-coded Accept (no UI gating yet). Registers with the rendezvous
//! server, displays a code, accepts one inbound control connection, runs the
//! handshake, fetches the collection into memory, and triggers browser
//! downloads. Accept/Decline UI, progress polish, and the inline-text path land
//! in later stages.

mod download;
mod wire;

use anyhow::{Context, Result, bail};
use futures_lite::StreamExt;
use iroh::Endpoint;
use iroh::endpoint::Connection;
use iroh_blobs::api::Store;
use iroh_blobs::api::remote::GetProgressItem;
use iroh_blobs::format::collection::Collection;
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::{ALPN as BLOBS_ALPN, Hash};
use js_sys::Function;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use wisp_wire::message::{
    Accept, DeviceType, Hello, Identity, ManifestItem, PROTOCOL_VERSION, ReceiverMessage,
    SenderMessage, TransferCompleted, TransferProgressPayload, TransferResult, TransferRole,
    TransferStatus,
};
use wisp_wire::plan::TransferPhase;
use wisp_wire::rendezvous::RendezvousClient;

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
    Registered {
        code: String,
    },
    #[serde(rename_all = "camelCase")]
    Offer {
        files: Vec<OfferItem>,
        total_bytes: u64,
        inline_text: Option<String>,
    },
    TransferStarted,
    #[serde(rename_all = "camelCase")]
    Progress {
        bytes_received: u64,
        total_bytes: u64,
    },
    FileReady {
        path: String,
        size: u64,
        url: String,
    },
    Completed,
    Error {
        message: String,
    },
}

#[derive(Serialize)]
struct OfferItem {
    path: String,
    size: u64,
}

#[wasm_bindgen]
pub struct WebReceiver {
    code: String,
}

#[wasm_bindgen]
impl WebReceiver {
    /// Bind a relay-only endpoint, register with the rendezvous server, and
    /// start accepting inbound transfers in the background. Resolves once the
    /// 6-char code is known; `on_event` streams progress thereafter.
    #[wasm_bindgen(js_name = start)]
    pub async fn start(rendezvous_url: String, on_event: Function) -> Result<WebReceiver, JsValue> {
        run_start(rendezvous_url, on_event)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:#}")))
    }

    /// The 6-char pairing code the sender enters.
    #[wasm_bindgen(js_name = code)]
    pub fn code(&self) -> String {
        self.code.clone()
    }
}

async fn run_start(rendezvous_url: String, on_event: Function) -> Result<WebReceiver> {
    let store: Store = MemStore::new().into();

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
    emit(&on_event, &Event::Registered { code: code.clone() });

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
            if let Err(err) = handle_connection(&endpoint, &store, &conn, &on_event).await {
                emit(
                    &on_event,
                    &Event::Error {
                        message: format!("{err:#}"),
                    },
                );
            }
        }
    });

    Ok(WebReceiver { code })
}

async fn handle_connection(
    endpoint: &Endpoint,
    store: &Store,
    conn: &Connection,
    on_event: &Function,
) -> Result<()> {
    let (mut control_send, mut control_recv) = conn.accept_bi().await?;

    // Handshake: read sender Hello, reply with our Hello.
    let hello = match wire::read_sender_message(&mut control_recv).await? {
        SenderMessage::Hello(hello) => hello,
        other => bail!("expected Hello, got {:?}", other.kind()),
    };
    let session_id = hello.session_id.clone();
    let our_identity = Identity {
        role: TransferRole::Receiver,
        endpoint_id: endpoint.id(),
        device_name: "Browser".to_owned(),
        device_type: DeviceType::Laptop,
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
        },
    );

    // Spike 1 handles the blob (file) path only; inline text is a later stage.
    if offer.inline_text.is_some() {
        bail!("inline-text transfers not handled yet (Spike 1 is file-only)");
    }

    // Hard-coded Accept (no UI gating in Spike 1).
    wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::Accept(Accept {
            session_id: session_id.clone(),
        }),
    )
    .await?;
    emit(on_event, &Event::TransferStarted);

    // MUST open the uni progress stream even if we never write to it, or the
    // sender's `accept_uni()` deadlocks.
    let mut progress_send = conn.open_uni().await?;

    // BlobTicket → dial the sender on the blobs ALPN and fetch the collection.
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
    let mut stream = store
        .remote()
        .fetch(connection, blob_ticket.clone())
        .stream();
    while let Some(item) = stream.next().await {
        match item {
            GetProgressItem::Progress(offset) => emit(
                on_event,
                &Event::Progress {
                    bytes_received: offset,
                    total_bytes,
                },
            ),
            GetProgressItem::Done(_) => break,
            GetProgressItem::Error(err) => bail!("blob fetch failed: {err}"),
        }
    }

    // Read the collection (path → hash), then hand each file to the browser.
    let root_hash: Hash = blob_ticket.hash();
    let collection = Collection::load(root_hash, store).await?;
    for (path, hash) in collection.into_iter() {
        let bytes = store.get_bytes(hash).await?;
        let url = download::trigger_download(&path, &bytes)?;
        emit(
            on_event,
            &Event::FileReady {
                path,
                size: bytes.len() as u64,
                url,
            },
        );
    }

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

    wire::write_receiver_message(
        &mut control_send,
        &ReceiverMessage::TransferResult(TransferResult {
            session_id: session_id.clone(),
            status: TransferStatus::Ok,
        }),
    )
    .await?;

    // Best-effort: wait for the sender's final ack before closing.
    match wire::read_sender_message(&mut control_recv).await {
        Ok(SenderMessage::TransferAck(_)) => {}
        Ok(other) => tracing::warn!("expected TransferAck, got {:?}", other.kind()),
        Err(err) => tracing::warn!("reading TransferAck: {err:#}"),
    }

    emit(on_event, &Event::Completed);
    Ok(())
}

fn emit(on_event: &Function, event: &Event) {
    if let Ok(value) = serde_wasm_bindgen::to_value(event) {
        let _ = on_event.call1(&JsValue::NULL, &value);
    }
}
