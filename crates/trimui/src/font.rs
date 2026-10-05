//! Text rendering on top of [`Canvas`], using the Noto Sans the Flutter app
//! already ships. Reusing that file keeps one font (and one OFL notice) in the
//! repository instead of adding a second copy for this target.
//!
//! `fontdue` reads the default instance of the variable font, which is the
//! regular weight. There is no bold master available that way, so emphasis is
//! faked by over-striking — see [`Fonts::draw`].

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::draw::{Canvas, Rect};
use crate::fb::Argb;

const FONT_DATA: &[u8] = include_bytes!("../../../flutter/assets/fonts/NotoSans-Variable.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A rasterised glyph: coverage bitmap plus where to put it relative to the pen.
struct Glyph {
    width: usize,
    height: usize,
    xmin: i32,
    ymin: i32,
    advance: f32,
    coverage: Vec<u8>,
}

pub struct Fonts {
    font: fontdue::Font,
    cache: HashMap<(char, u32), Glyph>,
}

impl Fonts {
    pub fn load() -> Result<Self> {
        let font = fontdue::Font::from_bytes(FONT_DATA, fontdue::FontSettings::default())
            .map_err(|err| anyhow::anyhow!("{err}"))
            .context("parse the bundled Noto Sans")?;
        Ok(Self {
            font,
            cache: HashMap::new(),
        })
    }

    /// Height of one line box at `size`, used for all vertical layout.
    pub fn line_height(&self, size: f32) -> i32 {
        match self.font.horizontal_line_metrics(size) {
            Some(metrics) => (metrics.ascent - metrics.descent + metrics.line_gap).ceil() as i32,
            None => (size * 1.3).ceil() as i32,
        }
    }

    fn ascent(&self, size: f32) -> f32 {
        self.font
            .horizontal_line_metrics(size)
            .map(|m| m.ascent)
            .unwrap_or(size)
    }

    fn glyph(&mut self, ch: char, size: f32) -> &Glyph {
        // Sizes are always whole pixels here, so quantising the cache key by
        // `to_bits` is exact rather than a rounding hazard.
        let key = (ch, size.to_bits());
        self.cache.entry(key).or_insert_with(|| {
            let (metrics, coverage) = self.font.rasterize(ch, size);
            Glyph {
                width: metrics.width,
                height: metrics.height,
                xmin: metrics.xmin,
                ymin: metrics.ymin,
                advance: metrics.advance_width,
                coverage,
            }
        })
    }

    /// Width of `text` in pixels, without drawing it.
    pub fn measure(&mut self, text: &str, size: f32) -> i32 {
        let mut width = 0.0f32;
        for ch in text.chars() {
            width += self.glyph(ch, size).advance;
        }
        width.ceil() as i32
    }

    /// Draws `text` with its top-left at `(x, y)`. Returns the advance width.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        x: i32,
        y: i32,
        text: &str,
        size: f32,
        color: Argb,
        weight: Weight,
    ) -> i32 {
        let baseline = y as f32 + self.ascent(size);
        let mut pen = x as f32;
        for ch in text.chars() {
            let glyph = self.glyph(ch, size);
            let left = pen.round() as i32 + glyph.xmin;
            let top = baseline.round() as i32 - glyph.height as i32 - glyph.ymin;
            let (w, h) = (glyph.width, glyph.height);
            let advance = glyph.advance;

            // `glyph` stays borrowed from the cache for the whole blit: the
            // canvas is a separate parameter, so nothing here needs `self`
            // again. Copying the coverage out instead would put a heap
            // allocation per character per frame on a 1 GHz handheld.
            for row in 0..h {
                for col in 0..w {
                    let alpha = glyph.coverage[row * w + col];
                    if alpha == 0 {
                        continue;
                    }
                    canvas.blend(left + col as i32, top + row as i32, color, alpha);
                    if weight == Weight::Bold {
                        // Over-strike one pixel right: the variable font's
                        // bold master is not reachable through fontdue.
                        canvas.blend(left + col as i32 + 1, top + row as i32, color, alpha);
                    }
                }
            }
            pen += advance;
        }
        (pen - x as f32).ceil() as i32
    }

    /// Draws `text` inside `rect`, aligned horizontally and clipped with an
    /// ellipsis when it does not fit.
    pub fn draw_in(
        &mut self,
        canvas: &mut Canvas,
        rect: Rect,
        text: &str,
        size: f32,
        color: Argb,
        weight: Weight,
        align: Align,
    ) -> i32 {
        let text = self.ellipsize(text, size, rect.w);
        let width = self.measure(&text, size);
        let x = match align {
            Align::Left => rect.x,
            Align::Center => rect.x + (rect.w - width) / 2,
            Align::Right => rect.right() - width,
        };
        self.draw(canvas, x, rect.y, &text, size, color, weight)
    }

    /// Truncates `text` with a trailing ellipsis so it fits `max_width`.
    pub fn ellipsize(&mut self, text: &str, size: f32, max_width: i32) -> String {
        if self.measure(text, size) <= max_width {
            return text.to_owned();
        }
        let ellipsis = "…";
        let ellipsis_width = self.measure(ellipsis, size);
        let budget = max_width - ellipsis_width;
        if budget <= 0 {
            return String::new();
        }
        let mut out = String::new();
        let mut width = 0.0f32;
        for ch in text.chars() {
            let advance = self.glyph(ch, size).advance;
            if (width + advance).ceil() as i32 > budget {
                break;
            }
            width += advance;
            out.push(ch);
        }
        out.push('…');
        out
    }

    /// Greedy word wrap. Falls back to breaking mid-word for a single word
    /// longer than the line, which filenames routinely are.
    pub fn wrap(&mut self, text: &str, size: f32, max_width: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            if paragraph.is_empty() {
                lines.push(String::new());
                continue;
            }
            let mut current = String::new();
            for word in paragraph.split(' ') {
                let candidate = if current.is_empty() {
                    word.to_owned()
                } else {
                    format!("{current} {word}")
                };
                if self.measure(&candidate, size) <= max_width {
                    current = candidate;
                    continue;
                }
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                if self.measure(word, size) <= max_width {
                    current = word.to_owned();
                } else {
                    // Hard-break the oversized word.
                    let mut chunk = String::new();
                    for ch in word.chars() {
                        let mut probe = chunk.clone();
                        probe.push(ch);
                        if self.measure(&probe, size) > max_width && !chunk.is_empty() {
                            lines.push(std::mem::take(&mut chunk));
                        }
                        chunk.push(ch);
                    }
                    current = chunk;
                }
            }
            lines.push(current);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Fonts {
        Fonts::load().expect("the bundled font must parse")
    }

    #[test]
    fn font_loads_and_has_a_positive_line_height() {
        let f = fonts();
        assert!(f.line_height(24.0) > 0);
    }

    #[test]
    fn measure_grows_with_text_length() {
        let mut f = fonts();
        let short = f.measure("Wisp", 24.0);
        let long = f.measure("Wisp Wisp", 24.0);
        assert!(long > short, "{long} should exceed {short}");
    }

    #[test]
    fn vietnamese_diacritics_rasterise() {
        let mut f = fonts();
        // A missing glyph rasterises to a zero-size box, so a positive width
        // is what proves the bundled font really covers these.
        assert!(f.measure("Nhận tệp", 24.0) > 0);
        let mut canvas = Canvas::new(200, 60);
        canvas.clear(0xFF00_0000);
        f.draw(
            &mut canvas,
            4,
            4,
            "Nhận tệp",
            24.0,
            0xFFFF_FFFF,
            Weight::Regular,
        );
        let lit = canvas
            .pixels()
            .iter()
            .filter(|p| **p != 0xFF00_0000)
            .count();
        assert!(lit > 0, "drawing should light up pixels");
    }

    #[test]
    fn ellipsize_shortens_only_when_needed() {
        let mut f = fonts();
        let text = "a short label";
        let wide = f.measure(text, 20.0) + 10;
        assert_eq!(f.ellipsize(text, 20.0, wide), text);

        let clipped = f.ellipsize(text, 20.0, 40);
        assert!(clipped.ends_with('…'));
        assert!(f.measure(&clipped, 20.0) <= 40);
    }

    #[test]
    fn ellipsize_handles_a_width_too_small_for_the_ellipsis() {
        let mut f = fonts();
        assert_eq!(f.ellipsize("something", 20.0, 1), "");
    }

    #[test]
    fn wrap_breaks_long_text_into_fitting_lines() {
        let mut f = fonts();
        let lines = f.wrap("the quick brown fox jumps over the lazy dog", 20.0, 120);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(f.measure(line, 20.0) <= 120, "line too wide: {line:?}");
        }
    }

    #[test]
    fn wrap_hard_breaks_a_word_longer_than_the_line() {
        let mut f = fonts();
        let lines = f.wrap("supercalifragilisticexpialidocious", 20.0, 60);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(f.measure(line, 20.0) <= 60, "line too wide: {line:?}");
        }
    }

    #[test]
    fn wrap_keeps_explicit_blank_lines() {
        let mut f = fonts();
        let lines = f.wrap("one\n\ntwo", 20.0, 400);
        assert_eq!(lines, vec!["one", "", "two"]);
    }
}
