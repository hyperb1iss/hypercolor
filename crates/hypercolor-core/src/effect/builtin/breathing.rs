//! Breathing renderer — sinusoidal brightness pulsation.
//!
//! Produces a calming "breathing" effect by modulating brightness
//! with a smooth sine curve. Configurable speed (BPM), color, and
//! brightness range.

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

/// Pulsing brightness effect with sinusoidal modulation.
pub struct BreathingRenderer {
    /// Base color in linear RGBA.
    color: [f32; 4],
    /// Breathing speed in beats per minute.
    speed_bpm: f32,
    /// Minimum brightness at the trough.
    min_brightness: f32,
    /// Maximum brightness at the peak.
    max_brightness: f32,
}

impl BreathingRenderer {
    /// Create a breathing renderer with warm defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            color: [1.0, 0.6, 0.2, 1.0],
            speed_bpm: 15.0,
            min_brightness: 0.1,
            max_brightness: 1.0,
        }
    }
}

impl Default for BreathingRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for BreathingRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        // Convert BPM to Hz, then to angular frequency
        let freq_hz = self.speed_bpm / 60.0;
        let phase = input.time_secs * freq_hz * std::f32::consts::TAU;

        // Sine wave mapped from [-1, 1] to [min_brightness, max_brightness]
        let sine_01 = (phase.sin() + 1.0) * 0.5;
        let brightness =
            self.min_brightness + (self.max_brightness - self.min_brightness) * sine_01;

        let pixel = RgbaF32::new(
            self.color[0] * brightness,
            self.color[1] * brightness,
            self.color[2] * brightness,
            self.color[3],
        )
        .to_srgba();

        canvas.fill(pixel);
        Ok(())
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
                    self.speed_bpm = v.max(0.1);
                }
            }
            "min_brightness" => {
                if let Some(v) = value.as_f32() {
                    self.min_brightness = v.clamp(0.0, 1.0);
                }
            }
            "max_brightness" => {
                if let Some(v) = value.as_f32() {
                    self.max_brightness = v.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}
