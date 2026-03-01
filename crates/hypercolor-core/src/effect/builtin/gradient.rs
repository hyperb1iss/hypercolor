//! Gradient renderer — animated horizontal, vertical, radial, or diagonal gradient.
//!
//! Interpolates between two colors in Oklab perceptual space for smooth,
//! visually pleasing gradients. Animates by shifting the gradient offset over time.

use hypercolor_types::canvas::{Canvas, Oklab, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Direction of the gradient sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GradientDirection {
    Horizontal,
    Vertical,
    Radial,
    Diagonal,
}

impl GradientDirection {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "vertical" => Self::Vertical,
            "radial" => Self::Radial,
            "diagonal" => Self::Diagonal,
            _ => Self::Horizontal,
        }
    }
}

/// Animated two-color gradient with configurable direction and speed.
pub struct GradientRenderer {
    color_start: [f32; 4],
    color_end: [f32; 4],
    direction: GradientDirection,
    /// Animation speed in cycles per second.
    speed: f32,
}

impl GradientRenderer {
    /// Create a gradient from electric purple to neon cyan.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color_start: [0.88, 0.21, 1.0, 1.0],
            color_end: [0.5, 1.0, 0.92, 1.0],
            direction: GradientDirection::Horizontal,
            speed: 0.2,
        }
    }
}

impl Default for GradientRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for GradientRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let w = input.canvas_width as f32;
        let h = input.canvas_height as f32;

        // Animated offset — wraps every cycle
        let offset = (input.time_secs * self.speed).fract();

        let lab_start = RgbaF32::new(
            self.color_start[0],
            self.color_start[1],
            self.color_start[2],
            self.color_start[3],
        )
        .to_oklab();

        let lab_end = RgbaF32::new(
            self.color_end[0],
            self.color_end[1],
            self.color_end[2],
            self.color_end[3],
        )
        .to_oklab();

        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                let raw_t = match self.direction {
                    GradientDirection::Horizontal => x as f32 / w.max(1.0),
                    GradientDirection::Vertical => y as f32 / h.max(1.0),
                    GradientDirection::Diagonal => {
                        (x as f32 / w.max(1.0) + y as f32 / h.max(1.0)) / 2.0
                    }
                    GradientDirection::Radial => {
                        let cx = x as f32 / w.max(1.0) - 0.5;
                        let cy = y as f32 / h.max(1.0) - 0.5;
                        ((cx * cx + cy * cy).sqrt() * 2.0).min(1.0)
                    }
                };

                // Apply animation offset with wrapping
                let t = ((raw_t + offset) % 1.0).abs();

                let blended = Oklab::lerp(lab_start, lab_end, t);
                let rgba = RgbaF32::from_oklab(blended).to_rgba();
                canvas.set_pixel(x, y, rgba);
            }
        }

        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color_start" => {
                if let ControlValue::Color(c) = value {
                    self.color_start = *c;
                }
            }
            "color_end" => {
                if let ControlValue::Color(c) = value {
                    self.color_end = *c;
                }
            }
            "direction" => {
                if let ControlValue::Enum(s) = value {
                    self.direction = GradientDirection::from_str(s);
                }
            }
            "speed" => {
                if let Some(v) = value.as_f32() {
                    self.speed = v;
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}
