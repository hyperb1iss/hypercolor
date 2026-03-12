use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// A stateless widget that renders an RGB pixel buffer as half-block characters.
///
/// Each terminal cell represents two vertically stacked pixels using the upper
/// half-block character (▀ U+2580). The foreground color encodes the top pixel
/// and the background color encodes the bottom pixel, effectively doubling the
/// vertical resolution of the terminal output.
pub struct HalfBlockCanvas<'a> {
    pixels: &'a [u8],
    src_width: u16,
    src_height: u16,
}

impl<'a> HalfBlockCanvas<'a> {
    /// Create a new canvas from raw RGB pixel data.
    ///
    /// `pixels` must contain `src_width * src_height * 3` bytes (RGB triplets,
    /// row-major order). If the slice is shorter than expected, out-of-bounds
    /// reads will return black.
    #[must_use]
    pub fn new(pixels: &'a [u8], src_width: u16, src_height: u16) -> Self {
        Self {
            pixels,
            src_width,
            src_height,
        }
    }
}

/// Read the RGB value of the pixel at `(x, y)` from a row-major RGB buffer.
///
/// Returns `(0, 0, 0)` (black) if the coordinates are out of range or the
/// underlying byte index falls outside the slice.
fn pixel_at(pixels: &[u8], width: u16, x: u16, y: u16) -> (u8, u8, u8) {
    if width == 0 {
        return (0, 0, 0);
    }
    let idx = (usize::from(y) * usize::from(width) + usize::from(x)) * 3;
    if idx + 2 < pixels.len() {
        (pixels[idx], pixels[idx + 1], pixels[idx + 2])
    } else {
        (0, 0, 0)
    }
}

impl Widget for HalfBlockCanvas<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.src_width == 0 || self.src_height == 0 {
            return;
        }

        // The virtual pixel grid is area.height * 2 rows tall (two pixels per
        // terminal row thanks to the half-block trick).
        let dst_h2 = u32::from(area.height) * 2;

        for row in 0..area.height {
            for col in 0..area.width {
                // Map destination cell to source coordinates.
                let sx =
                    (u32::from(col) * u32::from(self.src_width) / u32::from(area.width)) as u16;
                let sy_top = (u32::from(row) * 2 * u32::from(self.src_height) / dst_h2) as u16;
                let sy_bot =
                    ((u32::from(row) * 2 + 1) * u32::from(self.src_height) / dst_h2) as u16;

                // Clamp source coordinates to valid range.
                let sx = sx.min(self.src_width.saturating_sub(1));
                let sy_top = sy_top.min(self.src_height.saturating_sub(1));
                let sy_bot = sy_bot.min(self.src_height.saturating_sub(1));

                let (tr, tg, tb) = pixel_at(self.pixels, self.src_width, sx, sy_top);
                let (br, bg_val, bb) = pixel_at(self.pixels, self.src_width, sx, sy_bot);

                // Upper half-block: fg = top pixel, bg = bottom pixel.
                let style = Style::default()
                    .fg(Color::Rgb(tr, tg, tb))
                    .bg(Color::Rgb(br, bg_val, bb));

                let cell = &mut buf[(area.x + col, area.y + row)];
                cell.set_char('\u{2580}'); // ▀
                cell.set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_at_returns_black_for_empty_buffer() {
        assert_eq!(pixel_at(&[], 0, 0, 0), (0, 0, 0));
        assert_eq!(pixel_at(&[], 10, 5, 5), (0, 0, 0));
    }

    #[test]
    fn pixel_at_reads_correct_values() {
        // 2x1 image: pixel (0,0) = red, pixel (1,0) = green
        let pixels = [255, 0, 0, 0, 255, 0];
        assert_eq!(pixel_at(&pixels, 2, 0, 0), (255, 0, 0));
        assert_eq!(pixel_at(&pixels, 2, 1, 0), (0, 255, 0));
    }

    #[test]
    fn pixel_at_out_of_bounds_returns_black() {
        let pixels = [255, 128, 64];
        assert_eq!(pixel_at(&pixels, 1, 0, 0), (255, 128, 64));
        assert_eq!(pixel_at(&pixels, 1, 1, 0), (0, 0, 0));
        assert_eq!(pixel_at(&pixels, 1, 0, 1), (0, 0, 0));
    }

    #[test]
    fn render_on_zero_area_does_not_panic() {
        let pixels = [255, 0, 0];
        let canvas = HalfBlockCanvas::new(&pixels, 1, 1);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        canvas.render(area, &mut buf);
    }

    #[test]
    fn render_produces_half_block_chars() {
        // 1x2 source image: top pixel red, bottom pixel blue.
        let pixels = [255, 0, 0, 0, 0, 255];
        let canvas = HalfBlockCanvas::new(&pixels, 1, 2);
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        canvas.render(area, &mut buf);

        let cell = &buf[(0, 0)];
        assert_eq!(cell.symbol(), "\u{2580}");
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
        assert_eq!(cell.bg, Color::Rgb(0, 0, 255));
    }
}
