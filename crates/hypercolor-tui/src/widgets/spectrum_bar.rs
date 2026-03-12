use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// The eight block-element characters used to quantize bar height, from lowest
/// (▁ U+2581) to tallest (█ U+2588).
const BAR_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Renders an audio spectrum as vertical bars using Unicode block characters.
///
/// Each column maps to one frequency bin. Bar height is quantized to 8 levels.
/// Colours transition linearly from `bass_color` on the left through
/// `mid_color` in the centre to `treble_color` on the right.
pub struct SpectrumBar<'a> {
    bins: &'a [f32],
    bass_color: Color,
    mid_color: Color,
    treble_color: Color,
}

impl<'a> SpectrumBar<'a> {
    #[must_use]
    pub fn new(bins: &'a [f32]) -> Self {
        Self {
            bins,
            bass_color: Color::Rgb(0, 180, 255),
            mid_color: Color::Rgb(0, 255, 128),
            treble_color: Color::Rgb(255, 80, 200),
        }
    }

    #[must_use]
    pub fn bass_color(mut self, color: Color) -> Self {
        self.bass_color = color;
        self
    }

    #[must_use]
    pub fn mid_color(mut self, color: Color) -> Self {
        self.mid_color = color;
        self
    }

    #[must_use]
    pub fn treble_color(mut self, color: Color) -> Self {
        self.treble_color = color;
        self
    }
}

/// Linearly interpolate between two `Color::Rgb` values. If either colour is
/// not an `Rgb` variant the function falls back to `a`.
#[allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg_val, bb)) = (a, b) {
        let r = f32::from(ar) + (f32::from(br) - f32::from(ar)) * t;
        let g = f32::from(ag) + (f32::from(bg_val) - f32::from(ag)) * t;
        let b_ch = f32::from(ab) + (f32::from(bb) - f32::from(ab)) * t;
        Color::Rgb(r as u8, g as u8, b_ch as u8)
    } else {
        a
    }
}

/// Compute the spectrum colour for a column at fractional position `t` in
/// `[0, 1]`, blending bass -> mid -> treble.
fn spectrum_color(bass: Color, mid: Color, treble: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        lerp_color(bass, mid, t * 2.0)
    } else {
        lerp_color(mid, treble, (t - 0.5) * 2.0)
    }
}

impl Widget for SpectrumBar<'_> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.bins.is_empty() {
            return;
        }

        let col_count = area.width;

        for col in 0..col_count {
            // Map column to the corresponding bin (nearest-neighbour).
            let bin_idx = (usize::from(col) * self.bins.len() / usize::from(col_count))
                .min(self.bins.len() - 1);
            let value = self.bins[bin_idx].clamp(0.0, 1.0);

            // Fractional position across the bar for colour interpolation.
            let t = if col_count > 1 {
                f32::from(col) / f32::from(col_count - 1)
            } else {
                0.5
            };
            let color = spectrum_color(self.bass_color, self.mid_color, self.treble_color, t);

            // Quantize the normalised value to one of the 8 block characters.
            // A value of exactly 0 produces the lowest bar (▁); 1.0 the tallest (█).
            let level = ((value * 7.0).round() as usize).min(7);
            let ch = BAR_CHARS[level];

            // Render the bar character in the bottom row of the allocated area.
            let x = area.x + col;
            let y = area.y + area.height - 1;
            let style = Style::default().fg(color);

            let cell = &mut buf[(x, y)];
            cell.set_char(ch);
            cell.set_style(style);

            // Fill remaining rows above with spaces (clear any stale content).
            for row in 0..area.height.saturating_sub(1) {
                let cell = &mut buf[(x, area.y + row)];
                cell.set_char(' ');
                cell.set_style(Style::default());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_color_endpoints() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(255, 255, 255);
        assert_eq!(lerp_color(a, b, 0.0), Color::Rgb(0, 0, 0));
        assert_eq!(lerp_color(a, b, 1.0), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn lerp_color_midpoint() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(200, 100, 50);
        let mid = lerp_color(a, b, 0.5);
        assert_eq!(mid, Color::Rgb(100, 50, 25));
    }

    #[test]
    fn spectrum_color_transitions() {
        let bass = Color::Rgb(255, 0, 0);
        let mid = Color::Rgb(0, 255, 0);
        let treble = Color::Rgb(0, 0, 255);

        assert_eq!(spectrum_color(bass, mid, treble, 0.0), bass);
        assert_eq!(spectrum_color(bass, mid, treble, 0.5), mid);
        assert_eq!(spectrum_color(bass, mid, treble, 1.0), treble);
    }

    #[test]
    fn render_empty_bins_does_not_panic() {
        let bar = SpectrumBar::new(&[]);
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
    }

    #[test]
    fn render_zero_area_does_not_panic() {
        let bins = [0.5, 0.8];
        let bar = SpectrumBar::new(&bins);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
    }

    #[test]
    fn render_single_bin_produces_bar_char() {
        let bins = [1.0];
        let bar = SpectrumBar::new(&bins);
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        let cell = &buf[(0, 0)];
        assert_eq!(cell.symbol(), "\u{2588}"); // █
    }

    #[test]
    fn render_zero_value_produces_lowest_bar() {
        let bins = [0.0];
        let bar = SpectrumBar::new(&bins);
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        let cell = &buf[(0, 0)];
        assert_eq!(cell.symbol(), "\u{2581}"); // ▁
    }
}
