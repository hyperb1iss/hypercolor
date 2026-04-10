//! Breathing renderer — sinusoidal brightness pulsation.
//!
//! Produces a calming "breathing" effect by modulating brightness
//! with a smooth sine curve. Configurable speed (BPM), color, and
//! brightness range.

use std::path::PathBuf;

use hypercolor_types::canvas::{Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{builtin_effect_id, color_control, preset, preset_with_desc, slider_control};
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

fn controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "color",
            "Color",
            [1.0, 0.6, 0.2, 1.0],
            "Colors",
            "Base color that breathes in and out.",
        ),
        slider_control(
            "speed",
            "Speed",
            15.0,
            1.0,
            120.0,
            1.0,
            "Motion",
            "Breathing rate in beats per minute.",
        ),
        slider_control(
            "min_brightness",
            "Minimum Brightness",
            0.1,
            0.0,
            1.0,
            0.01,
            "Output",
            "Brightness at the trough of the cycle.",
        ),
        slider_control(
            "max_brightness",
            "Maximum Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Brightness at the peak of the cycle.",
        ),
    ]
}

fn presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Warm Ember",
            "Slow amber glow like dying embers",
            &[
                ("color", ControlValue::Color([1.0, 0.4, 0.1, 1.0])),
                ("speed", ControlValue::Float(8.0)),
                ("min_brightness", ControlValue::Float(0.05)),
                ("max_brightness", ControlValue::Float(0.8)),
            ],
        ),
        preset_with_desc(
            "Ocean Calm",
            "Deep blue with slow tidal rhythm",
            &[
                ("color", ControlValue::Color([0.1, 0.3, 1.0, 1.0])),
                ("speed", ControlValue::Float(6.0)),
                ("min_brightness", ControlValue::Float(0.08)),
                ("max_brightness", ControlValue::Float(0.7)),
            ],
        ),
        preset(
            "Alert Pulse",
            &[
                ("color", ControlValue::Color([1.0, 0.1, 0.1, 1.0])),
                ("speed", ControlValue::Float(40.0)),
                ("min_brightness", ControlValue::Float(0.2)),
                ("max_brightness", ControlValue::Float(1.0)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("breathing"),
        name: "Breathing".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Smooth sinusoidal brightness pulsation".into(),
        category: EffectCategory::Ambient,
        tags: vec!["breathing".into(), "pulse".into(), "calm".into()],
        controls: controls(),
        presets: presets(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/breathing"),
        },
        license: Some("Apache-2.0".into()),
    }
}
