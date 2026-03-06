//! Rainbow renderer — cycling rainbow pattern across the canvas.
//!
//! Sweeps through the hue spectrum in Oklch perceptual space, producing
//! vivid, evenly-spaced rainbow bands that animate over time.

use hypercolor_types::canvas::{Canvas, Oklch, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Cycling rainbow pattern using perceptual Oklch hue rotation.
pub struct RainbowRenderer {
    /// Animation speed in hue-degrees per second.
    speed: f32,
    /// Wavelength scale — lower values produce more bands.
    scale: f32,
    /// Output brightness (Oklch lightness).
    brightness: f32,
}

impl RainbowRenderer {
    /// Create a rainbow renderer with pleasant defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            speed: 60.0,
            scale: 1.0,
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
    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let w = input.canvas_width as f32;
        let time_offset = input.time_secs * self.speed;

        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                // Position-based hue offset, scaled by wavelength
                let pos_hue = (x as f32 / w.max(1.0)) * 360.0 * self.scale;
                let hue = ((pos_hue + time_offset) % 360.0 + 360.0) % 360.0;

                let lch = Oklch::new(self.brightness, 0.15, hue, 1.0);
                let rgba = RgbaF32::from_oklch(lch).to_srgba();
                canvas.set_pixel(x, y, rgba);
            }
        }

        Ok(canvas)
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
