//! `SignalRGB` Lightscript runtime shim helpers.
//!
//! This module builds JavaScript snippets for bootstrapping and per-frame
//! runtime injection without binding directly to any specific web engine.

use std::collections::HashMap;

use hypercolor_types::audio::AudioData;
use hypercolor_types::effect::ControlValue;

const LEVEL_FLOOR_DB: f32 = -100.0;

/// Batch of JavaScript snippets to evaluate for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightscriptFrameScripts {
    /// JavaScript that updates `window.engine.audio`.
    pub audio_update: String,

    /// JavaScript snippets for changed control values.
    pub control_updates: Vec<String>,
}

/// Runtime state for Lightscript injection.
#[derive(Debug, Clone)]
pub struct LightscriptRuntime {
    width: u32,
    height: u32,
    last_controls: HashMap<String, ControlValue>,
}

impl LightscriptRuntime {
    /// Create a runtime shim for a fixed effect canvas size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            last_controls: HashMap::new(),
        }
    }

    /// Build JavaScript that initializes the `window.engine` object.
    #[must_use]
    pub fn bootstrap_script(&self) -> String {
        format!(
            concat!(
                "(function(){{\n",
                "  if (typeof window.engine !== 'object' || window.engine === null) {{ window.engine = {{}}; }}\n",
                "  window.engine.width = {};\n",
                "  window.engine.height = {};\n",
                "  if (typeof window.engine.audio !== 'object' || window.engine.audio === null) {{ window.engine.audio = {{}}; }}\n",
                "  window.engine.audio.level = {};\n",
                "  window.engine.audio.bass = 0;\n",
                "  window.engine.audio.mid = 0;\n",
                "  window.engine.audio.treble = 0;\n",
                "  window.engine.audio.density = 0;\n",
                "  window.engine.audio.bpm = 0;\n",
                "  window.engine.audio.beat = false;\n",
                "  window.engine.audio.beatPulse = 0;\n",
                "  window.engine.audio.freq = new Float32Array(200);\n",
                "}})();",
            ),
            self.width, self.height, LEVEL_FLOOR_DB,
        )
    }

    /// Build JavaScript to update `window.engine` dimensions when canvas size
    /// changes.
    ///
    /// Returns `None` when dimensions are unchanged.
    #[must_use]
    pub fn resize_script(&mut self, width: u32, height: u32) -> Option<String> {
        if self.width == width && self.height == height {
            return None;
        }

        self.width = width;
        self.height = height;

        Some(format!(
            concat!(
                "(function(){{\n",
                "  if (typeof window.engine !== 'object' || window.engine === null) {{ window.engine = {{}}; }}\n",
                "  window.engine.width = {};\n",
                "  window.engine.height = {};\n",
                "}})();",
            ),
            width, height
        ))
    }

    /// Build JavaScript for one frame's audio + changed control updates.
    #[must_use]
    pub fn frame_scripts(
        &mut self,
        audio: &AudioData,
        controls: &HashMap<String, ControlValue>,
    ) -> LightscriptFrameScripts {
        let audio_update = Self::audio_update_script(audio);
        let control_updates = self.control_update_scripts(controls);

        LightscriptFrameScripts {
            audio_update,
            control_updates,
        }
    }

    fn audio_update_script(audio: &AudioData) -> String {
        let level_db = js_number(normalized_level_to_db(audio.rms_level));
        let beat_pulse = if audio.beat_detected {
            1.0_f32
        } else {
            0.0_f32
        };
        let spectrum_values = join_f32_csv(&audio.spectrum);

        format!(
            concat!(
                "(function(){{\n",
                "  if (typeof window.engine !== 'object' || window.engine === null) {{ window.engine = {{}}; }}\n",
                "  if (typeof window.engine.audio !== 'object' || window.engine.audio === null) {{ window.engine.audio = {{}}; }}\n",
                "  window.engine.audio.level = {};\n",
                "  window.engine.audio.bass = {};\n",
                "  window.engine.audio.mid = {};\n",
                "  window.engine.audio.treble = {};\n",
                "  window.engine.audio.density = {};\n",
                "  window.engine.audio.bpm = {};\n",
                "  window.engine.audio.beat = {};\n",
                "  window.engine.audio.beatPulse = {};\n",
                "  window.engine.audio.confidence = {};\n",
                "  window.engine.audio.freq = new Float32Array([{}]);\n",
                "}})();",
            ),
            level_db,
            js_number(audio.bass()),
            js_number(audio.mid()),
            js_number(audio.treble()),
            js_number(audio.spectral_flux),
            js_number(audio.bpm),
            if audio.beat_detected { "true" } else { "false" },
            js_number(beat_pulse),
            js_number(audio.beat_confidence),
            spectrum_values,
        )
    }

    fn control_update_scripts(&mut self, controls: &HashMap<String, ControlValue>) -> Vec<String> {
        let mut scripts = Vec::new();

        for (name, value) in controls {
            let changed = self
                .last_controls
                .get(name)
                .is_none_or(|previous| previous != value);

            if !changed {
                continue;
            }

            scripts.push(control_update_script(name, value));
            self.last_controls.insert(name.clone(), value.clone());
        }

        scripts
    }
}

/// Convert a control update into JavaScript assignment + change hook call.
#[must_use]
pub fn control_update_script(name: &str, value: &ControlValue) -> String {
    let key_literal = serde_json::to_string(name).unwrap_or_else(|_| "\"invalid\"".to_owned());
    let callback_literal = serde_json::to_string(&format!("on{name}Changed"))
        .unwrap_or_else(|_| "\"oninvalidChanged\"".to_owned());

    format!(
        concat!(
            "(function(){{\n",
            "  const callback = {};\n",
            "  window[{}] = {};\n",
            "  if (typeof window[callback] === 'function') {{ window[callback](); }}\n",
            "}})();",
        ),
        callback_literal,
        key_literal,
        value.to_js_literal(),
    )
}

/// Convert normalized 0..1 level to dB scale used by many `SignalRGB` effects.
#[must_use]
pub fn normalized_level_to_db(level: f32) -> f32 {
    if !level.is_finite() || level <= 0.0 {
        return LEVEL_FLOOR_DB;
    }

    let db = 20.0 * level.log10();
    db.clamp(LEVEL_FLOOR_DB, 0.0)
}

fn join_f32_csv(values: &[f32]) -> String {
    values
        .iter()
        .copied()
        .map(js_number)
        .collect::<Vec<String>>()
        .join(",")
}

fn js_number(value: f32) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "0".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_script_contains_runtime_shape() {
        let runtime = LightscriptRuntime::new(320, 200);
        let script = runtime.bootstrap_script();

        assert!(script.contains("window.engine.width = 320"));
        assert!(script.contains("window.engine.height = 200"));
        assert!(script.contains("window.engine.audio.freq = new Float32Array(200)"));
    }

    #[test]
    fn normalized_level_to_db_clamps_edges() {
        assert!((normalized_level_to_db(1.0) - 0.0).abs() < f32::EPSILON);
        assert!((normalized_level_to_db(0.0) - LEVEL_FLOOR_DB).abs() < f32::EPSILON);
        assert!((normalized_level_to_db(-1.0) - LEVEL_FLOOR_DB).abs() < f32::EPSILON);
    }

    #[test]
    fn control_update_script_escapes_key_and_invokes_callback() {
        let script = control_update_script("frontColor", &ControlValue::Text("#00ffcc".to_owned()));

        assert!(script.contains("window[\"frontColor\"]"));
        assert!(script.contains("const callback = \"onfrontColorChanged\""));
        assert!(script.contains("window[callback]"));
        assert!(script.contains("\"#00ffcc\""));
    }

    #[test]
    fn control_update_script_supports_non_identifier_keys() {
        let script = control_update_script("my-control", &ControlValue::Float(1.0));
        assert!(script.contains("window[\"my-control\"] = 1"));
        assert!(script.contains("const callback = \"onmy-controlChanged\""));
    }

    #[test]
    fn frame_scripts_emit_control_deltas_only() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let audio = AudioData::silence();

        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(0.5));

        let first = runtime.frame_scripts(&audio, &controls);
        assert_eq!(first.control_updates.len(), 1);

        let second = runtime.frame_scripts(&audio, &controls);
        assert!(second.control_updates.is_empty());

        controls.insert("speed".to_owned(), ControlValue::Float(0.8));
        let third = runtime.frame_scripts(&audio, &controls);
        assert_eq!(third.control_updates.len(), 1);
    }

    #[test]
    fn audio_script_contains_level_and_freq_payload() {
        let mut audio = AudioData::silence();
        audio.rms_level = 1.0;
        audio.spectrum = vec![0.1, 0.2, 0.3];

        let script = LightscriptRuntime::audio_update_script(&audio);
        assert!(script.contains("window.engine.audio.level = 0"));
        assert!(script.contains("window.engine.audio.freq = new Float32Array([0.1,0.2,0.3])"));
    }

    #[test]
    fn audio_script_sanitizes_non_finite_values() {
        let mut audio = AudioData::silence();
        audio.rms_level = f32::NAN;
        audio.spectrum = vec![f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
        audio.bpm = f32::INFINITY;
        audio.spectral_flux = f32::NEG_INFINITY;

        let script = LightscriptRuntime::audio_update_script(&audio);
        assert!(!script.contains("inf"));
        assert!(!script.contains("NaN"));
        assert!(script.contains("window.engine.audio.freq = new Float32Array([0,0,0])"));
    }

    #[test]
    fn resize_script_emits_only_on_dimension_change() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        assert!(runtime.resize_script(320, 200).is_none());

        let resize = runtime
            .resize_script(640, 360)
            .expect("resize should emit when dimensions change");
        assert!(resize.contains("window.engine.width = 640"));
        assert!(resize.contains("window.engine.height = 360"));
        assert!(runtime.resize_script(640, 360).is_none());
    }
}
