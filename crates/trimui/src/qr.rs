//! QR rendering for offline pairing.
//!
//! The payload is whatever `ReceiverService::qr_pairing_info()` produced —
//! `"wisp-pair:" + base64url(json)`, the exact string the phone app already
//! knows how to scan — so nothing about the pairing format is re-implemented
//! here. This module only turns that string into modules and paints them.

use anyhow::{Context, Result};
use qrcode::{EcLevel, QrCode};

use crate::draw::{Canvas, Rect};
use crate::fb::Argb;

/// A QR as a square grid of booleans, sized in modules.
pub struct QrImage {
    /// Side length in modules, excluding the quiet zone.
    pub modules: usize,
    dark: Vec<bool>,
}

impl QrImage {
    pub fn encode(payload: &str) -> Result<Self> {
        // The pairing payload carries a full iroh ticket and can run to a few
        // hundred bytes. Error-correction level L keeps the module count (and
        // therefore the on-screen module size) as favourable as possible; the
        // screen is a clean, backlit source, so the extra redundancy of M
        // would cost scannability rather than buy it.
        let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L)
            .context("encode the pairing payload as a QR code")?;
        let modules = code.width();
        let dark = code
            .to_colors()
            .into_iter()
            .map(|color| color == qrcode::Color::Dark)
            .collect();
        Ok(Self { modules, dark })
    }

    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        if x >= self.modules || y >= self.modules {
            return false;
        }
        self.dark[y * self.modules + x]
    }

    /// Largest whole-pixel module size that fits `available` pixels, including
    /// a 4-module quiet zone on each side as the QR spec requires.
    pub fn module_size(&self, available: i32, quiet_modules: i32) -> i32 {
        let total = self.modules as i32 + quiet_modules * 2;
        (available / total).max(1)
    }

    /// Paints the code centred in `area`, on its own light background.
    ///
    /// Returns the rect actually covered, so callers can lay out text
    /// underneath without guessing.
    pub fn draw(&self, canvas: &mut Canvas, area: Rect, dark: Argb, light: Argb) -> Rect {
        const QUIET: i32 = 4;
        let side = area.w.min(area.h);
        let scale = self.module_size(side, QUIET);
        let total = (self.modules as i32 + QUIET * 2) * scale;
        let origin_x = area.x + (area.w - total) / 2;
        let origin_y = area.y + (area.h - total) / 2;
        let frame = Rect::new(origin_x, origin_y, total, total);

        // The quiet zone has to be light, not "whatever was behind it".
        canvas.fill_rect(frame, light);

        for y in 0..self.modules {
            for x in 0..self.modules {
                if !self.is_dark(x, y) {
                    continue;
                }
                canvas.fill_rect(
                    Rect::new(
                        origin_x + (x as i32 + QUIET) * scale,
                        origin_y + (y as i32 + QUIET) * scale,
                        scale,
                        scale,
                    ),
                    dark,
                );
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "wisp-pair:eyJ0aWNrZXQiOiJhYmNkZWYiLCJuYW1lIjoiQnJpY2sgUHJvIn0";

    #[test]
    fn encodes_a_pairing_payload() {
        let qr = QrImage::encode(SAMPLE).expect("payload should encode");
        assert!(qr.modules >= 21, "smallest QR version is 21 modules");
        // The three finder patterns mean the corners are always dark.
        assert!(qr.is_dark(0, 0));
    }

    #[test]
    fn out_of_range_lookups_read_as_light() {
        let qr = QrImage::encode(SAMPLE).unwrap();
        assert!(!qr.is_dark(qr.modules, 0));
        assert!(!qr.is_dark(0, qr.modules));
    }

    #[test]
    fn module_size_leaves_room_for_the_quiet_zone() {
        let qr = QrImage::encode(SAMPLE).unwrap();
        let scale = qr.module_size(420, 4);
        let total = (qr.modules as i32 + 8) * scale;
        assert!(total <= 420, "{total} must fit in 420px");
    }

    #[test]
    fn module_size_never_drops_below_one_pixel() {
        let qr = QrImage::encode(SAMPLE).unwrap();
        assert_eq!(qr.module_size(4, 4), 1);
    }

    #[test]
    fn draw_paints_inside_the_area_and_lights_dark_modules() {
        let qr = QrImage::encode(SAMPLE).unwrap();
        let mut canvas = Canvas::new(400, 400);
        canvas.clear(0xFF00_0000);
        let frame = qr.draw(
            &mut canvas,
            Rect::new(0, 0, 400, 400),
            0xFF00_0000,
            0xFFFF_FFFF,
        );

        assert!(frame.w <= 400 && frame.h <= 400);
        assert!(frame.x >= 0 && frame.y >= 0);

        let light = canvas
            .pixels()
            .iter()
            .filter(|p| **p == 0xFFFF_FFFF)
            .count();
        assert!(light > 0, "the quiet zone should be painted light");
    }

    #[test]
    fn a_long_ticket_still_encodes() {
        // Real payloads carry a full ticket; make sure we are not sized for
        // toy inputs only.
        let long = format!("wisp-pair:{}", "A".repeat(600));
        assert!(QrImage::encode(&long).is_ok());
    }
}
