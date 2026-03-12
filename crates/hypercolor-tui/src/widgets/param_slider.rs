use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

/// A horizontal parameter slider widget.
///
/// Renders as: `Label    ▓▓▓▓▓▓▓░░░░  65%`
///
/// The label is left-aligned, the percentage is right-aligned, and the bar
/// fills the remaining space between them. The filled portion uses the
/// `accent_color` and the empty portion uses a dim gray.
pub struct ParamSlider<'a> {
    label: &'a str,
    value: f32,
    min_label: Option<&'a str>,
    max_label: Option<&'a str>,
    accent_color: Color,
}

/// Dim gray used for the unfilled portion of the slider track.
const DIM_GRAY: Color = Color::Rgb(68, 68, 68);

/// Minimum number of columns required for the slider bar to be rendered (not
/// counting label / percentage). Below this we just render label + percentage.
const MIN_BAR_WIDTH: u16 = 3;

impl<'a> ParamSlider<'a> {
    #[must_use]
    pub fn new(label: &'a str, value: f32) -> Self {
        Self {
            label,
            value: value.clamp(0.0, 1.0),
            min_label: None,
            max_label: None,
            accent_color: Color::Rgb(0, 200, 255),
        }
    }

    #[must_use]
    pub fn min_label(mut self, label: &'a str) -> Self {
        self.min_label = Some(label);
        self
    }

    #[must_use]
    pub fn max_label(mut self, label: &'a str) -> Self {
        self.max_label = Some(label);
        self
    }

    #[must_use]
    pub fn accent_color(mut self, color: Color) -> Self {
        self.accent_color = color;
        self
    }
}

impl Widget for ParamSlider<'_> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::as_conversions
    )]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let y = area.y;
        let total_width = area.width as usize;

        // -- Build the percentage string --
        let pct = (self.value * 100.0).round() as u8;
        let pct_str = format!("{pct:>3}%");
        let pct_len = pct_str.len(); // always 4

        // -- Render the label (left-aligned) --
        let label_style = Style::default().add_modifier(Modifier::BOLD);
        let label_chars: Vec<char> = self.label.chars().collect();
        let label_len = label_chars.len().min(total_width);
        for (i, &ch) in label_chars.iter().take(label_len).enumerate() {
            let cell = &mut buf[(area.x + i as u16, y)];
            cell.set_char(ch);
            cell.set_style(label_style);
        }

        // Spacing after label.
        let gap = 1_usize;

        // -- Determine bar region --
        // Layout: [label] [gap] [min?] [bar] [max?] [gap] [pct]
        let min_text = self.min_label.unwrap_or("");
        let max_text = self.max_label.unwrap_or("");
        let min_len = min_text.len();
        let max_len = max_text.len();

        let overhead = label_len
            + gap
            + min_len
            + usize::from(min_len > 0)
            + max_len
            + usize::from(max_len > 0)
            + gap
            + pct_len;

        let bar_width = if total_width > overhead {
            (total_width - overhead) as u16
        } else {
            0
        };

        if bar_width >= MIN_BAR_WIDTH {
            let mut x = area.x + label_len as u16 + gap as u16;

            // Optional min label.
            if !min_text.is_empty() {
                let dim_style = Style::default().fg(DIM_GRAY);
                for ch in min_text.chars() {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(ch);
                    cell.set_style(dim_style);
                    x += 1;
                }
                let cell = &mut buf[(x, y)];
                cell.set_char(' ');
                x += 1;
            }

            // Filled / empty portions of the bar.
            let filled = ((self.value * f32::from(bar_width)).round() as u16).min(bar_width);
            let empty = bar_width - filled;

            let filled_style = Style::default().fg(self.accent_color);
            for _ in 0..filled {
                let cell = &mut buf[(x, y)];
                cell.set_char('\u{2593}'); // ▓
                cell.set_style(filled_style);
                x += 1;
            }

            let empty_style = Style::default().fg(DIM_GRAY);
            for _ in 0..empty {
                let cell = &mut buf[(x, y)];
                cell.set_char('\u{2591}'); // ░
                cell.set_style(empty_style);
                x += 1;
            }

            // Optional max label.
            if !max_text.is_empty() {
                let cell = &mut buf[(x, y)];
                cell.set_char(' ');
                x += 1;
                let dim_style = Style::default().fg(DIM_GRAY);
                for ch in max_text.chars() {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(ch);
                    cell.set_style(dim_style);
                    x += 1;
                }
            }
        }

        // -- Render percentage right-aligned --
        let pct_x = area.x + area.width - pct_len as u16;
        let pct_style = Style::default().fg(Color::White);
        for (i, ch) in pct_str.chars().enumerate() {
            let cell = &mut buf[(pct_x + i as u16, y)];
            cell.set_char(ch);
            cell.set_style(pct_style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_zero_area_does_not_panic() {
        let slider = ParamSlider::new("Speed", 0.5);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        slider.render(area, &mut buf);
    }

    #[test]
    fn render_narrow_area_shows_label_and_percent() {
        // Very narrow: only label + pct should appear; bar is skipped.
        let slider = ParamSlider::new("Sp", 0.0);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        slider.render(area, &mut buf);

        // Percentage should be right-aligned ending at col 9.
        let pct_cell = &buf[(9, 0)];
        assert_eq!(pct_cell.symbol(), "%");
    }

    #[test]
    fn render_wide_area_fills_bar() {
        let slider = ParamSlider::new("Vol", 1.0);
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        slider.render(area, &mut buf);

        // At 100% the bar should be fully filled (▓ chars).
        // Just verify at least one filled char exists after the label.
        let has_filled = (0..30).any(|x| buf[(x, 0)].symbol() == "\u{2593}");
        assert!(has_filled, "expected filled bar chars at 100%");
    }

    #[test]
    fn render_zero_value_shows_empty_bar() {
        let slider = ParamSlider::new("Vol", 0.0);
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        slider.render(area, &mut buf);

        let has_empty = (0..30).any(|x| buf[(x, 0)].symbol() == "\u{2591}");
        assert!(has_empty, "expected empty bar chars at 0%");
    }

    #[test]
    fn value_is_clamped() {
        // Values outside [0,1] should be clamped, not panic.
        let slider = ParamSlider::new("X", 1.5);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        slider.render(area, &mut buf);

        let pct_cell = &buf[(19, 0)];
        assert_eq!(pct_cell.symbol(), "%");
    }
}
