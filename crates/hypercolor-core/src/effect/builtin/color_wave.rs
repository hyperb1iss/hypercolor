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
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
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
    /// Master output brightness.
    brightness: f32,
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
            brightness: 1.0,
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
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;

        let time_phase =
            input.time_secs * self.speed * self.direction.sign() * std::f32::consts::TAU;

        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                let pos_phase = (x as f32 / width) * std::f32::consts::TAU * self.wave_count as f32;

                // Sine wave mapped to [0, 1]
                let intensity = ((pos_phase + time_phase).sin() + 1.0) * 0.5;

                let pixel = RgbaF32::new(
                    self.color[0] * intensity * self.brightness,
                    self.color[1] * intensity * self.brightness,
                    self.color[2] * intensity * self.brightness,
                    self.color[3],
                )
                .to_srgba();

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
                if let Some(v) = value.as_f32() {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        clippy::as_conversions
                    )]
                    {
                        self.wave_count = v.round().max(1.0) as i32;
                    }
                }
            }
            "direction" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.direction = WaveDirection::from_str(choice);
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
