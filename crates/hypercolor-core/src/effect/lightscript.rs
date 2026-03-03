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
        let level_db = normalized_level_to_db(audio.rms_level);
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
            audio.bass(),
            audio.mid(),
            audio.treble(),
            audio.spectral_flux,
            audio.bpm,
            if audio.beat_detected { "true" } else { "false" },
            beat_pulse,
            audio.beat_confidence,
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
    let callback = format!("on{name}Changed");

    format!(
        concat!(
            "(function(){{\n",
            "  window[{}] = {};\n",
            "  if (typeof window.{} === 'function') {{ window.{}(); }}\n",
            "}})();",
        ),
        key_literal,
        value.to_js_literal(),
        callback,
        callback,
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
        .map(f32::to_string)
        .collect::<Vec<String>>()
        .join(",")
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
        assert!(script.contains("window.onfrontColorChanged"));
        assert!(script.contains("\"#00ffcc\""));
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
}
