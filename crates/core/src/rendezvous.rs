//! Rendezvous client + DTOs moved to the wasm-clean `wisp-wire` crate so the
//! browser receiver can register/poll with the same code. Re-exported here at
//! the historical `crate::rendezvous` path.
//!
//! The one native-only piece is [`resolve_server_url`], which reads the
//! `WISP_RENDEZVOUS_URL` environment variable — meaningless in a browser, so
//! `wisp-wire` only exposes the pure `resolve_server_url_with_env` that this
//! wraps.

pub use wisp_wire::rendezvous::*;

pub fn resolve_server_url(override_url: Option<&str>) -> String {
    resolve_server_url_with_env(override_url, std::env::var("WISP_RENDEZVOUS_URL").ok())
}
