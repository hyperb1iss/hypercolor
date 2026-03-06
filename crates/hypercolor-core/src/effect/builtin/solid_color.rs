//! Solid color renderer — fills the entire canvas with a single color.
//!
//! The simplest possible effect. Essential for testing, default states,
//! and as a compositing base layer.

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Fills every pixel with a single color, modulated by brightness.
pub struct SolidColorRenderer {
    /// Current color in linear RGBA.
    color: [f32; 4],
    /// Brightness multiplier (0.0 = black, 1.0 = full color).
    brightness: f32,
}

impl SolidColorRenderer {
    /// Create a new solid color renderer with opaque white at full brightness.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            brightness: 1.0,
        }
    }
}

impl Default for SolidColorRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for SolidColorRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);

        let pixel = RgbaF32::new(
            self.color[0] * self.brightness,
            self.color[1] * self.brightness,
            self.color[2] * self.brightness,
            self.color[3],
        )
        .to_srgba();

        canvas.fill(pixel);
        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color" => {
                if let ControlValue::Color(c) = value {
                    self.color = *c;
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
