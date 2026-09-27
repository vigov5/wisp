//! Colours and metrics for the 1024x768 panel.
//!
//! The accents are Wisp's own (`kAccentCyan` / `kAccentCyanStrong` from the
//! Flutter app) so the handheld reads as the same product, and the existing
//! rule carries over: a filled call-to-action uses the strong cyan, inline
//! text uses the plain one. The neutrals are dark rather than the app's light
//! palette — this screen is usually looked at in a dim room, and the panel is
//! the only light source in the user's hands.

use crate::fb::Argb;

// Brand accents, shared with the Flutter app.
pub const ACCENT: Argb = 0xFF06_B6D4;
pub const ACCENT_STRONG: Argb = 0xFF08_91B2;

// Dark neutrals.
pub const BG: Argb = 0xFF0E_1214;
pub const SURFACE: Argb = 0xFF18_1E21;
pub const SURFACE_RAISED: Argb = 0xFF22_292D;
pub const BORDER: Argb = 0xFF2F_383D;

pub const TEXT: Argb = 0xFFEC_EFF1;
pub const TEXT_MUTED: Argb = 0xFF93_A1A8;
pub const TEXT_FAINT: Argb = 0xFF60_6E75;
pub const ON_ACCENT: Argb = 0xFF04_1417;

pub const SUCCESS: Argb = 0xFF34_D399;
pub const DANGER: Argb = 0xFFF8_7171;
pub const WARNING: Argb = 0xFFFB_BF24;

// QR panels are always rendered light — scanners cope far better with a dark
// module on a bright background than the inverse.
pub const QR_LIGHT: Argb = 0xFFFF_FFFF;
pub const QR_DARK: Argb = 0xFF00_0000;

/// Text sizes, in pixels, for a 1024x768 panel held at arm's length.
pub mod text {
    pub const TITLE: f32 = 38.0;
    pub const HEADING: f32 = 28.0;
    pub const BODY: f32 = 22.0;
    pub const SMALL: f32 = 18.0;
    /// The pairing code is the one thing read from across a room.
    pub const CODE: f32 = 64.0;
}

/// Layout metrics.
pub mod metrics {
    pub const GUTTER: i32 = 28;
    pub const HEADER_HEIGHT: i32 = 74;
    pub const FOOTER_HEIGHT: i32 = 56;
    pub const RADIUS: i32 = 14;
    pub const ROW_HEIGHT: i32 = 56;
    pub const GAP: i32 = 16;
}
