//! Color Zones — multi-zone static color grid.
//!
//! Assigns independent colors to up to 9 spatial zones, arranged as rows,
//! columns, or a 2D grid. Blend softness creates smooth Oklab transitions
//! between adjacent zones.

use hypercolor_types::canvas::{Canvas, Oklab, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Zone arrangement on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZoneLayout {
    /// Vertical strips (zones side-by-side).
    Columns,
    /// Horizontal strips (zones stacked).
    Rows,
    /// 2D grid (auto-sized based on zone count).
    Grid,
}

impl ZoneLayout {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "rows" => Self::Rows,
            "grid" => Self::Grid,
            _ => Self::Columns,
        }
    }
}

/// Multi-zone static color renderer with per-zone color pickers and soft blending.
pub struct ColorZonesRenderer {
    zones: [[f32; 4]; 9],
    zone_count: u8,
    layout: ZoneLayout,
    blend: f32,
    brightness: f32,
}

impl ColorZonesRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            zones: [
                [0.88, 0.08, 1.0, 1.0],  // Electric purple
                [0.0, 1.0, 0.85, 1.0],   // Neon cyan
                [1.0, 0.25, 0.55, 1.0],  // Hot coral
                [0.31, 0.98, 0.48, 1.0], // Success green
                [0.95, 0.98, 0.55, 1.0], // Electric yellow
                [1.0, 0.39, 0.39, 1.0],  // Error red
                [0.0, 0.4, 1.0, 1.0],    // Deep blue
                [1.0, 0.6, 0.0, 1.0],    // Amber
                [0.6, 0.0, 1.0, 1.0],    // Deep purple
            ],
            zone_count: 3,
            layout: ZoneLayout::Columns,
            blend: 0.15,
            brightness: 1.0,
        }
    }

    /// Compute grid dimensions (rows, cols) for the active layout and count.
    fn grid_dimensions(&self) -> (u8, u8) {
        match self.layout {
            ZoneLayout::Rows => (self.zone_count, 1),
            ZoneLayout::Columns => (1, self.zone_count),
            ZoneLayout::Grid => match self.zone_count {
                2 => (1, 2),
                3 => (1, 3),
                4 => (2, 2),
                5 => (1, 5),
                6 => (2, 3),
                7 => (1, 7),
                8 => (2, 4),
                9 => (3, 3),
                _ => (1, 1),
            },
        }
    }

    /// Look up the color for a zone at (row, col), clamping to valid indices.
    #[allow(clippy::as_conversions)]
    fn zone_color(&self, row: usize, col: usize, cols: u8) -> [f32; 4] {
        let index = (row * usize::from(cols) + col).min(usize::from(self.zone_count) - 1);
        self.zones[index]
    }

    /// Sample blended zone color at normalized canvas position.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn sample_blended(&self, nx: f32, ny: f32, rows: u8, cols: u8) -> RgbaF32 {
        let gx = nx * f32::from(cols);
        let gy = ny * f32::from(rows);

        // Find the 2x2 neighborhood of zone centers (centers at 0.5, 1.5, ...).
        let max_col = usize::from(cols.saturating_sub(2));
        let max_row = usize::from(rows.saturating_sub(2));
        let base_col = ((gx - 0.5).floor().max(0.0) as usize).min(max_col);
        let base_row = ((gy - 0.5).floor().max(0.0) as usize).min(max_row);

        // Fractional position between the two nearest zone centers.
        let center_left = base_col as f32 + 0.5;
        let center_top = base_row as f32 + 0.5;
        let fx = (gx - center_left).clamp(0.0, 1.0);
        let fy = (gy - center_top).clamp(0.0, 1.0);

        // Apply blend-controlled smoothstep (blend=0 → hard edge, blend=1 → full smooth).
        let sx = smoothstep_blend(fx, self.blend);
        let sy = smoothstep_blend(fy, self.blend);

        // Corner zone colors.
        let right_col = (base_col + 1).min(usize::from(cols) - 1);
        let bottom_row = (base_row + 1).min(usize::from(rows) - 1);

        let c00 = rgba_to_oklab(self.zone_color(base_row, base_col, cols));
        let c10 = rgba_to_oklab(self.zone_color(base_row, right_col, cols));
        let c01 = rgba_to_oklab(self.zone_color(bottom_row, base_col, cols));
        let c11 = rgba_to_oklab(self.zone_color(bottom_row, right_col, cols));

        // Bilinear interpolation in Oklab.
        let top = Oklab::lerp(c00, c10, sx);
        let bot = Oklab::lerp(c01, c11, sx);
        let result = Oklab::lerp(top, bot, sy);

        RgbaF32::from_oklab(result)
    }
}

impl Default for ColorZonesRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for ColorZonesRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let (rows, cols) = self.grid_dimensions();

        for y in 0..input.canvas_height {
            let ny = (y as f32 + 0.5) / height;
            for x in 0..input.canvas_width {
                let nx = (x as f32 + 0.5) / width;

                let rgba = if self.blend <= f32::EPSILON {
                    // Hard zone boundaries.
                    let col = ((nx * f32::from(cols)).floor() as u8).min(cols - 1);
                    let row = ((ny * f32::from(rows)).floor() as u8).min(rows - 1);
                    let color = self.zone_color(usize::from(row), usize::from(col), cols);
                    RgbaF32::new(color[0], color[1], color[2], color[3])
                } else {
                    self.sample_blended(nx, ny, rows, cols)
                };

                let output = RgbaF32::new(
                    rgba.r * self.brightness,
                    rgba.g * self.brightness,
                    rgba.b * self.brightness,
                    rgba.a,
                );
                canvas.set_pixel(x, y, output.to_srgba());
            }
        }

        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        // Handle zone_1 through zone_9.
        if let Some(idx) = name.strip_prefix("zone_") {
            if let Ok(n) = idx.parse::<usize>()
                && (1..=9).contains(&n)
                && let ControlValue::Color(c) = value
            {
                self.zones[n - 1] = *c;
            }
            return;
        }

        match name {
            "zone_count" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value
                    && let Ok(n) = choice.parse::<u8>()
                {
                    self.zone_count = n.clamp(2, 9);
                }
            }
            "layout" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.layout = ZoneLayout::from_str(choice);
                }
            }
            "blend" => {
                if let Some(v) = value.as_f32() {
                    self.blend = v.clamp(0.0, 1.0);
                }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() {
                    self.brightness = v.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn rgba_to_oklab(color: [f32; 4]) -> Oklab {
    RgbaF32::new(color[0], color[1], color[2], color[3]).to_oklab()
}

/// Blend-controlled smoothstep transition.
///
/// - `blend = 0.0`: hard step at the midpoint.
/// - `blend = 1.0`: smooth transition across the full range.
fn smoothstep_blend(t: f32, blend: f32) -> f32 {
    if blend <= f32::EPSILON {
        return if t < 0.5 { 0.0 } else { 1.0 };
    }

    let half_blend = blend * 0.5;
    let lower = 0.5 - half_blend;
    let upper = 0.5 + half_blend;

    if t <= lower {
        return 0.0;
    }
    if t >= upper {
        return 1.0;
    }

    let n = (t - lower) / (upper - lower);
    n * n * (3.0 - 2.0 * n)
}

fn normalize_choice(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_owned()
}
