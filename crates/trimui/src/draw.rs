//! A small software canvas: opaque `0xAARRGGBB` pixels, source-over blending.
//!
//! Everything the UI draws goes through here and only [`Canvas::pixels`] is
//! handed to the framebuffer, which keeps the rendering code testable on any
//! host — there is no device dependency in this module.

use crate::fb::Argb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Shrinks the rect by `amount` on every side.
    pub fn inset(&self, amount: i32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            w: (self.w - amount * 2).max(0),
            h: (self.h - amount * 2).max(0),
        }
    }

    pub fn with_height(&self, h: i32) -> Self {
        Self { h, ..*self }
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }
}

pub struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<Argb>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as i32,
            height: height as i32,
            pixels: vec![0xFF00_0000; (width as usize) * (height as usize)],
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub fn pixels(&self) -> &[Argb] {
        &self.pixels
    }

    pub fn clear(&mut self, color: Argb) {
        self.pixels.fill(color | 0xFF00_0000);
    }

    /// Source-over blend of one pixel. `coverage` is 0..=255 and is multiplied
    /// into the colour's own alpha, which is how the glyph rasteriser feeds
    /// antialiasing in.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, color: Argb, coverage: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let src_alpha = ((color >> 24) & 0xFF) * coverage as u32 / 255;
        if src_alpha == 0 {
            return;
        }
        let index = (y as usize) * (self.width as usize) + (x as usize);
        if src_alpha >= 255 {
            self.pixels[index] = color | 0xFF00_0000;
            return;
        }
        let dst = self.pixels[index];
        let inv = 255 - src_alpha;
        let mix = |shift: u32| -> u32 {
            let s = (color >> shift) & 0xFF;
            let d = (dst >> shift) & 0xFF;
            ((s * src_alpha + d * inv) / 255) & 0xFF
        };
        self.pixels[index] = 0xFF00_0000 | (mix(16) << 16) | (mix(8) << 8) | mix(0);
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Argb) {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = rect.right().min(self.width);
        let y1 = rect.bottom().min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let opaque = (color >> 24) & 0xFF == 0xFF;
        for y in y0..y1 {
            if opaque {
                let row = (y as usize) * (self.width as usize);
                self.pixels[row + x0 as usize..row + x1 as usize].fill(color);
            } else {
                for x in x0..x1 {
                    self.blend(x, y, color, 255);
                }
            }
        }
    }

    pub fn stroke_rect(&mut self, rect: Rect, thickness: i32, color: Argb) {
        if thickness <= 0 {
            return;
        }
        self.fill_rect(rect.with_height(thickness), color);
        self.fill_rect(
            Rect::new(rect.x, rect.bottom() - thickness, rect.w, thickness),
            color,
        );
        self.fill_rect(Rect::new(rect.x, rect.y, thickness, rect.h), color);
        self.fill_rect(
            Rect::new(rect.right() - thickness, rect.y, thickness, rect.h),
            color,
        );
    }

    /// Filled rounded rectangle. Corners are antialiased by sampling the
    /// circle equation per pixel, which is cheap enough at these sizes and
    /// avoids pulling in a rasteriser for four arcs.
    pub fn fill_round_rect(&mut self, rect: Rect, radius: i32, color: Argb) {
        let radius = radius.min(rect.w / 2).min(rect.h / 2).max(0);
        if radius == 0 {
            self.fill_rect(rect, color);
            return;
        }

        // Straight middle band, then the two rounded caps.
        self.fill_rect(
            Rect::new(rect.x, rect.y + radius, rect.w, rect.h - radius * 2),
            color,
        );

        let r = radius as f32;
        for dy in 0..radius {
            // Distance from the corner centre to the edge of the circle at
            // this row, used for both the top and bottom caps.
            let y = r - 0.5 - dy as f32;
            let half = (r * r - y * y).max(0.0).sqrt();
            let inset = r - half;
            let whole = inset.floor() as i32;
            let coverage = (255.0 * (1.0 - (inset - inset.floor()))) as u8;

            for (row, _) in [(rect.y + dy, 0), (rect.bottom() - 1 - dy, 1)] {
                let x0 = rect.x + whole;
                let x1 = rect.right() - whole;
                if x1 > x0 {
                    self.fill_rect(Rect::new(x0 + 1, row, (x1 - x0 - 2).max(0), 1), color);
                    self.blend(x0, row, color, coverage);
                    self.blend(x1 - 1, row, color, coverage);
                }
            }
        }
    }

    /// Horizontal separator line.
    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: Argb) {
        self.fill_rect(Rect::new(x, y, w, 1), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(canvas: &Canvas, x: i32, y: i32) -> Argb {
        canvas.pixels()[(y as usize) * (canvas.width() as usize) + x as usize]
    }

    #[test]
    fn clear_forces_opaque_alpha() {
        let mut canvas = Canvas::new(4, 4);
        canvas.clear(0x0012_3456);
        assert_eq!(pixel(&canvas, 0, 0), 0xFF12_3456);
    }

    #[test]
    fn fill_rect_clips_to_the_canvas() {
        let mut canvas = Canvas::new(4, 4);
        canvas.clear(0xFF00_0000);
        canvas.fill_rect(Rect::new(-2, -2, 3, 3), 0xFFFF_0000);
        assert_eq!(pixel(&canvas, 0, 0), 0xFFFF_0000);
        assert_eq!(pixel(&canvas, 1, 1), 0xFF00_0000);
    }

    #[test]
    fn blend_mixes_towards_the_source() {
        let mut canvas = Canvas::new(2, 2);
        canvas.clear(0xFF00_0000);
        canvas.blend(0, 0, 0xFFFF_FFFF, 128);
        let mixed = pixel(&canvas, 0, 0) & 0xFF;
        assert!((100..=140).contains(&mixed), "got {mixed}");
    }

    #[test]
    fn zero_coverage_leaves_the_destination_alone() {
        let mut canvas = Canvas::new(2, 2);
        canvas.clear(0xFF01_0203);
        canvas.blend(0, 0, 0xFFFF_FFFF, 0);
        assert_eq!(pixel(&canvas, 0, 0), 0xFF01_0203);
    }

    #[test]
    fn round_rect_clears_its_corners_but_fills_its_middle() {
        let mut canvas = Canvas::new(20, 20);
        canvas.clear(0xFF00_0000);
        canvas.fill_round_rect(Rect::new(0, 0, 20, 20), 6, 0xFFFF_FFFF);
        assert_eq!(pixel(&canvas, 10, 10), 0xFFFF_FFFF, "middle is filled");
        assert_eq!(pixel(&canvas, 0, 0), 0xFF00_0000, "corner is untouched");
    }

    #[test]
    fn stroke_rect_draws_the_border_only() {
        let mut canvas = Canvas::new(10, 10);
        canvas.clear(0xFF00_0000);
        canvas.stroke_rect(Rect::new(0, 0, 10, 10), 1, 0xFFFF_FFFF);
        assert_eq!(pixel(&canvas, 0, 5), 0xFFFF_FFFF);
        assert_eq!(pixel(&canvas, 5, 5), 0xFF00_0000);
    }

    #[test]
    fn inset_never_produces_a_negative_size() {
        assert_eq!(Rect::new(0, 0, 4, 4).inset(10), Rect::new(10, 10, 0, 0));
    }
}
