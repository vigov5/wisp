//! `wisp/transfer/v1` frame codec for the browser side.
//!
//! Same wire format as the native `wisp_core::protocol::wire` — a big-endian
//! `u32` length prefix followed by a JSON [`MessageEnvelope`] — re-implemented
//! here over iroh's streams (which implement tokio's async IO traits, including
//! on wasm). The message *types* come from `wisp-wire`, so the schema stays a
//! single source of truth.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wisp_wire::message::{
    MessageEnvelope, PROTOCOL_VERSION, ReceiverMessage, SenderMessage, TransferRole,
};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Read one framed [`SenderMessage`], validating protocol version and role.
pub async fn read_sender_message<R>(reader: &mut R) -> Result<SenderMessage>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32().await.context("reading frame length")? as usize;
    if len > MAX_MESSAGE_BYTES {
        bail!("frame too large: {len} > {MAX_MESSAGE_BYTES}");
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .context("reading frame body")?;
    let envelope: MessageEnvelope = serde_json::from_slice(&buf).context("parsing envelope")?;
    if envelope.version != PROTOCOL_VERSION {
        bail!(
            "unsupported protocol version {} (want {PROTOCOL_VERSION})",
            envelope.version
        );
    }
    if envelope.role != TransferRole::Sender {
        bail!("expected sender role, got {:?}", envelope.role);
    }
    serde_json::from_value(envelope.message).context("parsing sender message")
}

/// Write one framed [`ReceiverMessage`].
pub async fn write_receiver_message<W>(writer: &mut W, message: &ReceiverMessage) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let envelope = MessageEnvelope {
        version: PROTOCOL_VERSION,
        role: message.role(),
        kind: message.kind(),
        message: serde_json::to_value(message).context("serializing receiver message")?,
    };
    let bytes = serde_json::to_vec(&envelope).context("serializing envelope")?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        bail!("frame too large: {} > {MAX_MESSAGE_BYTES}", bytes.len());
    }
    writer
        .write_u32(bytes.len() as u32)
        .await
        .context("writing frame length")?;
    writer
        .write_all(&bytes)
        .await
        .context("writing frame body")?;
    writer.flush().await.context("flushing frame")?;
    Ok(())
}
