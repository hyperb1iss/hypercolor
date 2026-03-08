//! Audio pulse renderer — audio-reactive color modulation.
//!
//! Blends between a base and peak color driven by RMS audio level,
//! and flashes bright white on detected beats. The bread-and-butter
//! audio-reactive effect.

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{ControlValue, EffectMetadata};

use crate::effect::traits::{EffectRenderer, FrameInput};

/// Audio-reactive effect that pulses color intensity with sound.
pub struct AudioPulseRenderer {
    /// Color shown during silence.
    base_color: [f32; 4],
    /// Color shown at peak audio level.
    peak_color: [f32; 4],
    /// Sensitivity multiplier for RMS level (higher = more responsive).
    sensitivity: f32,
    /// Exponential decay factor for the beat flash (0.0-1.0 per frame).
    beat_decay: f32,
    /// Current beat flash intensity (decays over frames).
    beat_flash: f32,
    /// Master output brightness.
    brightness: f32,
}

impl AudioPulseRenderer {
    /// Create an audio pulse renderer with vivid defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_color: [0.0, 0.1, 0.3, 1.0],
            peak_color: [1.0, 0.2, 0.5, 1.0],
            sensitivity: 2.0,
            beat_decay: 0.85,
            beat_flash: 0.0,
            brightness: 1.0,
        }
    }
}

impl Default for AudioPulseRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for AudioPulseRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        self.beat_flash = 0.0;
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);

        // RMS-driven blend factor
        let rms_t = (input.audio.rms_level * self.sensitivity).clamp(0.0, 1.0);

        // Beat flash: spike on beat detect, then exponential decay
        if input.audio.beat_detected {
            self.beat_flash = 1.0;
        } else {
            self.beat_flash *= self.beat_decay;
        }

        let base = RgbaF32::new(
            self.base_color[0],
            self.base_color[1],
            self.base_color[2],
            self.base_color[3],
        );
        let peak = RgbaF32::new(
            self.peak_color[0],
            self.peak_color[1],
            self.peak_color[2],
            self.peak_color[3],
        );
        let white = RgbaF32::new(1.0, 1.0, 1.0, 1.0);

        // Blend base → peak by RMS, then mix in beat flash
        let rms_color = RgbaF32::lerp(&base, &peak, rms_t);
        let mut final_color = RgbaF32::lerp(&rms_color, &white, self.beat_flash * 0.6);
        final_color.r *= self.brightness;
        final_color.g *= self.brightness;
        final_color.b *= self.brightness;

        canvas.fill(final_color.to_srgba());
        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "base_color" => {
                if let ControlValue::Color(c) = value {
                    self.base_color = *c;
                }
            }
            "peak_color" => {
                if let ControlValue::Color(c) = value {
                    self.peak_color = *c;
                }
            }
            "sensitivity" => {
                if let Some(v) = value.as_f32() {
                    self.sensitivity = v.max(0.01);
                }
            }
            "beat_decay" => {
                if let Some(v) = value.as_f32() {
                    self.beat_decay = v.clamp(0.0, 0.99);
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

    fn destroy(&mut self) {
        self.beat_flash = 0.0;
    }
}
