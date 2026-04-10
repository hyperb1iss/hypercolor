//! Audio pulse renderer — audio-reactive color modulation.
//!
//! Blends between a base and peak color driven by RMS audio level,
//! and flashes bright white on detected beats. The bread-and-butter
//! audio-reactive effect.

use std::path::PathBuf;

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{builtin_effect_id, color_control, preset, preset_with_desc, slider_control};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

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

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
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
        Ok(())
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

fn controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "base_color",
            "Base Color",
            [0.0, 0.1, 0.3, 1.0],
            "Colors",
            "Color shown during silence or very quiet audio.",
        ),
        color_control(
            "peak_color",
            "Peak Color",
            [1.0, 0.2, 0.5, 1.0],
            "Colors",
            "Color reached at peak RMS intensity.",
        ),
        slider_control(
            "sensitivity",
            "Sensitivity",
            2.0,
            0.1,
            4.0,
            0.01,
            "Audio",
            "Higher values react harder to quieter input.",
        ),
        slider_control(
            "beat_decay",
            "Beat Decay",
            0.85,
            0.5,
            0.99,
            0.01,
            "Audio",
            "How long the beat flash lingers after a detected beat.",
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

fn presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Cyberpunk",
            "Hot pink on dark blue",
            &[
                ("base_color", ControlValue::Color([0.0, 0.02, 0.12, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.1, 0.6, 1.0])),
                ("sensitivity", ControlValue::Float(2.5)),
                ("beat_decay", ControlValue::Float(0.88)),
            ],
        ),
        preset(
            "Fire Response",
            &[
                ("base_color", ControlValue::Color([0.08, 0.02, 0.0, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.4, 0.0, 1.0])),
                ("sensitivity", ControlValue::Float(3.0)),
                ("beat_decay", ControlValue::Float(0.82)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("audio_pulse"),
        name: "Audio Pulse".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Audio-reactive effect driven by RMS level and beat detection".into(),
        category: EffectCategory::Audio,
        tags: vec![
            "audio".into(),
            "reactive".into(),
            "beat".into(),
            "pulse".into(),
        ],
        controls: controls(),
        presets: presets(),
        audio_reactive: true,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/audio_pulse"),
        },
        license: Some("Apache-2.0".into()),
    }
}
