//! Wisp receiver for the TrimUI Brick Pro (TG4040).
//!
//! The binary in `main.rs` is a thin frame loop over these modules. They are a
//! library so the UI can also be driven by `examples/preview.rs`, which
//! renders every screen to a PNG — the only way to check the layout without
//! the handheld in hand.

pub mod app;
pub mod config;
pub mod draw;
pub mod engine;
pub mod fb;
pub mod font;
pub mod i18n;
pub mod input;
pub mod qr;
pub mod theme;
pub mod ui;
