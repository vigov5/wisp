//! Pure, wasm-clean wire types shared by the native app (`wisp-core`) and the
//! browser receiver (`wisp-web-receiver`).
//!
//! Everything here compiles to `wasm32-unknown-unknown`: the `wisp/transfer/v1`
//! control-protocol message schema, the transfer-plan value types those messages
//! carry, the base64/bincode ticket codec, and the rendezvous HTTP client. Keep
//! this crate free of native-only dependencies (mdns, netdev, filesystem stores,
//! multi-threaded tokio) so the schema stays a single source of truth across both
//! targets.

pub mod message;
pub mod plan;
pub mod rendezvous;
pub mod ticket;
