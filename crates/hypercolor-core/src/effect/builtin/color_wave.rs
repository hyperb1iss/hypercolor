//! Color wave renderer — traveling sinusoidal wave of color.
//!
//! Produces a smooth wave pattern that sweeps across the canvas,
//! modulating brightness with a configurable number of waves and direction.

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Direction the wave travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaveDirection {
    Left,
    Right,
}

impl WaveDirection {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "left" => Self::Left,
            _ => Self::Right,
        }
    }

    /// Returns +1.0 for right, -1.0 for left.
    const fn sign(self) -> f32 {
        match self {
            Self::Right => 1.0,
            Self::Left => -1.0,
        }
    }
}

/// Traveling wave of color across the canvas.
pub struct ColorWaveRenderer {
    /// Wave color in linear RGBA.
    color: [f32; 4],
    /// Animation speed in cycles per second.
    speed: f32,
    /// Number of complete wave cycles visible across the canvas.
    wave_count: i32,
    /// Direction the wave travels.
    direction: WaveDirection,
}

impl ColorWaveRenderer {
    /// Create a color wave renderer with neon cyan defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color: [0.5, 1.0, 0.92, 1.0],
            speed: 1.0,
            wave_count: 3,
            direction: WaveDirection::Right,
        }
    }
}

impl Default for ColorWaveRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for ColorWaveRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let w = input.canvas_width as f32;

        let time_phase =
            input.time_secs * self.speed * self.direction.sign() * std::f32::consts::TAU;

        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                let pos_phase =
                    (x as f32 / w.max(1.0)) * std::f32::consts::TAU * self.wave_count as f32;

                // Sine wave mapped to [0, 1]
                let intensity = ((pos_phase + time_phase).sin() + 1.0) * 0.5;

                let pixel = RgbaF32::new(
                    self.color[0] * intensity,
                    self.color[1] * intensity,
                    self.color[2] * intensity,
                    self.color[3],
                )
                .to_rgba();

                canvas.set_pixel(x, y, pixel);
            }
        }

        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "color" => {
                if let ControlValue::Color(c) = value {
                    self.color = *c;
                }
            }
            "speed" => {
                if let Some(v) = value.as_f32() {
                    self.speed = v;
                }
            }
            "wave_count" => {
                if let ControlValue::Integer(v) = value {
                    self.wave_count = (*v).max(1);
                }
            }
            "direction" => {
                if let ControlValue::Enum(s) = value {
                    self.direction = WaveDirection::from_str(s);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}
