//! Reusable chrome: header, footer hints, panels, lists and the progress bar.
//!
//! Every helper is pure drawing over [`Canvas`], so a screen can be rendered
//! and asserted on in a unit test without a framebuffer.

use crate::draw::{Canvas, Rect};
use crate::fb::Argb;
use crate::font::{Align, Fonts, Weight};
use crate::theme::{self, metrics, text};

/// One row in a menu or file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub title: String,
    pub subtitle: Option<String>,
    /// Right-aligned value, such as a file size or the current setting.
    pub value: Option<String>,
}

impl Row {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            value: None,
        }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    fn height(&self) -> i32 {
        if self.subtitle.is_some() {
            metrics::ROW_HEIGHT + 22
        } else {
            metrics::ROW_HEIGHT
        }
    }
}

/// Draws the title bar and returns the rect left for content.
pub fn header(canvas: &mut Canvas, fonts: &mut Fonts, title: &str, status: Option<&str>) -> Rect {
    let bounds = canvas.bounds();
    canvas.fill_rect(bounds.with_height(metrics::HEADER_HEIGHT), theme::SURFACE);
    canvas.hline(0, metrics::HEADER_HEIGHT - 1, bounds.w, theme::BORDER);

    let line = metrics::HEADER_HEIGHT / 2 - fonts.line_height(text::HEADING) / 2;
    fonts.draw(
        canvas,
        metrics::GUTTER,
        line,
        title,
        text::HEADING,
        theme::TEXT,
        Weight::Bold,
    );

    if let Some(status) = status {
        let small_line = metrics::HEADER_HEIGHT / 2 - fonts.line_height(text::SMALL) / 2;
        let width = fonts.measure(status, text::SMALL);
        fonts.draw(
            canvas,
            bounds.w - metrics::GUTTER - width,
            small_line,
            status,
            text::SMALL,
            theme::TEXT_MUTED,
            Weight::Regular,
        );
    }

    Rect::new(
        metrics::GUTTER,
        metrics::HEADER_HEIGHT + metrics::GAP,
        bounds.w - metrics::GUTTER * 2,
        bounds.h - metrics::HEADER_HEIGHT - metrics::FOOTER_HEIGHT - metrics::GAP * 2,
    )
}

/// Draws the button legend along the bottom edge.
pub fn footer(canvas: &mut Canvas, fonts: &mut Fonts, hints: &[(&str, &str)]) {
    let bounds = canvas.bounds();
    let top = bounds.h - metrics::FOOTER_HEIGHT;
    canvas.fill_rect(
        Rect::new(0, top, bounds.w, metrics::FOOTER_HEIGHT),
        theme::SURFACE,
    );
    canvas.hline(0, top, bounds.w, theme::BORDER);

    let mut x = metrics::GUTTER;
    let label_y = top + metrics::FOOTER_HEIGHT / 2 - fonts.line_height(text::SMALL) / 2;
    for (button, action) in hints {
        let badge_width = fonts.measure(button, text::SMALL).max(18) + 18;
        let badge = Rect::new(x, top + 13, badge_width, 30);
        canvas.fill_round_rect(badge, 8, theme::SURFACE_RAISED);
        fonts.draw_in(
            canvas,
            Rect::new(badge.x, badge.y + 5, badge.w, 24),
            button,
            text::SMALL,
            theme::TEXT,
            Weight::Bold,
            Align::Center,
        );
        x += badge_width + 8;
        let width = fonts.draw(
            canvas,
            x,
            label_y,
            action,
            text::SMALL,
            theme::TEXT_MUTED,
            Weight::Regular,
        );
        x += width + metrics::GAP + 6;
    }
}

/// A raised surface with a border, the base for most content blocks.
pub fn panel(canvas: &mut Canvas, rect: Rect) {
    canvas.fill_round_rect(rect, metrics::RADIUS, theme::SURFACE);
    // A 1px border reads as a subtle edge at this DPI without looking drawn-on.
    canvas.stroke_rect(rect, 1, theme::BORDER);
}

/// Small coloured capsule used for status ("Đang chờ", "Tin cậy", ...).
/// Returns its width so callers can lay out after it.
pub fn pill(
    canvas: &mut Canvas,
    fonts: &mut Fonts,
    x: i32,
    y: i32,
    label: &str,
    foreground: Argb,
    background: Argb,
) -> i32 {
    let width = fonts.measure(label, text::SMALL) + 24;
    let height = fonts.line_height(text::SMALL) + 10;
    canvas.fill_round_rect(Rect::new(x, y, width, height), height / 2, background);
    fonts.draw_in(
        canvas,
        Rect::new(x, y + 5, width, height),
        label,
        text::SMALL,
        foreground,
        Weight::Regular,
        Align::Center,
    );
    width
}

/// Horizontal progress bar. `fraction` is clamped to 0.0..=1.0.
pub fn progress_bar(canvas: &mut Canvas, rect: Rect, fraction: f32, color: Argb) {
    let fraction = fraction.clamp(0.0, 1.0);
    canvas.fill_round_rect(rect, rect.h / 2, theme::SURFACE_RAISED);
    let filled = (rect.w as f32 * fraction).round() as i32;
    if filled <= 0 {
        return;
    }
    // Keep the cap round even at tiny fractions, otherwise the bar starts as a
    // 1px sliver that looks like a rendering fault.
    let filled = filled.max(rect.h);
    canvas.fill_round_rect(
        Rect::new(rect.x, rect.y, filled.min(rect.w), rect.h),
        rect.h / 2,
        color,
    );
}

/// Which slice of a list is on screen.
///
/// With a `selected` row the window follows it; without one the caller drives
/// the offset directly, which is what a read-only list (an incoming file
/// manifest) wants — there is nothing to pick, so showing a cursor would
/// invite a press that does nothing.
///
/// Split out as a pure function because off-by-one scrolling is the classic
/// bug in this kind of UI and it deserves its own tests.
pub fn scroll_offset(
    count: usize,
    selected: Option<usize>,
    visible: usize,
    current: usize,
) -> usize {
    if count <= visible || visible == 0 {
        return 0;
    }
    let max_offset = count - visible;
    let mut offset = current.min(max_offset);
    if let Some(selected) = selected {
        if selected < offset {
            offset = selected;
        } else if selected >= offset + visible {
            offset = selected + 1 - visible;
        }
    }
    offset.min(max_offset)
}

/// Draws a scrolling list of rows and returns the new scroll offset.
pub fn list(
    canvas: &mut Canvas,
    fonts: &mut Fonts,
    area: Rect,
    rows: &[Row],
    selected: Option<usize>,
    scroll: usize,
    empty_label: &str,
) -> usize {
    if rows.is_empty() {
        fonts.draw_in(
            canvas,
            Rect::new(area.x, area.y + 8, area.w, area.h),
            empty_label,
            text::BODY,
            theme::TEXT_FAINT,
            Weight::Regular,
            Align::Center,
        );
        return 0;
    }

    // Rows can differ in height, so size the window off the tallest one to
    // guarantee whatever is drawn actually fits.
    let tallest = rows
        .iter()
        .map(Row::height)
        .max()
        .unwrap_or(metrics::ROW_HEIGHT);
    let visible = ((area.h + metrics::GAP) / (tallest + metrics::GAP)).max(1) as usize;
    let offset = scroll_offset(rows.len(), selected, visible, scroll);

    let mut y = area.y;
    for (index, row) in rows.iter().enumerate().skip(offset).take(visible) {
        let height = row.height();
        let rect = Rect::new(area.x, y, area.w, height);
        let is_selected = selected == Some(index);

        if is_selected {
            canvas.fill_round_rect(rect, metrics::RADIUS, theme::SURFACE_RAISED);
            // A cyan spine on the left is the selection cue; a full cyan fill
            // at this size would drown the text it is meant to highlight.
            canvas.fill_round_rect(
                Rect::new(rect.x, rect.y + 8, 4, height - 16),
                2,
                theme::ACCENT,
            );
        }

        let text_x = rect.x + metrics::GAP;
        let value_width = row
            .value
            .as_deref()
            .map(|value| fonts.measure(value, text::BODY) + metrics::GAP)
            .unwrap_or(0);
        let title_width = rect.w - metrics::GAP * 2 - value_width;

        let title_y = if row.subtitle.is_some() {
            rect.y + 10
        } else {
            rect.y + (height - fonts.line_height(text::BODY)) / 2
        };
        fonts.draw_in(
            canvas,
            Rect::new(text_x, title_y, title_width, height),
            &row.title,
            text::BODY,
            if is_selected {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            },
            if is_selected {
                Weight::Bold
            } else {
                Weight::Regular
            },
            Align::Left,
        );

        if let Some(subtitle) = &row.subtitle {
            fonts.draw_in(
                canvas,
                Rect::new(
                    text_x,
                    title_y + fonts.line_height(text::BODY) + 2,
                    title_width,
                    height,
                ),
                subtitle,
                text::SMALL,
                theme::TEXT_FAINT,
                Weight::Regular,
                Align::Left,
            );
        }

        if let Some(value) = &row.value {
            let value_y = rect.y + (height - fonts.line_height(text::BODY)) / 2;
            fonts.draw_in(
                canvas,
                Rect::new(
                    rect.right() - metrics::GAP - value_width,
                    value_y,
                    value_width,
                    height,
                ),
                value,
                text::BODY,
                theme::ACCENT,
                Weight::Regular,
                Align::Right,
            );
        }

        y += height + metrics::GAP;
    }

    // Scrollbar, only when there is something to scroll.
    if rows.len() > visible {
        let track = Rect::new(area.right() - 4, area.y, 4, area.h);
        canvas.fill_round_rect(track, 2, theme::SURFACE_RAISED);
        let thumb_height = ((visible as f32 / rows.len() as f32) * area.h as f32).max(24.0) as i32;
        let span = (area.h - thumb_height).max(0) as f32;
        let progress = offset as f32 / (rows.len() - visible) as f32;
        canvas.fill_round_rect(
            Rect::new(track.x, area.y + (span * progress) as i32, 4, thumb_height),
            2,
            theme::BORDER,
        );
    }

    offset
}

/// Multi-line paragraph, clipped to `area`. Returns the number of lines that
/// the text needed, so a caller can drive scrolling.
pub fn paragraph(
    canvas: &mut Canvas,
    fonts: &mut Fonts,
    area: Rect,
    body: &str,
    size: f32,
    color: Argb,
    scroll: usize,
) -> usize {
    let lines = fonts.wrap(body, size, area.w);
    let line_height = fonts.line_height(size);
    let visible = (area.h / line_height).max(1) as usize;
    let mut y = area.y;
    for line in lines.iter().skip(scroll).take(visible) {
        fonts.draw(canvas, area.x, y, line, size, color, Weight::Regular);
        y += line_height;
    }
    lines.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Fonts {
        Fonts::load().unwrap()
    }

    #[test]
    fn scroll_offset_stays_zero_when_everything_fits() {
        assert_eq!(scroll_offset(3, Some(2), 5, 0), 0);
    }

    #[test]
    fn scroll_offset_follows_the_selection_downwards() {
        // 10 rows, 4 visible, selecting row 5 must scroll to show it.
        assert_eq!(scroll_offset(10, Some(5), 4, 0), 2);
    }

    #[test]
    fn scroll_offset_follows_the_selection_upwards() {
        assert_eq!(scroll_offset(10, Some(1), 4, 6), 1);
    }

    #[test]
    fn scroll_offset_never_runs_past_the_end() {
        assert_eq!(scroll_offset(10, Some(9), 4, 99), 6);
    }

    #[test]
    fn scroll_offset_handles_a_zero_height_window() {
        assert_eq!(scroll_offset(10, Some(5), 0, 3), 0);
    }

    #[test]
    fn a_list_without_a_selection_scrolls_where_it_is_told() {
        assert_eq!(scroll_offset(10, None, 4, 3), 3);
        assert_eq!(scroll_offset(10, None, 4, 99), 6, "still clamped");
    }

    #[test]
    fn header_returns_a_content_rect_inside_the_canvas() {
        let mut canvas = Canvas::new(1024, 768);
        let mut fonts = fonts();
        let content = header(&mut canvas, &mut fonts, "Wisp", Some("sẵn sàng"));
        assert!(content.y > metrics::HEADER_HEIGHT);
        assert!(content.bottom() <= 768 - metrics::FOOTER_HEIGHT);
        assert!(content.w > 0 && content.h > 0);
    }

    #[test]
    fn progress_bar_clamps_out_of_range_fractions() {
        let mut canvas = Canvas::new(200, 40);
        canvas.clear(theme::BG);
        // Neither of these may panic or paint outside the rect.
        progress_bar(&mut canvas, Rect::new(10, 10, 100, 12), -3.0, theme::ACCENT);
        progress_bar(&mut canvas, Rect::new(10, 10, 100, 12), 12.0, theme::ACCENT);
        let stray = (0..40)
            .flat_map(|y| (150..200).map(move |x| (x, y)))
            .filter(|(x, y)| canvas.pixels()[(*y as usize) * 200 + *x as usize] == theme::ACCENT)
            .count();
        assert_eq!(stray, 0, "the bar must stay inside its rect");
    }

    #[test]
    fn an_empty_list_renders_a_placeholder_instead_of_panicking() {
        let mut canvas = Canvas::new(400, 300);
        let mut fonts = fonts();
        let area = canvas.bounds();
        assert_eq!(
            list(&mut canvas, &mut fonts, area, &[], Some(0), 0, "Empty"),
            0
        );
    }

    #[test]
    fn list_scrolls_so_the_selected_row_is_drawn() {
        let mut canvas = Canvas::new(600, 260);
        let mut fonts = fonts();
        let rows: Vec<Row> = (0..12).map(|i| Row::new(format!("Row {i}"))).collect();
        let area = Rect::new(0, 0, 600, 260);
        let offset = list(&mut canvas, &mut fonts, area, &rows, Some(11), 0, "Empty");
        assert!(offset > 0, "selecting the last row must scroll");
        assert!(offset <= 11);
    }

    #[test]
    fn paragraph_reports_the_full_line_count_even_when_clipped() {
        let mut canvas = Canvas::new(300, 60);
        let mut fonts = fonts();
        let body = "one two three four five six seven eight nine ten eleven twelve";
        let total = paragraph(
            &mut canvas,
            &mut fonts,
            Rect::new(0, 0, 300, 60),
            body,
            20.0,
            theme::TEXT,
            0,
        );
        assert!(total > 2, "the text should need more lines than fit");
    }
}
