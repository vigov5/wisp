pub(crate) mod error;
pub(crate) mod receive;
pub(crate) mod send;
pub(crate) mod wire;

pub use error::ProtocolError;
// The `wisp/transfer/v1` message schema now lives in the wasm-clean `wisp-wire`
// crate so the browser receiver shares a single source of truth. Re-export it at
// the historical `crate::protocol::message` path (and the flat item paths) so all
// existing call sites resolve unchanged.
pub use wisp_wire::message;
pub use wisp_wire::message::{
    ALPN, CancelPhase, DeviceType, Identity, PROTOCOL_VERSION, TransferRole,
};
