//! Color Zones — multi-zone static color grid.
//!
//! Assigns independent colors to up to 9 spatial zones, arranged as rows,
//! columns, or a 2D grid. Blend softness creates smooth Oklab transitions
//! between adjacent zones.

use std::path::PathBuf;

use hypercolor_types::canvas::{Canvas, Oklab, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{
    builtin_effect_id, color_control, dropdown_control, preset_with_desc, slider_control,
};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

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
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
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

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        if name == "zone_count" {
            if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value
                && let Ok(n) = choice.parse::<u8>()
            {
                self.zone_count = n.clamp(2, 9);
            }
            return;
        }

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

#[allow(
    clippy::too_many_lines,
    reason = "zone control list is intentionally authored inline for readability"
)]
fn controls() -> Vec<ControlDefinition> {
    vec![
        dropdown_control(
            "zone_count",
            "Zone Count",
            "3",
            &["2", "3", "4", "5", "6", "7", "8", "9"],
            "Layout",
            "Number of active color zones.",
        ),
        dropdown_control(
            "layout",
            "Layout",
            "Columns",
            &["Columns", "Rows", "Grid"],
            "Layout",
            "Arrange zones as vertical columns, horizontal rows, or a 2D grid.",
        ),
        slider_control(
            "blend",
            "Blend Softness",
            0.15,
            0.0,
            1.0,
            0.01,
            "Layout",
            "Smoothness of transitions between adjacent zones. 0 = hard edges.",
        ),
        color_control(
            "zone_1",
            "Zone 1",
            [0.88, 0.08, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 1.",
        ),
        color_control(
            "zone_2",
            "Zone 2",
            [0.0, 1.0, 0.85, 1.0],
            "Zone Colors",
            "Color for zone 2.",
        ),
        color_control(
            "zone_3",
            "Zone 3",
            [1.0, 0.25, 0.55, 1.0],
            "Zone Colors",
            "Color for zone 3.",
        ),
        color_control(
            "zone_4",
            "Zone 4",
            [0.31, 0.98, 0.48, 1.0],
            "Zone Colors",
            "Color for zone 4.",
        ),
        color_control(
            "zone_5",
            "Zone 5",
            [0.95, 0.98, 0.55, 1.0],
            "Zone Colors",
            "Color for zone 5.",
        ),
        color_control(
            "zone_6",
            "Zone 6",
            [1.0, 0.39, 0.39, 1.0],
            "Zone Colors",
            "Color for zone 6.",
        ),
        color_control(
            "zone_7",
            "Zone 7",
            [0.0, 0.4, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 7.",
        ),
        color_control(
            "zone_8",
            "Zone 8",
            [1.0, 0.6, 0.0, 1.0],
            "Zone Colors",
            "Color for zone 8.",
        ),
        color_control(
            "zone_9",
            "Zone 9",
            [0.6, 0.0, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 9.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

#[allow(
    clippy::too_many_lines,
    reason = "preset catalogs are maintained as one readable static table"
)]
fn presets() -> Vec<PresetTemplate> {
    vec![
        // ── Signature ────────────────────────────────────────────────────
        preset_with_desc(
            "SilkCircuit",
            "Electric purple, neon cyan, and coral",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.88, 0.08, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 1.0, 0.85, 1.0])),
                ("zone_3", ControlValue::Color([1.0, 0.25, 0.55, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Fire & Ice",
            "Warm and cool contrast across the system",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.15, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.6, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.3, 1.0, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "RGB Diagnostic",
            "Pure red, green, blue columns",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 1.0, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.0, 1.0, 1.0])),
                ("blend", ControlValue::Float(0.0)),
            ],
        ),
        preset_with_desc(
            "Ocean Layers",
            "Horizontal depth bands from surface to deep",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([0.4, 0.85, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.1, 0.5, 0.9, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.2, 0.6, 1.0])),
                ("zone_4", ControlValue::Color([0.0, 0.05, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Neon Matrix",
            "9-zone grid with vibrant neon palette",
            &[
                ("zone_count", ControlValue::Enum("9".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        // ── Nature & Atmosphere ──────────────────────────────────────────
        preset_with_desc(
            "Sunset Boulevard",
            "Golden hour fading into deep twilight",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.85, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.4, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.1, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([0.3, 0.0, 0.4, 1.0])),
                ("blend", ControlValue::Float(0.2)),
            ],
        ),
        preset_with_desc(
            "Arctic Aurora",
            "Northern lights dancing across the sky",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.9, 0.3, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.8, 0.7, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.3, 0.9, 1.0])),
                ("zone_4", ControlValue::Color([0.4, 0.0, 0.8, 1.0])),
                ("zone_5", ControlValue::Color([0.8, 0.1, 0.5, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Cherry Blossom",
            "Spring pinks from deep rose to soft bloom",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.4, 0.55, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.7, 0.75, 1.0])),
                ("zone_3", ControlValue::Color([0.85, 0.15, 0.4, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Tropical Reef",
            "Coral, turquoise, deep blue, and sandy gold",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.35, 0.25, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.85, 0.7, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.2, 0.7, 1.0])),
                ("zone_4", ControlValue::Color([0.95, 0.75, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Lava Flow",
            "Molten orange cooling into deep obsidian",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.5, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([0.8, 0.1, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.3, 0.02, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Deep Space",
            "Dark cosmic nebula in a 2x3 grid",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.02, 0.15, 1.0])),
                ("zone_2", ControlValue::Color([0.15, 0.0, 0.3, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.1, 0.2, 1.0])),
                ("zone_4", ControlValue::Color([0.2, 0.0, 0.15, 1.0])),
                ("zone_5", ControlValue::Color([0.0, 0.05, 0.25, 1.0])),
                ("zone_6", ControlValue::Color([0.1, 0.0, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.2)),
            ],
        ),
        preset_with_desc(
            "Emerald City",
            "Dark forest, bright emerald, and gold",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.4, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([0.1, 0.9, 0.3, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.75, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        // ── Neon & Cyber ─────────────────────────────────────────────────
        preset_with_desc(
            "Vaporwave",
            "Hot pink, purple, and teal retrowave",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.2, 0.6, 1.0])),
                ("zone_2", ControlValue::Color([0.5, 0.1, 0.9, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.9, 0.8, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Cyberpunk Alley",
            "Neon pink, blue, purple, and toxic green grid",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.5, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.4, 1.0, 1.0])),
                ("zone_3", ControlValue::Color([0.6, 0.0, 1.0, 1.0])),
                ("zone_4", ControlValue::Color([0.2, 1.0, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.08)),
            ],
        ),
        preset_with_desc(
            "Hacker Terminal",
            "Matrix green and void black",
            &[
                ("zone_count", ControlValue::Enum("2".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.9, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.15, 0.02, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Midnight Jazz",
            "Deep navy, purple, gold accent, warm ivory",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.02, 0.02, 0.2, 1.0])),
                ("zone_2", ControlValue::Color([0.35, 0.0, 0.6, 1.0])),
                ("zone_3", ControlValue::Color([0.85, 0.7, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([1.0, 0.9, 0.7, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Stealth",
            "Barely-there dim blue and purple",
            &[
                ("zone_count", ControlValue::Enum("2".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.02, 0.12, 1.0])),
                ("zone_2", ControlValue::Color([0.05, 0.0, 0.1, 1.0])),
                ("blend", ControlValue::Float(0.2)),
                ("brightness", ControlValue::Float(0.5)),
            ],
        ),
        // ── Pastel & Soft ────────────────────────────────────────────────
        preset_with_desc(
            "Candy Pastel",
            "Bright candy colors in a 2x3 grid",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.5, 0.6, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.95, 0.4, 1.0])),
                ("zone_3", ControlValue::Color([0.4, 0.65, 1.0, 1.0])),
                ("zone_4", ControlValue::Color([0.4, 1.0, 0.6, 1.0])),
                ("zone_5", ControlValue::Color([0.7, 0.45, 1.0, 1.0])),
                ("zone_6", ControlValue::Color([1.0, 0.65, 0.35, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Lavender Dream",
            "Soft purples and rose in layered rows",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([0.7, 0.5, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.5, 0.15, 0.85, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.3, 0.6, 1.0])),
                ("zone_4", ControlValue::Color([0.75, 0.45, 0.95, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        // ── Pride Flags ──────────────────────────────────────────────────
        preset_with_desc(
            "Trans Pride",
            "Light blue, pink, white, pink, blue stripes",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.357, 0.808, 0.98, 1.0])), // #5BCEFA
                ("zone_2", ControlValue::Color([0.961, 0.663, 0.722, 1.0])), // #F5A9B8
                ("zone_3", ControlValue::Color([1.0, 1.0, 1.0, 1.0])),      // #FFFFFF
                ("zone_4", ControlValue::Color([0.961, 0.663, 0.722, 1.0])), // #F5A9B8
                ("zone_5", ControlValue::Color([0.357, 0.808, 0.98, 1.0])), // #5BCEFA
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Bi Pride",
            "Magenta, purple, and blue bands",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.839, 0.008, 0.439, 1.0])), // #D60270
                ("zone_2", ControlValue::Color([0.608, 0.31, 0.588, 1.0])),  // #9B4F96
                ("zone_3", ControlValue::Color([0.0, 0.22, 0.659, 1.0])),    // #0038A8
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Lesbian Pride",
            "Orange, white, and pink sunset stripes",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.839, 0.161, 0.0, 1.0])), // #D62900
                ("zone_2", ControlValue::Color([1.0, 0.608, 0.333, 1.0])), // #FF9B55
                ("zone_3", ControlValue::Color([1.0, 1.0, 1.0, 1.0])),     // #FFFFFF
                ("zone_4", ControlValue::Color([0.831, 0.38, 0.651, 1.0])), // #D461A6
                ("zone_5", ControlValue::Color([0.647, 0.0, 0.384, 1.0])), // #A50062
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Non-Binary Pride",
            "Yellow, white, purple, and black stripes",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.988, 0.957, 0.204, 1.0])), // #FCF434
                ("zone_2", ControlValue::Color([1.0, 1.0, 1.0, 1.0])),       // #FFFFFF
                ("zone_3", ControlValue::Color([0.612, 0.349, 0.82, 1.0])),  // #9C59D1
                ("zone_4", ControlValue::Color([0.0, 0.0, 0.0, 1.0])),       // #000000
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Rainbow Pride",
            "Classic six-stripe rainbow flag",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.894, 0.012, 0.012, 1.0])), // #E40303
                ("zone_2", ControlValue::Color([1.0, 0.549, 0.0, 1.0])),     // #FF8C00
                ("zone_3", ControlValue::Color([1.0, 0.929, 0.0, 1.0])),     // #FFED00
                ("zone_4", ControlValue::Color([0.0, 0.502, 0.149, 1.0])),   // #008026
                ("zone_5", ControlValue::Color([0.0, 0.302, 1.0, 1.0])),     // #004DFF
                ("zone_6", ControlValue::Color([0.459, 0.027, 0.529, 1.0])), // #750787
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        // ── Bold Blocks ──────────────────────────────────────────────────
        preset_with_desc(
            "Rainbow Blocks",
            "Full spectrum 3x3 grid with hard edges",
            &[
                ("zone_count", ControlValue::Enum("9".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.5, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([1.0, 1.0, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([0.0, 1.0, 0.0, 1.0])),
                ("zone_5", ControlValue::Color([0.0, 1.0, 1.0, 1.0])),
                ("zone_6", ControlValue::Color([0.0, 0.0, 1.0, 1.0])),
                ("zone_7", ControlValue::Color([0.3, 0.0, 0.5, 1.0])),
                ("zone_8", ControlValue::Color([0.6, 0.0, 1.0, 1.0])),
                ("zone_9", ControlValue::Color([1.0, 0.0, 0.6, 1.0])),
                ("blend", ControlValue::Float(0.0)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("color_zones"),
        name: "Color Zones".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description:
            "Multi-zone color grid with per-zone colors, flexible layouts, and smooth blending"
                .into(),
        category: EffectCategory::Ambient,
        tags: vec![
            "zones".into(),
            "grid".into(),
            "static".into(),
            "scene".into(),
        ],
        controls: controls(),
        presets: presets(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/color_zones"),
        },
        license: Some("Apache-2.0".into()),
    }
}
