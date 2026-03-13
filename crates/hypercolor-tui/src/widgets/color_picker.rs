//! HSL color picker popup widget with rainbow hue bar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);

/// HSL color picker rendered as a popup overlay.
pub struct ColorPickerPopup<'a> {
    name: &'a str,
    hsl: [f32; 3],
    selected: usize,
}

impl<'a> ColorPickerPopup<'a> {
    #[must_use]
    pub fn new(name: &'a str, hsl: [f32; 3], selected: usize) -> Self {
        Self {
            name,
            hsl,
            selected,
        }
    }
}

impl Widget for ColorPickerPopup<'_> {
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::cast_lossless,
        clippy::too_many_lines,
        clippy::many_single_char_names
    )]
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let block = Block::default()
            .title(format!(" \u{25C8} {} ", self.name))
            .title_style(
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(NEON_CYAN));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 20 || inner.height < 7 {
            return;
        }

        let (r, g, b) = hsl_to_rgb(self.hsl[0], self.hsl[1], self.hsl[2]);
        let r8 = f32_to_u8(r);
        let g8 = f32_to_u8(g);
        let b8 = f32_to_u8(b);
        let swatch = Color::Rgb(r8, g8, b8);

        // Row 0: large color swatch + hex code
        let swatch_w = 10.min(inner.width.saturating_sub(14));
        for x in 0..swatch_w {
            buf.set_string(
                inner.x + 1 + x,
                inner.y,
                "\u{2588}",
                Style::default().fg(swatch),
            );
        }
        buf.set_string(
            inner.x + 2 + swatch_w,
            inner.y,
            format!("#{r8:02x}{g8:02x}{b8:02x}"),
            Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
        );

        // Rows 2-4: H, S, L channel sliders with contextual gradients
        let labels = ["Hue", "Sat", "Lit"];
        let norms = [self.hsl[0] / 360.0, self.hsl[1], self.hsl[2]];
        let value_strs = [
            format!("{:.0}\u{00B0}", self.hsl[0]),
            format!("{:.0}%", self.hsl[1] * 100.0),
            format!("{:.0}%", self.hsl[2] * 100.0),
        ];

        for (i, ((label, &norm), value)) in labels
            .iter()
            .zip(norms.iter())
            .zip(value_strs.iter())
            .enumerate()
        {
            let y = inner.y + 2 + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let is_sel = i == self.selected;
            let ptr = if is_sel { "\u{25B8} " } else { "  " };
            let label_style = if is_sel {
                Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(BASE_WHITE)
            };

            buf.set_string(inner.x + 1, y, format!("{ptr}{label:<4}"), label_style);

            // Gradient bar with position cursor
            let bar_x = inner.x + 8;
            let val_w = value.len() as u16 + 1;
            let bar_end = (inner.x + inner.width).saturating_sub(val_w + 1);
            let bar_w = bar_end.saturating_sub(bar_x);

            if bar_w >= 4 {
                let cursor = (norm * (bar_w.saturating_sub(1)) as f32).round() as u16;

                for x_off in 0..bar_w {
                    let t = x_off as f32 / (bar_w.saturating_sub(1)).max(1) as f32;

                    let color = match i {
                        0 => {
                            let (hr, hg, hb) = hsl_to_rgb(t * 360.0, 1.0, 0.5);
                            Color::Rgb(f32_to_u8(hr), f32_to_u8(hg), f32_to_u8(hb))
                        }
                        1 => {
                            let (sr, sg, sb) = hsl_to_rgb(self.hsl[0], t, 0.5_f32.max(self.hsl[2]));
                            Color::Rgb(f32_to_u8(sr), f32_to_u8(sg), f32_to_u8(sb))
                        }
                        _ => {
                            let (lr, lg, lb) = hsl_to_rgb(self.hsl[0], self.hsl[1], t);
                            Color::Rgb(f32_to_u8(lr), f32_to_u8(lg), f32_to_u8(lb))
                        }
                    };

                    let (ch, style) = if x_off == cursor {
                        ("\u{2503}", Style::default().fg(Color::White))
                    } else {
                        ("\u{2593}", Style::default().fg(color))
                    };

                    buf.set_string(bar_x + x_off, y, ch, style);
                }
            }

            buf.set_string(
                (inner.x + inner.width).saturating_sub(val_w),
                y,
                value,
                Style::default().fg(ELECTRIC_YELLOW),
            );
        }

        // Hint row at bottom
        let hint_y = inner.y + inner.height.saturating_sub(1);
        if hint_y > inner.y + 5 {
            buf.set_string(
                inner.x + 2,
                hint_y,
                "Enter confirm \u{00B7} Esc cancel \u{00B7} h/l adjust",
                Style::default().fg(DIM_GRAY),
            );
        }
    }
}

/// Convert a 0.0..1.0 float to a 0..255 byte.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn f32_to_u8(v: f32) -> u8 {
    v.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8
}

/// Convert HSL to RGB. H: 0–360, S: 0–1, L: 0–1. Returns (R, G, B) each 0–1.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s.abs() < f32::EPSILON {
        return (l, l, l);
    }

    let h = (h.rem_euclid(360.0)) / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    (
        hue_channel(p, q, h + 1.0 / 3.0),
        hue_channel(p, q, h),
        hue_channel(p, q, h - 1.0 / 3.0),
    )
}

fn hue_channel(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

/// Convert RGB to HSL. R, G, B each 0–1. Returns (H: 0–360, S: 0–1, L: 0–1).
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);

    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };

    (h * 360.0, s, l)
}
