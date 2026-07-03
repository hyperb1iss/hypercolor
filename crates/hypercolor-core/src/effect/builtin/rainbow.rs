//! Rainbow renderer — cycling rainbow pattern across the canvas.
//!
//! Sweeps through the hue spectrum in HSV space, producing vivid,
//! fully-saturated rainbow bands that animate over time.

use std::path::PathBuf;

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource,
};

use super::common::{builtin_effect_id, dropdown_control, slider_control};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

/// Axis along which the hue gradient sweeps.
///
/// Vertical LED strips see a single color under a horizontal sweep, so the
/// axis is exposed as a control instead of being hardcoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RainbowDirection {
    /// Hue varies along x; every row is identical (fast row-copy path).
    Horizontal,
    /// Hue varies along y; every column is identical (fast row-fill path).
    Vertical,
    /// Hue varies along the x+y diagonal (per-pixel path).
    Diagonal,
}

impl RainbowDirection {
    fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "vertical" => Self::Vertical,
            "diagonal" => Self::Diagonal,
            _ => Self::Horizontal,
        }
    }
}

/// Cycling rainbow pattern using HSV hue rotation.
pub struct RainbowRenderer {
    /// Animation speed in hue-degrees per second.
    speed: f32,
    /// Wavelength scale — lower values produce more bands.
    scale: f32,
    /// Output saturation.
    saturation: f32,
    /// Output brightness (HSV value).
    brightness: f32,
    /// Sweep axis for the hue gradient.
    direction: RainbowDirection,
}

impl RainbowRenderer {
    /// Create a rainbow renderer with pleasant defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            speed: 60.0,
            scale: 1.0,
            saturation: 1.0,
            brightness: 0.75,
            direction: RainbowDirection::Horizontal,
        }
    }
}

impl Default for RainbowRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for RainbowRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let time_offset = input.time_secs * self.speed;
        let row_len = input.canvas_width as usize * BYTES_PER_PIXEL;

        if row_len == 0 {
            return Ok(());
        }

        let pixels = canvas.as_rgba_bytes_mut();
        match self.direction {
            RainbowDirection::Horizontal => {
                // Every row is identical: compute the first row, then copy.
                let (first_row, remaining_rows) = pixels.split_at_mut(row_len);
                for (x, pixel) in first_row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
                    let pos_hue = (x as f32 / width) * 360.0 * self.scale;
                    let hue = (pos_hue + time_offset).rem_euclid(360.0);
                    let (r, g, b) = hsv_to_rgb(hue, self.saturation, self.brightness);
                    pixel[0] = r;
                    pixel[1] = g;
                    pixel[2] = b;
                    pixel[3] = u8::MAX;
                }

                for row in remaining_rows.chunks_exact_mut(row_len) {
                    row.copy_from_slice(first_row);
                }
            }
            RainbowDirection::Vertical => {
                // Every column is identical: one hue per row, filled across.
                for (y, row) in pixels.chunks_exact_mut(row_len).enumerate() {
                    let pos_hue = (y as f32 / height) * 360.0 * self.scale;
                    let hue = (pos_hue + time_offset).rem_euclid(360.0);
                    let (r, g, b) = hsv_to_rgb(hue, self.saturation, self.brightness);
                    for pixel in row.chunks_exact_mut(BYTES_PER_PIXEL) {
                        pixel[0] = r;
                        pixel[1] = g;
                        pixel[2] = b;
                        pixel[3] = u8::MAX;
                    }
                }
            }
            RainbowDirection::Diagonal => {
                // Hue varies with (nx + ny) / 2 — the per-pixel cost is
                // accepted only in this mode.
                for (y, row) in pixels.chunks_exact_mut(row_len).enumerate() {
                    let ny = y as f32 / height;
                    for (x, pixel) in row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
                        let nx = x as f32 / width;
                        let pos_hue = (nx + ny) * 0.5 * 360.0 * self.scale;
                        let hue = (pos_hue + time_offset).rem_euclid(360.0);
                        let (r, g, b) = hsv_to_rgb(hue, self.saturation, self.brightness);
                        pixel[0] = r;
                        pixel[1] = g;
                        pixel[2] = b;
                        pixel[3] = u8::MAX;
                    }
                }
            }
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "speed" => {
                if let Some(v) = value.as_f32() {
                    self.speed = v;
                }
            }
            "scale" => {
                if let Some(v) = value.as_f32() {
                    self.scale = v.max(0.01);
                }
            }
            "saturation" => {
                if let Some(v) = value.as_f32() {
                    self.saturation = v.clamp(0.0, 1.0);
                }
            }
            "brightness" => {
                if let Some(v) = value.as_f32() {
                    self.brightness = v.clamp(0.0, 1.0);
                }
            }
            "direction" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.direction = RainbowDirection::from_str(choice);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

/// Simple HSV to RGB conversion. H in [0, 360), S and V in [0, 1].
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = v - c;

    #[allow(clippy::cast_precision_loss)]
    let (r1, g1, b1) = if h_prime < 1.0 {
        (c, x, 0.0)
    } else if h_prime < 2.0 {
        (x, c, 0.0)
    } else if h_prime < 3.0 {
        (0.0, c, x)
    } else if h_prime < 4.0 {
        (0.0, x, c)
    } else if h_prime < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        slider_control(
            "speed",
            "Speed",
            60.0,
            -180.0,
            180.0,
            1.0,
            "Motion",
            "Hue rotation speed in degrees per second.",
        ),
        slider_control(
            "scale",
            "Band Density",
            1.0,
            0.1,
            4.0,
            0.01,
            "Shape",
            "Lower values create broad rainbow bands; higher values add more stripes.",
        ),
        dropdown_control(
            "direction",
            "Direction",
            "Horizontal",
            &["Horizontal", "Vertical", "Diagonal"],
            "Shape",
            "Sweep axis for the hue gradient. Use Vertical for vertically mounted strips.",
        ),
        slider_control(
            "saturation",
            "Saturation",
            1.0,
            0.0,
            1.0,
            0.01,
            "Colors",
            "Color intensity. Lower values soften the rainbow; 1.0 gives fully saturated hues.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            0.75,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("rainbow"),
        name: "Rainbow".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Vivid full-spectrum rainbow cycle with animated hue bands".into(),
        category: EffectCategory::Ambient,
        tags: vec!["rainbow".into(), "hue".into(), "colorful".into()],
        controls: controls(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/rainbow"),
        },
        license: Some("Apache-2.0".into()),
    }
}
