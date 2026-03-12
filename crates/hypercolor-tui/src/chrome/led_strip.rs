//! LED preview strip — live half-block canvas rendering.
//!
//! When a `CanvasFrame` is available, the strip downsamples the canvas to the
//! terminal width and renders it using the lower-half-block character (U+2584).
//! Each terminal cell encodes two vertical pixels: the **background** color is
//! the top pixel and the **foreground** color is the bottom pixel.
//!
//! When no canvas data is present, a dim gradient placeholder is shown.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use crate::state::{AppState, CanvasFrame};
use crate::theme;

/// The lower-half-block character used for two-pixel-per-cell rendering.
const HALF_BLOCK: char = '\u{2584}';

/// Stateless LED strip renderer.
pub struct LedStrip;

impl LedStrip {
    /// Render the LED preview strip into the given 2-row area.
    pub fn render(&self, frame: &mut Frame, area: Rect, state: &AppState) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let widget = LedStripWidget {
            canvas: state.canvas_frame.as_ref(),
        };
        frame.render_widget(widget, area);
    }
}

/// Widget that renders the LED preview into a buffer.
struct LedStripWidget<'a> {
    canvas: Option<&'a CanvasFrame>,
}

impl Widget for LedStripWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match self.canvas {
            Some(canvas) if !canvas.pixels.is_empty() && canvas.width > 0 => {
                render_canvas(canvas, area, buf);
            }
            _ => {
                render_placeholder(area, buf);
            }
        }
    }
}

/// Render actual canvas data into the buffer using half-block characters.
///
/// The canvas is downsampled to fit `area.width` columns and `area.height * 2`
/// pixel rows (since each terminal row encodes two vertical pixels).
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn render_canvas(canvas: &CanvasFrame, area: Rect, buf: &mut Buffer) {
    let term_cols = area.width as usize;
    let term_rows = area.height as usize;
    let pixel_rows = term_rows * 2;

    let src_w = canvas.width as usize;
    let src_h = canvas.height as usize;

    for row in 0..term_rows {
        let top_pixel_row = row * 2;
        let bot_pixel_row = row * 2 + 1;

        for col in 0..term_cols {
            let top_color = sample_pixel(
                canvas,
                src_w,
                src_h,
                col,
                top_pixel_row,
                term_cols,
                pixel_rows,
            );
            let bot_color = sample_pixel(
                canvas,
                src_w,
                src_h,
                col,
                bot_pixel_row,
                term_cols,
                pixel_rows,
            );

            let cell = &mut buf[(area.x + col as u16, area.y + row as u16)];
            cell.set_char(HALF_BLOCK)
                .set_style(Style::default().fg(bot_color).bg(top_color));
        }
    }
}

/// Sample a single pixel from the canvas using nearest-neighbor downsampling.
fn sample_pixel(
    canvas: &CanvasFrame,
    src_w: usize,
    src_h: usize,
    col: usize,
    row: usize,
    dest_w: usize,
    dest_h: usize,
) -> Color {
    if dest_w == 0 || dest_h == 0 {
        return Color::Black;
    }

    let sx = col * src_w / dest_w;
    let sy = row * src_h / dest_h;
    let sx = sx.min(src_w.saturating_sub(1));
    let sy = sy.min(src_h.saturating_sub(1));

    let idx = (sy * src_w + sx) * 3;
    if idx + 2 < canvas.pixels.len() {
        Color::Rgb(
            canvas.pixels[idx],
            canvas.pixels[idx + 1],
            canvas.pixels[idx + 2],
        )
    } else {
        Color::Black
    }
}

/// Render a dim gradient placeholder when no canvas data is available.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_lossless
)]
fn render_placeholder(area: Rect, buf: &mut Buffer) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }

    let bg = theme::bg_panel();

    for row in 0..area.height {
        for col in 0..area.width {
            // Create a subtle hue sweep across the width.
            let t = col as f32 / width.max(1) as f32;
            let dim_color = dim_gradient_at(t);

            let cell = &mut buf[(area.x + col, area.y + row)];
            cell.set_char(HALF_BLOCK)
                .set_style(Style::default().fg(dim_color).bg(bg));
        }
    }
}

/// Generate a dim gradient color at position `t` (0.0..1.0).
///
/// Sweeps through a muted version of the `SilkCircuit` palette:
/// purple -> cyan -> coral, all at ~20% brightness.
fn dim_gradient_at(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);

    // Three-stop gradient: purple(0.0) -> cyan(0.5) -> coral(1.0), dimmed
    let (r, g, b) = if t < 0.5 {
        let s = t * 2.0;
        lerp_rgb((56, 13, 64), (32, 64, 58), s) // dim purple -> dim cyan
    } else {
        let s = (t - 0.5) * 2.0;
        lerp_rgb((32, 64, 58), (64, 26, 48), s) // dim cyan -> dim coral
    };

    Color::Rgb(r, g, b)
}

/// Linear interpolation between two RGB triples.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let lerp = |a: u8, b: u8, t: f32| -> u8 {
        let v = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            clippy::as_conversions
        )]
        {
            v.clamp(0.0, 255.0) as u8
        }
    };

    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}
