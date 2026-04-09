//! Rainbow renderer — cycling rainbow pattern across the canvas.
//!
//! Sweeps through the hue spectrum in HSV space, producing vivid,
//! fully-saturated rainbow bands that animate over time.

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

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
        let time_offset = input.time_secs * self.speed;
        let row_len = input.canvas_width as usize * BYTES_PER_PIXEL;

        if row_len == 0 {
            return Ok(());
        }

        let pixels = canvas.as_rgba_bytes_mut();
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
