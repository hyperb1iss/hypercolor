//! Audio pulse renderer — radial beat rings over an RMS-reactive floor.
//!
//! Draws an ambient color field that fades between `base_color` and `peak_color`
//! with the RMS audio level, and spawns a radial ring wave from the canvas
//! center on every detected beat. Rings expand outward in normalized
//! coordinates so the effect stays resolution-independent across LED layouts.
//!
//! This replaces an older single-color flash renderer whose output was identical
//! for every pixel — see the deep backend audit (M10) for the frame-rate-
//! dependent beat decay bug that the time-based envelope here also fixes.

use std::path::PathBuf;

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{builtin_effect_id, color_control, preset_with_desc, slider_control};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

/// Hard cap on concurrent rings. At 120 BPM and the default wave speed a ring
/// lives ~1.7 s, so four or five are typical; the cap guards against runaway
/// beat detection.
const MAX_WAVES: usize = 12;

/// A single radial ring expanding outward from the canvas center.
#[derive(Clone, Copy, Debug)]
struct RadialWave {
    /// Normalized radius. 0.0 sits at the center, 1.0 at the farthest corner.
    radius: f32,
}

/// Radial audio pulse with RMS ambient and beat-triggered rings.
pub struct AudioPulseRenderer {
    base_color: [f32; 4],
    peak_color: [f32; 4],
    sensitivity: f32,
    wave_speed: f32,
    wave_width: f32,
    beat_decay_secs: f32,
    brightness: f32,
    waves: Vec<RadialWave>,
    beat_flash: f32,
}

impl AudioPulseRenderer {
    /// Create a renderer with vivid `SilkCircuit` defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_color: [0.02, 0.02, 0.12, 1.0],
            peak_color: [1.0, 0.15, 0.55, 1.0],
            sensitivity: 2.0,
            wave_speed: 1.2,
            wave_width: 0.18,
            beat_decay_secs: 0.35,
            brightness: 1.0,
            waves: Vec::new(),
            beat_flash: 0.0,
        }
    }

    fn spawn_wave(&mut self) {
        if self.waves.len() >= MAX_WAVES {
            self.waves.remove(0);
        }
        self.waves.push(RadialWave { radius: 0.0 });
    }

    /// Exponential time-based decay. `beat_decay_secs` is the ~95% decay time
    /// (three time constants), so `tau = beat_decay_secs / 3`.
    fn decay_beat_flash(&mut self, delta_secs: f32) {
        if self.beat_decay_secs <= 1e-4 {
            self.beat_flash = 0.0;
            return;
        }
        let tau = self.beat_decay_secs / 3.0;
        self.beat_flash *= (-delta_secs / tau).exp();
        if self.beat_flash < 1e-3 {
            self.beat_flash = 0.0;
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
        self.waves.clear();
        self.beat_flash = 0.0;
        Ok(())
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "canvas dimensions fit comfortably within f32 precision"
    )]
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);

        let delta = input.delta_secs.max(0.0);

        if input.audio.beat_detected {
            self.beat_flash = 1.0;
            self.spawn_wave();
        } else {
            self.decay_beat_flash(delta);
        }

        for wave in &mut self.waves {
            wave.radius += self.wave_speed * delta;
        }
        self.waves.retain(|w| w.radius < 2.0);

        let width = input.canvas_width;
        let height = input.canvas_height;
        if width == 0 || height == 0 {
            return Ok(());
        }

        let cx = (width as f32) * 0.5;
        let cy = (height as f32) * 0.5;
        let half_diag = (cx * cx + cy * cy).sqrt().max(1.0);

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
        // Rings wash toward a boosted peak so they read as a highlight rather
        // than a plain tint of the ambient color.
        let accent = RgbaF32::new(
            (self.peak_color[0] * 1.25 + 0.1).clamp(0.0, 1.0),
            (self.peak_color[1] * 1.25 + 0.1).clamp(0.0, 1.0),
            (self.peak_color[2] * 1.25 + 0.1).clamp(0.0, 1.0),
            1.0,
        );

        let rms_t = (input.audio.rms_level * self.sensitivity).clamp(0.0, 1.0);
        let ambient = RgbaF32::lerp(&base, &peak, rms_t);

        let beat_boost = self.beat_flash * 0.22;
        let half_width = self.wave_width.max(1e-4);

        let row_stride = (width as usize) * BYTES_PER_PIXEL;
        let bytes = canvas.as_rgba_bytes_mut();

        for y in 0..height {
            let row_offset = (y as usize) * row_stride;
            let dy = (y as f32) + 0.5 - cy;
            for x in 0..width {
                let dx = (x as f32) + 0.5 - cx;
                let dist_norm = (dx * dx + dy * dy).sqrt() / half_diag;

                let mut wave_accum = 0.0_f32;
                for wave in &self.waves {
                    let age_fade = (1.0 - wave.radius * 0.5).clamp(0.0, 1.0);
                    if age_fade <= 0.0 {
                        continue;
                    }
                    let ring_dist = (dist_norm - wave.radius).abs();
                    if ring_dist < half_width {
                        let falloff = 1.0 - (ring_dist / half_width);
                        wave_accum += age_fade * falloff * falloff;
                    }
                }
                let wave_t = wave_accum.clamp(0.0, 1.0);

                let mut r = ambient.r + (accent.r - ambient.r) * wave_t + beat_boost;
                let mut g = ambient.g + (accent.g - ambient.g) * wave_t + beat_boost;
                let mut b = ambient.b + (accent.b - ambient.b) * wave_t + beat_boost;

                r = (r * self.brightness).clamp(0.0, 1.0);
                g = (g * self.brightness).clamp(0.0, 1.0);
                b = (b * self.brightness).clamp(0.0, 1.0);

                let rgba = RgbaF32::new(r, g, b, 1.0).to_srgba();
                let pixel_offset = row_offset + (x as usize) * BYTES_PER_PIXEL;
                bytes[pixel_offset] = rgba.r;
                bytes[pixel_offset + 1] = rgba.g;
                bytes[pixel_offset + 2] = rgba.b;
                bytes[pixel_offset + 3] = 255;
            }
        }

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
            "wave_speed" => {
                if let Some(v) = value.as_f32() {
                    self.wave_speed = v.clamp(0.0, 8.0);
                }
            }
            "wave_width" => {
                if let Some(v) = value.as_f32() {
                    self.wave_width = v.clamp(0.01, 1.0);
                }
            }
            "beat_decay" => {
                if let Some(v) = value.as_f32() {
                    self.beat_decay_secs = v.clamp(0.02, 3.0);
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
        self.waves.clear();
        self.beat_flash = 0.0;
    }
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "base_color",
            "Base Color",
            [0.02, 0.02, 0.12, 1.0],
            "Colors",
            "Ambient color shown during silence or very quiet audio.",
        ),
        color_control(
            "peak_color",
            "Peak Color",
            [1.0, 0.15, 0.55, 1.0],
            "Colors",
            "Color reached at peak RMS intensity and used for the ring highlight.",
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
            0.35,
            0.05,
            1.5,
            0.01,
            "Audio",
            "How many seconds the beat flash lingers after a detected beat.",
        ),
        slider_control(
            "wave_speed",
            "Wave Speed",
            1.2,
            0.2,
            4.0,
            0.01,
            "Motion",
            "How fast each beat ring expands outward from the canvas center.",
        ),
        slider_control(
            "wave_width",
            "Wave Width",
            0.18,
            0.02,
            0.5,
            0.01,
            "Motion",
            "Thickness of each beat ring as a fraction of the canvas radius.",
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
            "Hot pink rings on electric midnight",
            &[
                ("base_color", ControlValue::Color([0.0, 0.02, 0.12, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.1, 0.6, 1.0])),
                ("sensitivity", ControlValue::Float(2.5)),
                ("beat_decay", ControlValue::Float(0.35)),
                ("wave_speed", ControlValue::Float(1.4)),
                ("wave_width", ControlValue::Float(0.16)),
            ],
        ),
        preset_with_desc(
            "Fire Response",
            "Ember rings rolling across dark maroon",
            &[
                ("base_color", ControlValue::Color([0.08, 0.02, 0.0, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.4, 0.0, 1.0])),
                ("sensitivity", ControlValue::Float(3.0)),
                ("beat_decay", ControlValue::Float(0.25)),
                ("wave_speed", ControlValue::Float(1.8)),
                ("wave_width", ControlValue::Float(0.2)),
            ],
        ),
        preset_with_desc(
            "Arctic Beat",
            "Cold cyan rings on deep indigo ice",
            &[
                ("base_color", ControlValue::Color([0.01, 0.02, 0.1, 1.0])),
                ("peak_color", ControlValue::Color([0.35, 0.9, 1.0, 1.0])),
                ("sensitivity", ControlValue::Float(1.8)),
                ("beat_decay", ControlValue::Float(0.5)),
                ("wave_speed", ControlValue::Float(1.0)),
                ("wave_width", ControlValue::Float(0.22)),
            ],
        ),
        preset_with_desc(
            "Bass Thunder",
            "Slow, thick crimson rings for heavy drops",
            &[
                ("base_color", ControlValue::Color([0.02, 0.0, 0.0, 1.0])),
                ("peak_color", ControlValue::Color([0.95, 0.08, 0.12, 1.0])),
                ("sensitivity", ControlValue::Float(2.8)),
                ("beat_decay", ControlValue::Float(0.7)),
                ("wave_speed", ControlValue::Float(0.7)),
                ("wave_width", ControlValue::Float(0.35)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("audio_pulse"),
        name: "Audio Pulse".into(),
        author: "Hypercolor".into(),
        version: "0.2.0".into(),
        description: "Radial beat rings over an RMS-reactive ambient floor".into(),
        category: EffectCategory::Audio,
        tags: vec![
            "audio".into(),
            "reactive".into(),
            "beat".into(),
            "pulse".into(),
            "radial".into(),
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
