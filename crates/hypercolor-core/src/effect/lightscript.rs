//! `LightScript` runtime shim helpers.
//!
//! This module builds JavaScript snippets for bootstrapping and per-frame
//! runtime injection without binding directly to any specific web engine.

use std::collections::HashMap;
use std::fmt::Write as _;

use hypercolor_types::audio::{AudioData, CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};
use hypercolor_types::effect::ControlValue;
use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
use serde_json::{Map, Value, json};

use crate::input::{InteractionData, ScreenData};

const LEVEL_FLOOR_DB: f32 = -100.0;
const DEFAULT_ZONE_WIDTH: usize = 28;
const DEFAULT_ZONE_HEIGHT: usize = 20;
const DEFAULT_ZONE_SAMPLES: usize = DEFAULT_ZONE_WIDTH * DEFAULT_ZONE_HEIGHT;
const DEFAULT_ZONE_IMAGE_WIDTH: usize = 160;
const DEFAULT_ZONE_IMAGE_HEIGHT: usize = 100;
const DEFAULT_ZONE_IMAGE_BYTES: usize = DEFAULT_ZONE_IMAGE_WIDTH * DEFAULT_ZONE_IMAGE_HEIGHT * 4;

/// Runtime state for Lightscript injection.
#[derive(Debug, Clone)]
pub struct LightscriptRuntime {
    width: u32,
    height: u32,
    last_controls: HashMap<String, ControlValue>,
    last_interaction: Option<InteractionData>,
}

impl LightscriptRuntime {
    /// Create a runtime shim for a fixed effect canvas size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            last_controls: HashMap::new(),
            last_interaction: None,
        }
    }

    /// Build JavaScript that initializes the `window.engine` object.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        clippy::format_push_string,
        clippy::uninlined_format_args
    )]
    pub fn bootstrap_script(&self) -> String {
        let mut script = String::new();
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(&format!("  window.engine.width = {};\n", self.width));
        script.push_str(&format!("  window.engine.height = {};\n", self.height));

        // Core LightScript contract: audio + vision + zone are always present.
        script.push_str(
            "  if (typeof window.engine.audio !== 'object' || window.engine.audio === null) { window.engine.audio = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.vision !== 'object' || window.engine.vision === null) { window.engine.vision = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.zone !== 'object' || window.engine.zone === null) { window.engine.zone = {}; }\n",
        );
        script.push_str("  window.engine.meters = window.engine.vision;\n");

        // Audio defaults (LightScript-compatible surface + extended fields used by
        // lightscript-workshop helpers).
        script.push_str(&format!(
            "  if (!(window.engine.audio.freq instanceof Int8Array) || window.engine.audio.freq.length !== {}) {{ window.engine.audio.freq = new Int8Array({}); }}\n",
            SPECTRUM_BINS, SPECTRUM_BINS
        ));
        script.push_str(&format!(
            "  if (!(window.engine.audio.frequencyRaw instanceof Int8Array) || window.engine.audio.frequencyRaw.length !== {}) {{ window.engine.audio.frequencyRaw = new Int8Array({}); }}\n",
            SPECTRUM_BINS, SPECTRUM_BINS
        ));
        script.push_str(&format!(
            "  if (!(window.engine.audio.frequency instanceof Float32Array) || window.engine.audio.frequency.length !== {}) {{ window.engine.audio.frequency = new Float32Array({}); }}\n",
            SPECTRUM_BINS, SPECTRUM_BINS
        ));
        script.push_str(&format!(
            "  if (!(window.engine.audio.melBands instanceof Float32Array) || window.engine.audio.melBands.length !== {}) {{ window.engine.audio.melBands = new Float32Array({}); }}\n",
            MEL_BANDS, MEL_BANDS
        ));
        script.push_str(&format!(
            "  if (!(window.engine.audio.melBandsNormalized instanceof Float32Array) || window.engine.audio.melBandsNormalized.length !== {}) {{ window.engine.audio.melBandsNormalized = new Float32Array({}); }}\n",
            MEL_BANDS, MEL_BANDS
        ));
        script.push_str(&format!(
            "  if (!(window.engine.audio.chromagram instanceof Float32Array) || window.engine.audio.chromagram.length !== {}) {{ window.engine.audio.chromagram = new Float32Array({}); }}\n",
            CHROMA_BINS, CHROMA_BINS
        ));
        script.push_str(
            "  if (!(window.engine.audio.spectralFluxBands instanceof Float32Array) || window.engine.audio.spectralFluxBands.length !== 3) { window.engine.audio.spectralFluxBands = new Float32Array(3); }\n",
        );
        script.push_str(&format!(
            "  window.engine.audio.level = {};\n",
            LEVEL_FLOOR_DB
        ));
        script.push_str(&format!(
            "  window.engine.audio.levelRaw = {};\n",
            LEVEL_FLOOR_DB
        ));
        script.push_str("  window.engine.audio.levelLinear = 0;\n");
        script.push_str("  window.engine.audio.rms = 0;\n");
        script.push_str("  window.engine.audio.peak = 0;\n");
        script.push_str("  window.engine.audio.bass = 0;\n");
        script.push_str("  window.engine.audio.mid = 0;\n");
        script.push_str("  window.engine.audio.treble = 0;\n");
        script.push_str("  window.engine.audio.density = 0;\n");
        script.push_str("  window.engine.audio.width = 0.5;\n");
        script.push_str("  window.engine.audio.bpm = 0;\n");
        script.push_str("  window.engine.audio.tempo = 0;\n");
        script.push_str("  window.engine.audio.beat = false;\n");
        script.push_str("  window.engine.audio.beatPulse = 0;\n");
        script.push_str("  window.engine.audio.beatPhase = 0;\n");
        script.push_str("  window.engine.audio.beatConfidence = 0;\n");
        script.push_str("  window.engine.audio.confidence = 0;\n");
        script.push_str("  window.engine.audio.onset = false;\n");
        script.push_str("  window.engine.audio.onsetPulse = 0;\n");
        script.push_str("  window.engine.audio.spectralFlux = 0;\n");
        script.push_str("  window.engine.audio.brightness = 0;\n");
        script.push_str("  window.engine.audio.spread = 0;\n");
        script.push_str("  window.engine.audio.rolloff = 0;\n");
        script.push_str("  window.engine.audio.roughness = 0;\n");
        script.push_str("  window.engine.audio.harmonicHue = 0;\n");
        script.push_str("  window.engine.audio.chordMood = 0;\n");
        script.push_str("  window.engine.audio.dominantPitch = 0;\n");
        script.push_str("  window.engine.audio.dominantPitchConfidence = 0;\n");

        // Screen-ambience payload compatibility (`engine.zone`).
        script.push_str(&format!(
            "  if (!(window.engine.zone.hue instanceof Int16Array) || window.engine.zone.hue.length !== {}) {{ window.engine.zone.hue = new Int16Array({}); }}\n",
            DEFAULT_ZONE_SAMPLES, DEFAULT_ZONE_SAMPLES
        ));
        script.push_str(&format!(
            "  if (!(window.engine.zone.saturation instanceof Int8Array) || window.engine.zone.saturation.length !== {}) {{ window.engine.zone.saturation = new Int8Array({}); }}\n",
            DEFAULT_ZONE_SAMPLES, DEFAULT_ZONE_SAMPLES
        ));
        script.push_str(&format!(
            "  if (!(window.engine.zone.lightness instanceof Int8Array) || window.engine.zone.lightness.length !== {}) {{ window.engine.zone.lightness = new Int8Array({}); }}\n",
            DEFAULT_ZONE_SAMPLES, DEFAULT_ZONE_SAMPLES
        ));
        script.push_str(&format!(
            "  if (!(window.engine.zone.imagedata instanceof Uint8ClampedArray) || window.engine.zone.imagedata.length !== {}) {{ window.engine.zone.imagedata = new Uint8ClampedArray({}); }}\n",
            DEFAULT_ZONE_IMAGE_BYTES, DEFAULT_ZONE_IMAGE_BYTES
        ));
        script.push_str(&format!(
            "  window.engine.zone.width = {};\n",
            DEFAULT_ZONE_WIDTH
        ));
        script.push_str(&format!(
            "  window.engine.zone.height = {};\n",
            DEFAULT_ZONE_HEIGHT
        ));

        // Sensor API used by HTML effects.
        script.push_str(
            "  if (typeof window.engine.sensors !== 'object' || window.engine.sensors === null) { window.engine.sensors = {}; }\n",
        );
        script.push_str(
            "  if (!Array.isArray(window.engine.sensorList)) { window.engine.sensorList = []; }\n",
        );
        script.push_str("  if (typeof window.engine.getSensorValue !== 'function') {\n");
        script.push_str("    window.engine.getSensorValue = function(name) {\n");
        script.push_str(
            "      const sensors = (window.engine && typeof window.engine.sensors === 'object' && window.engine.sensors !== null) ? window.engine.sensors : {};\n",
        );
        script.push_str("      const key = typeof name === 'string' ? name : '';\n");
        script.push_str(
            "      const entry = key && typeof sensors[key] === 'object' && sensors[key] !== null ? sensors[key] : null;\n",
        );
        script
            .push_str("      if (!entry) { return { value: 0, min: 0, max: 100, unit: '%' }; }\n");
        script.push_str("      const value = Number.isFinite(entry.value) ? entry.value : 0;\n");
        script.push_str("      const min = Number.isFinite(entry.min) ? entry.min : 0;\n");
        script.push_str("      let max = Number.isFinite(entry.max) ? entry.max : 100;\n");
        script.push_str("      if (max === min) { max = min + 1; }\n");
        script.push_str("      const unit = typeof entry.unit === 'string' ? entry.unit : '%';\n");
        script.push_str("      return { value, min, max, unit };\n");
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.engine.setSensorValue !== 'function') {\n");
        script.push_str(
            "    window.engine.setSensorValue = function(name, value, min, max, unit) {\n",
        );
        script.push_str("      if (typeof name !== 'string' || name.length === 0) { return; }\n");
        script.push_str(
            "      const safeMin = Number.isFinite(min) ? min : 0;\n      let safeMax = Number.isFinite(max) ? max : 100;\n      if (safeMax === safeMin) { safeMax = safeMin + 1; }\n",
        );
        script.push_str("      window.engine.sensors[name] = {\n");
        script.push_str("        value: Number.isFinite(value) ? value : 0,\n");
        script.push_str("        min: safeMin,\n");
        script.push_str("        max: safeMax,\n");
        script.push_str("        unit: typeof unit === 'string' ? unit : '%',\n");
        script.push_str("      };\n");
        script.push_str(
            "      if (window.engine.sensorList.indexOf(name) === -1) { window.engine.sensorList.push(name); }\n",
        );
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.engine.resetSensors !== 'function') {\n");
        script.push_str("    window.engine.resetSensors = function() {\n");
        script.push_str("      window.engine.sensors = {};\n");
        script.push_str("      window.engine.sensorList = [];\n");
        script.push_str("    };\n");
        script.push_str("  }\n");

        // Vision meter helpers.
        script.push_str("  if (typeof window.engine.getMeterValue !== 'function') {\n");
        script.push_str("    window.engine.getMeterValue = function(name) {\n");
        script.push_str("      if (typeof name !== 'string' || name.length === 0) { return 0; }\n");
        script.push_str("      const raw = window.engine.vision[name];\n");
        script.push_str("      return Number.isFinite(raw) ? raw : 0;\n");
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.engine.setMeterValue !== 'function') {\n");
        script.push_str("    window.engine.setMeterValue = function(name, value) {\n");
        script.push_str("      if (typeof name !== 'string' || name.length === 0) { return; }\n");
        script.push_str("      window.engine.vision[name] = Number.isFinite(value) ? value : 0;\n");
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.engine.setVisionValues !== 'function') {\n");
        script.push_str("    window.engine.setVisionValues = function(values) {\n");
        script.push_str("      if (typeof values !== 'object' || values === null) { return; }\n");
        script.push_str(
            "      for (const key in values) {\n        if (!Object.prototype.hasOwnProperty.call(values, key)) { continue; }\n        const value = values[key];\n        if (Number.isFinite(value)) { window.engine.vision[key] = value; }\n      }\n",
        );
        script.push_str("    };\n");
        script.push_str("  }\n");

        // Keyboard/mouse stubs for interactive effects.
        script.push_str(
            "  if (typeof window.engine.keyboard !== 'object' || window.engine.keyboard === null) { window.engine.keyboard = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.keyboard.keys !== 'object' || window.engine.keyboard.keys === null) { window.engine.keyboard.keys = {}; }\n",
        );
        script.push_str(
            "  if (!Array.isArray(window.engine.keyboard.recent)) { window.engine.keyboard.recent = []; }\n",
        );
        script.push_str("  if (typeof window.engine.keyboard.isKeyDown !== 'function') {\n");
        script.push_str("    window.engine.keyboard.isKeyDown = function(key) {\n");
        script
            .push_str("      if (typeof key !== 'string' || key.length === 0) { return false; }\n");
        script.push_str("      if (window.engine.keyboard.keys[key]) { return true; }\n");
        script.push_str("      return !!window.engine.keyboard.keys[key.toLowerCase()];\n");
        script.push_str("    };\n");
        script.push_str("  }\n");
        script
            .push_str("  if (typeof window.engine.keyboard.consumePressedKeys !== 'function') {\n");
        script.push_str("    window.engine.keyboard.consumePressedKeys = function() {\n");
        script.push_str(
            "      const recent = Array.isArray(window.engine.keyboard.recent) ? window.engine.keyboard.recent.slice() : [];\n",
        );
        script.push_str("      window.engine.keyboard.recent = [];\n");
        script.push_str("      return recent;\n");
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.engine.keyboard.wasKeyPressed !== 'function') {\n");
        script.push_str("    window.engine.keyboard.wasKeyPressed = function(key) {\n");
        script
            .push_str("      if (typeof key !== 'string' || key.length === 0) { return false; }\n");
        script.push_str(
            "      const recent = Array.isArray(window.engine.keyboard.recent) ? window.engine.keyboard.recent : [];\n",
        );
        script.push_str("      const lower = key.toLowerCase();\n");
        script.push_str(
            "      return recent.some(function(entry) { return typeof entry === 'string' && (entry === key || entry.toLowerCase() === lower); });\n",
        );
        script.push_str("    };\n");
        script.push_str("  }\n");

        script.push_str(
            "  if (typeof window.engine.mouse !== 'object' || window.engine.mouse === null) { window.engine.mouse = {}; }\n",
        );
        script.push_str(
            "  if (!Number.isFinite(window.engine.mouse.x)) { window.engine.mouse.x = 0; }\n",
        );
        script.push_str(
            "  if (!Number.isFinite(window.engine.mouse.y)) { window.engine.mouse.y = 0; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.mouse.down !== 'boolean') { window.engine.mouse.down = false; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.mouse.buttons !== 'object' || window.engine.mouse.buttons === null) { window.engine.mouse.buttons = {}; }\n",
        );
        script.push_str("  if (typeof window.engine.mouse.isDown !== 'function') {\n");
        script.push_str("    window.engine.mouse.isDown = function(button) {\n");
        script.push_str(
            "      if ((typeof button !== 'string' || button.length === 0) && typeof button !== 'number') { return !!window.engine.mouse.down; }\n",
        );
        script
            .push_str("      const key = typeof button === 'number' ? String(button) : button;\n");
        script.push_str("      if (window.engine.mouse.buttons[key]) { return true; }\n");
        script.push_str(
            "      return typeof key === 'string' ? !!window.engine.mouse.buttons[key.toLowerCase()] : false;\n",
        );
        script.push_str("    };\n");
        script.push_str("  }\n");

        // Tap/game integration hooks used by HTML effects.
        script.push_str("  if (typeof window.engine.onCanvasTapped !== 'function') {\n");
        script.push_str("    window.engine.onCanvasTapped = function(x, y) {\n");
        script.push_str(
            "      if (typeof window.onCanvasTapped === 'function') { window.onCanvasTapped(x, y); }\n",
        );
        script.push_str("    };\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.onCanvasApiEvent !== 'function') {\n");
        script.push_str("    window.onCanvasApiEvent = function(_event) {};\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof window.showNotification !== 'function') {\n");
        script.push_str("    window.showNotification = function(_message, _isError) {};\n");
        script.push_str("  }\n");
        script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n");
        script.push_str("})();");
        script
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

    /// Append JavaScript for one frame's audio + changed control updates.
    pub fn push_frame_scripts(
        &mut self,
        scripts: &mut Vec<String>,
        audio: &AudioData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        controls: &HashMap<String, ControlValue>,
        include_audio: bool,
        include_screen: bool,
    ) {
        if include_audio {
            scripts.push(Self::audio_update_script(audio));
        }

        if include_screen {
            scripts.push(Self::screen_update_script(screen));
        }
        scripts.push(Self::sensor_update_script(sensors));
        self.push_control_update_scripts(scripts, controls);
    }

    /// Build JavaScript to update `window.engine.keyboard` and `window.engine.mouse`.
    #[must_use]
    pub fn input_update_script(interaction: &InteractionData) -> String {
        let keyboard_keys =
            js_true_object_literal(&keyboard_lookup_keys(&interaction.keyboard.pressed_keys));
        let recent_keys = serde_json::to_string(&interaction.keyboard.recent_keys)
            .unwrap_or_else(|_| "[]".to_owned());
        let mouse_buttons = js_true_object_literal(&mouse_lookup_keys(&interaction.mouse.buttons));
        let mut script = String::with_capacity(
            320_usize
                .saturating_add(keyboard_keys.len())
                .saturating_add(recent_keys.len())
                .saturating_add(mouse_buttons.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.keyboard !== 'object' || window.engine.keyboard === null) { window.engine.keyboard = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.mouse !== 'object' || window.engine.mouse === null) { window.engine.mouse = {}; }\n",
        );
        script.push_str("  window.engine.keyboard.keys = ");
        script.push_str(&keyboard_keys);
        script.push_str(";\n");
        script.push_str("  window.engine.keyboard.recent = ");
        script.push_str(&recent_keys);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.x = ");
        let _ = write!(&mut script, "{}", interaction.mouse.x);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.y = ");
        let _ = write!(&mut script, "{}", interaction.mouse.y);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.down = ");
        script.push_str(js_bool(interaction.mouse.down));
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.buttons = ");
        script.push_str(&mouse_buttons);
        script.push_str(";\n");
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }

    /// Build JavaScript for interaction state when the payload changed.
    #[must_use]
    pub fn input_update_script_if_changed(
        &mut self,
        interaction: &InteractionData,
    ) -> Option<String> {
        if self.last_interaction.as_ref() == Some(interaction) {
            return None;
        }

        self.last_interaction = Some(interaction.clone());
        Some(Self::input_update_script(interaction))
    }

    #[allow(
        clippy::too_many_lines,
        clippy::format_push_string,
        clippy::uninlined_format_args
    )]
    fn audio_update_script(audio: &AudioData) -> String {
        let level_db = normalized_level_to_db(audio.rms_level);
        let level_linear = clamp_unit(audio.rms_level);
        let peak = clamp_unit(audio.peak_level);
        let bass = clamp_unit(audio.bass());
        let mid = clamp_unit(audio.mid());
        let treble = clamp_unit(audio.treble());
        let density = clamp_unit(audio.spectral_flux);
        let brightness = clamp_unit(audio.spectral_centroid);
        let beat_pulse = clamp_unit(audio.beat_pulse);
        let onset_pulse = clamp_unit(audio.onset_pulse);

        let spectral_flux_bands = [bass, mid, treble];

        let spectrum_csv = join_padded_f32_csv(&audio.spectrum, SPECTRUM_BINS);
        let frequency_raw_csv = join_padded_normalized_i8_csv(&audio.spectrum, SPECTRUM_BINS);
        let mel_csv = join_padded_f32_csv(&audio.mel_bands, MEL_BANDS);
        let chroma_csv = join_padded_f32_csv(&audio.chromagram, CHROMA_BINS);
        let flux_bands_csv = join_f32_csv(&spectral_flux_bands);

        let mut script = String::with_capacity(
            1200_usize
                .saturating_add(spectrum_csv.len())
                .saturating_add(frequency_raw_csv.len().saturating_mul(2))
                .saturating_add(mel_csv.len().saturating_mul(2))
                .saturating_add(chroma_csv.len())
                .saturating_add(flux_bands_csv.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.audio !== 'object' || window.engine.audio === null) { window.engine.audio = {}; }\n",
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.level", level_db);
        push_js_f32_assignment(&mut script, "window.engine.audio.levelRaw", level_db);
        push_js_f32_assignment(&mut script, "window.engine.audio.levelLinear", level_linear);
        push_js_f32_assignment(&mut script, "window.engine.audio.rms", level_linear);
        push_js_f32_assignment(&mut script, "window.engine.audio.peak", peak);
        push_js_f32_assignment(&mut script, "window.engine.audio.bass", bass);
        push_js_f32_assignment(&mut script, "window.engine.audio.mid", mid);
        push_js_f32_assignment(&mut script, "window.engine.audio.treble", treble);
        push_js_f32_assignment(&mut script, "window.engine.audio.density", density);
        script.push_str("  window.engine.audio.width = 0.5;\n");
        push_js_f32_assignment(&mut script, "window.engine.audio.bpm", audio.bpm);
        push_js_f32_assignment(&mut script, "window.engine.audio.tempo", audio.bpm);
        push_js_bool_assignment(&mut script, "window.engine.audio.beat", audio.beat_detected);
        push_js_f32_assignment(&mut script, "window.engine.audio.beatPulse", beat_pulse);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.beatPhase",
            clamp_unit(audio.beat_phase),
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.beatConfidence",
            audio.beat_confidence,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.confidence",
            audio.beat_confidence,
        );
        push_js_bool_assignment(
            &mut script,
            "window.engine.audio.onset",
            audio.onset_detected,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.onsetPulse", onset_pulse);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.spectralFlux",
            audio.spectral_flux,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.spectralFluxBands",
            "Float32Array",
            &flux_bands_csv,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.brightness", brightness);
        script.push_str("  window.engine.audio.spread = 0;\n");
        script.push_str("  window.engine.audio.rolloff = 0;\n");
        script.push_str("  window.engine.audio.roughness = 0;\n");
        script.push_str("  window.engine.audio.harmonicHue = 0;\n");
        script.push_str("  window.engine.audio.chordMood = 0;\n");
        script.push_str("  window.engine.audio.dominantPitch = 0;\n");
        script.push_str("  window.engine.audio.dominantPitchConfidence = 0;\n");
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.freq",
            "Int8Array",
            &frequency_raw_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.frequencyRaw",
            "Int8Array",
            &frequency_raw_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.frequency",
            "Float32Array",
            &spectrum_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.melBands",
            "Float32Array",
            &mel_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.melBandsNormalized",
            "Float32Array",
            &mel_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.chromagram",
            "Float32Array",
            &chroma_csv,
        );
        script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n");
        script.push_str("})();");
        script
    }

    fn sensor_update_script(snapshot: &SystemSnapshot) -> String {
        let readings = snapshot.readings();
        let sensor_list = serde_json::to_string(
            &readings
                .iter()
                .map(|reading| reading.label.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_owned());
        let sensors_json =
            serde_json::to_string(&sensor_payload(&readings)).unwrap_or_else(|_| "{}".to_owned());

        let mut script = String::with_capacity(
            256_usize
                .saturating_add(sensor_list.len())
                .saturating_add(sensors_json.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str("  window.engine.sensors = ");
        script.push_str(&sensors_json);
        script.push_str(";\n");
        script.push_str("  window.engine.sensorList = ");
        script.push_str(&sensor_list);
        script.push_str(";\n");
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }

    fn screen_update_script(screen: Option<&ScreenData>) -> String {
        let (grid_width, grid_height, hue_csv, saturation_csv, lightness_csv) =
            screen_payload(screen);
        let mut script = String::with_capacity(
            320_usize
                .saturating_add(hue_csv.len())
                .saturating_add(saturation_csv.len())
                .saturating_add(lightness_csv.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.zone !== 'object' || window.engine.zone === null) { window.engine.zone = {}; }\n",
        );
        let _ = writeln!(script, "  window.engine.zone.width = {grid_width};");
        let _ = writeln!(script, "  window.engine.zone.height = {grid_height};");
        let _ = writeln!(
            script,
            "  window.engine.zone.hue = new Int16Array([{hue_csv}]);"
        );
        let _ = writeln!(
            script,
            "  window.engine.zone.saturation = new Int8Array([{saturation_csv}]);"
        );
        let _ = writeln!(
            script,
            "  window.engine.zone.lightness = new Int8Array([{lightness_csv}]);"
        );
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }

    fn push_control_update_scripts(
        &mut self,
        scripts: &mut Vec<String>,
        controls: &HashMap<String, ControlValue>,
    ) {
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
            "  if (typeof window[callback] === 'function') {{\n",
            "    try {{ window[callback](); }} catch (_err) {{}}\n",
            "  }}\n",
            "}})();",
        ),
        callback_literal,
        key_literal,
        value.to_js_literal(),
    )
}

/// Convert normalized 0..1 level to dB scale used by many `LightScript` effects.
#[must_use]
pub fn normalized_level_to_db(level: f32) -> f32 {
    if !level.is_finite() || level <= 0.0 {
        return LEVEL_FLOOR_DB;
    }

    let db = 20.0 * level.log10();
    db.clamp(LEVEL_FLOOR_DB, 0.0)
}

fn clamp_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn normalized_to_int8(value: f32) -> i8 {
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let scaled = (clamp_unit(value) * 127.0).round() as i16;
    i8::try_from(scaled).unwrap_or_default()
}

fn push_js_f32_assignment(script: &mut String, path: &str, value: f32) {
    script.push_str("  ");
    script.push_str(path);
    script.push_str(" = ");
    push_js_number_literal(script, value);
    script.push_str(";\n");
}

fn push_js_bool_assignment(script: &mut String, path: &str, value: bool) {
    let _ = writeln!(script, "  {path} = {};", js_bool(value));
}

fn push_js_csv_typed_array_assignment(
    script: &mut String,
    path: &str,
    typed_array: &str,
    csv: &str,
) {
    let _ = writeln!(script, "  {path} = new {typed_array}([{csv}]);");
}

fn push_js_number_literal(script: &mut String, value: f32) {
    if value.is_finite() {
        let _ = write!(script, "{value}");
    } else {
        script.push('0');
    }
}

fn join_f32_csv(values: &[f32]) -> String {
    let mut csv = String::with_capacity(values.len().saturating_mul(8));
    for (index, value) in values.iter().copied().enumerate() {
        if index > 0 {
            csv.push(',');
        }
        if value.is_finite() {
            let _ = write!(&mut csv, "{value}");
        } else {
            csv.push('0');
        }
    }
    csv
}

fn join_padded_f32_csv(values: &[f32], expected_len: usize) -> String {
    let mut csv = String::with_capacity(expected_len.saturating_mul(8));
    for index in 0..expected_len {
        if index > 0 {
            csv.push(',');
        }
        let value = values.get(index).copied().unwrap_or_default();
        if value.is_finite() {
            let _ = write!(&mut csv, "{value}");
        } else {
            csv.push('0');
        }
    }
    csv
}

fn join_i16_csv(values: &[i16]) -> String {
    let mut csv = String::with_capacity(values.len().saturating_mul(5));
    for (index, value) in values.iter().copied().enumerate() {
        if index > 0 {
            csv.push(',');
        }
        let _ = write!(&mut csv, "{value}");
    }
    csv
}

fn join_i8_csv(values: &[i8]) -> String {
    let mut csv = String::with_capacity(values.len().saturating_mul(4));
    for (index, value) in values.iter().copied().enumerate() {
        if index > 0 {
            csv.push(',');
        }
        let _ = write!(&mut csv, "{value}");
    }
    csv
}

fn screen_payload(screen: Option<&ScreenData>) -> (u32, u32, String, String, String) {
    let Some(screen) = screen else {
        let sample_count = DEFAULT_ZONE_SAMPLES;
        let zero_hues = vec![0_i16; sample_count];
        let zero_channels = vec![0_i8; sample_count];
        return (
            DEFAULT_ZONE_WIDTH as u32,
            DEFAULT_ZONE_HEIGHT as u32,
            join_i16_csv(&zero_hues),
            join_i8_csv(&zero_channels),
            join_i8_csv(&zero_channels),
        );
    };

    let grid_width = screen.grid_width.max(1);
    let grid_height = screen.grid_height.max(1);
    let sample_count = usize::try_from(grid_width.saturating_mul(grid_height)).unwrap_or(0);
    let mut hue = Vec::with_capacity(sample_count);
    let mut saturation = Vec::with_capacity(sample_count);
    let mut lightness = Vec::with_capacity(sample_count);

    for index in 0..sample_count {
        let rgb = screen
            .zone_colors
            .get(index)
            .and_then(|zone| zone.colors.first().copied())
            .unwrap_or([0, 0, 0]);
        let (h, s, l) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
        hue.push(h);
        saturation.push(s);
        lightness.push(l);
    }

    (
        grid_width,
        grid_height,
        join_i16_csv(&hue),
        join_i8_csv(&saturation),
        join_i8_csv(&lightness),
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (i16, i8, i8) {
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let lightness = (max + min) * 0.5;

    let saturation = if delta <= f32::EPSILON {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };

    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (max - rf).abs() <= f32::EPSILON {
        60.0 * ((gf - bf) / delta).rem_euclid(6.0)
    } else if (max - gf).abs() <= f32::EPSILON {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    (
        hue.round() as i16,
        (saturation.clamp(0.0, 1.0) * 100.0).round() as i8,
        (lightness.clamp(0.0, 1.0) * 100.0).round() as i8,
    )
}

fn join_padded_normalized_i8_csv(values: &[f32], expected_len: usize) -> String {
    let mut csv = String::with_capacity(expected_len.saturating_mul(4));
    for index in 0..expected_len {
        if index > 0 {
            csv.push(',');
        }
        let _ = write!(
            &mut csv,
            "{}",
            normalized_to_int8(values.get(index).copied().unwrap_or_default())
        );
    }
    csv
}

fn js_true_object_literal(values: &[String]) -> String {
    if values.is_empty() {
        return "{}".to_owned();
    }

    let mut object = String::with_capacity(values.len().saturating_mul(16));
    object.push('{');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            object.push(',');
        }
        let key = serde_json::to_string(value).unwrap_or_else(|_| "\"invalid\"".to_owned());
        object.push_str(&key);
        object.push_str(":true");
    }
    object.push('}');
    object
}

fn keyboard_lookup_keys(pressed_keys: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    for name in pressed_keys {
        push_unique(&mut keys, name.clone());
        push_unique(&mut keys, name.to_ascii_lowercase());

        match name.as_str() {
            "Escape" => {
                push_unique(&mut keys, "Esc".to_owned());
                push_unique(&mut keys, "esc".to_owned());
                push_unique(&mut keys, "escape".to_owned());
            }
            "Space" => {
                push_unique(&mut keys, " ".to_owned());
                push_unique(&mut keys, "space".to_owned());
                push_unique(&mut keys, "Spacebar".to_owned());
            }
            "ArrowLeft" => {
                push_unique(&mut keys, "Left".to_owned());
                push_unique(&mut keys, "left".to_owned());
            }
            "ArrowRight" => {
                push_unique(&mut keys, "Right".to_owned());
                push_unique(&mut keys, "right".to_owned());
            }
            "ArrowUp" => {
                push_unique(&mut keys, "Up".to_owned());
                push_unique(&mut keys, "up".to_owned());
            }
            "ArrowDown" => {
                push_unique(&mut keys, "Down".to_owned());
                push_unique(&mut keys, "down".to_owned());
            }
            "ControlLeft" | "ControlRight" => {
                push_unique(&mut keys, "Control".to_owned());
                push_unique(&mut keys, "control".to_owned());
            }
            "ShiftLeft" | "ShiftRight" => {
                push_unique(&mut keys, "Shift".to_owned());
                push_unique(&mut keys, "shift".to_owned());
            }
            "AltLeft" | "AltRight" => {
                push_unique(&mut keys, "Alt".to_owned());
                push_unique(&mut keys, "alt".to_owned());
            }
            "MetaLeft" | "MetaRight" => {
                push_unique(&mut keys, "Meta".to_owned());
                push_unique(&mut keys, "meta".to_owned());
                push_unique(&mut keys, "Command".to_owned());
                push_unique(&mut keys, "command".to_owned());
            }
            _ => {
                if let Some(ch) = single_ascii_alpha(name) {
                    push_unique(&mut keys, ch.to_ascii_uppercase().to_string());
                    push_unique(&mut keys, format!("Key{}", ch.to_ascii_uppercase()));
                } else if let Some(ch) = single_ascii_digit(name) {
                    push_unique(&mut keys, format!("Digit{ch}"));
                    push_unique(&mut keys, format!("Key{ch}"));
                }
            }
        }
    }

    keys
}

fn mouse_lookup_keys(buttons: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    for button in buttons {
        push_unique(&mut keys, button.clone());
        push_unique(&mut keys, button.to_ascii_lowercase());
        match button.as_str() {
            "left" => {
                push_unique(&mut keys, "1".to_owned());
                push_unique(&mut keys, "primary".to_owned());
            }
            "middle" => {
                push_unique(&mut keys, "2".to_owned());
            }
            "right" => {
                push_unique(&mut keys, "3".to_owned());
                push_unique(&mut keys, "secondary".to_owned());
            }
            "button4" => {
                push_unique(&mut keys, "4".to_owned());
            }
            "button5" => {
                push_unique(&mut keys, "5".to_owned());
            }
            _ => {}
        }
    }

    keys
}

fn push_unique(values: &mut Vec<String>, candidate: String) {
    if !values.contains(&candidate) {
        values.push(candidate);
    }
}

fn single_ascii_alpha(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && ch.is_ascii_alphabetic()).then_some(ch)
}

fn single_ascii_digit(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && ch.is_ascii_digit()).then_some(ch)
}

const fn js_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn sensor_payload(readings: &[SensorReading]) -> Value {
    let mut sensors = Map::with_capacity(readings.len());
    for reading in readings {
        let (default_min, default_max) = default_sensor_range(reading);
        sensors.insert(
            reading.label.clone(),
            json!({
                "value": reading.value,
                "min": reading.min.unwrap_or(default_min),
                "max": reading.max.or(reading.critical).unwrap_or(default_max),
                "unit": reading.unit.symbol(),
            }),
        );
    }
    Value::Object(sensors)
}

fn default_sensor_range(reading: &SensorReading) -> (f32, f32) {
    match reading.unit {
        SensorUnit::Celsius => (0.0, reading.critical.unwrap_or(100.0)),
        SensorUnit::Percent => (0.0, 100.0),
        SensorUnit::Megabytes => (0.0, reading.max.unwrap_or(reading.value.max(1.0))),
        SensorUnit::Rpm => (0.0, reading.max.unwrap_or(5000.0)),
        SensorUnit::Watts => (0.0, reading.max.unwrap_or(500.0)),
        SensorUnit::Mhz => (0.0, reading.max.unwrap_or(5000.0)),
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
        assert!(script.contains("window.engine.audio.freq = new Int8Array(200)"));
        assert!(script.contains("window.engine.zone.hue = new Int16Array(560)"));
        assert!(script.contains("window.engine.getSensorValue = function(name)"));
        assert!(script.contains("window.engine.keyboard.isKeyDown = function(key)"));
        assert!(script.contains("window.engine.onCanvasTapped = function(x, y)"));
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
        assert!(script.contains("try { window[callback](); } catch (_err) {}"));
        assert!(script.contains("\"#00ffcc\""));
    }

    #[test]
    fn control_update_script_supports_non_identifier_keys() {
        let script = control_update_script("my-control", &ControlValue::Float(1.0));
        assert!(script.contains("window[\"my-control\"] = 1"));
        assert!(script.contains("const callback = \"onmy-controlChanged\""));
    }

    #[test]
    fn push_frame_scripts_emit_control_deltas_only() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let audio = AudioData::silence();
        let sensors = SystemSnapshot::empty();
        let mut scripts = Vec::new();

        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(0.5));

        runtime.push_frame_scripts(&mut scripts, &audio, None, &sensors, &controls, true, false);
        assert_eq!(
            scripts
                .iter()
                .filter(|script| script.contains("window[\"speed\"]"))
                .count(),
            1
        );
        assert!(
            scripts
                .iter()
                .any(|script| script.contains("window.engine.audio.level"))
        );

        scripts.clear();
        runtime.push_frame_scripts(&mut scripts, &audio, None, &sensors, &controls, true, false);
        assert!(
            scripts
                .iter()
                .all(|script| !script.contains("window[\"speed\"]"))
        );

        controls.insert("speed".to_owned(), ControlValue::Float(0.8));
        scripts.clear();
        runtime.push_frame_scripts(&mut scripts, &audio, None, &sensors, &controls, true, false);
        assert_eq!(
            scripts
                .iter()
                .filter(|script| script.contains("window[\"speed\"]"))
                .count(),
            1
        );
    }

    #[test]
    fn push_frame_scripts_can_skip_audio_update() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let audio = AudioData::silence();
        let sensors = SystemSnapshot::empty();
        let mut scripts = Vec::new();

        runtime.push_frame_scripts(
            &mut scripts,
            &audio,
            None,
            &sensors,
            &HashMap::new(),
            false,
            false,
        );
        assert!(
            scripts
                .iter()
                .all(|script| !script.contains("window.engine.audio.level"))
        );
    }

    #[test]
    fn audio_script_contains_level_and_freq_payload() {
        let mut audio = AudioData::silence();
        audio.rms_level = 1.0;
        audio.beat_pulse = 0.75;
        audio.onset_pulse = 0.5;
        audio.beat_phase = 0.25;
        audio.spectrum = vec![0.1, 0.2, 0.3];

        let script = LightscriptRuntime::audio_update_script(&audio);
        assert!(script.contains("window.engine.audio.level = 0"));
        assert!(script.contains("window.engine.audio.levelRaw = 0"));
        assert!(script.contains("window.engine.audio.beatPulse = 0.75"));
        assert!(script.contains("window.engine.audio.onsetPulse = 0.5"));
        assert!(script.contains("window.engine.audio.beatPhase = 0.25"));
        assert!(script.contains("window.engine.audio.freq = new Int8Array([13,25,38"));
        assert!(script.contains("window.engine.audio.frequency = new Float32Array([0.1,0.2,0.3"));
        assert!(script.contains("window.engine.audio.melBands = new Float32Array(["));
        assert!(script.contains("window.engine.audio.chromagram = new Float32Array(["));
    }

    #[test]
    fn audio_script_sanitizes_non_finite_values() {
        let mut audio = AudioData::silence();
        audio.rms_level = f32::NAN;
        audio.spectrum = vec![f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
        audio.mel_bands = vec![f32::INFINITY, f32::NEG_INFINITY];
        audio.chromagram = vec![f32::NAN];
        audio.bpm = f32::INFINITY;
        audio.spectral_flux = f32::NEG_INFINITY;

        let script = LightscriptRuntime::audio_update_script(&audio);
        assert!(!script.contains("inf"));
        assert!(!script.contains("NaN"));
        assert!(script.contains("window.engine.audio.freq = new Int8Array([0,0,0"));
        assert!(script.contains("window.engine.audio.frequency = new Float32Array([0,0,0"));
        assert!(script.contains("window.engine.audio.melBands = new Float32Array([0,0"));
    }

    #[test]
    fn input_script_populates_keyboard_and_mouse_state() {
        let interaction = InteractionData {
            keyboard: crate::input::KeyboardData {
                pressed_keys: vec!["a".to_owned(), "Space".to_owned()],
                recent_keys: vec!["a".to_owned()],
            },
            mouse: crate::input::MouseData {
                x: 42,
                y: 24,
                buttons: vec!["left".to_owned()],
                down: true,
            },
        };

        let script = LightscriptRuntime::input_update_script(&interaction);
        assert!(script.contains("window.engine.keyboard.keys = {"));
        assert!(script.contains("\"a\":true"));
        assert!(script.contains("\"A\":true"));
        assert!(script.contains("\"KeyA\":true"));
        assert!(script.contains("\"Space\":true"));
        assert!(script.contains("\"space\":true"));
        assert!(script.contains("\" \":true"));
        assert!(script.contains("\"Spacebar\":true"));
        assert!(script.contains("window.engine.keyboard.recent = [\"a\"]"));
        assert!(script.contains("window.engine.mouse.x = 42"));
        assert!(script.contains("window.engine.mouse.y = 24"));
        assert!(script.contains("window.engine.mouse.down = true"));
        assert!(script.contains("window.engine.mouse.buttons = {"));
        assert!(script.contains("\"left\":true"));
        assert!(script.contains("\"1\":true"));
        assert!(script.contains("\"primary\":true"));
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

    #[test]
    fn input_update_script_emits_only_on_change() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let interaction = InteractionData {
            keyboard: crate::input::KeyboardData {
                pressed_keys: vec!["a".to_owned()],
                recent_keys: vec!["a".to_owned()],
            },
            mouse: crate::input::MouseData {
                x: 1,
                y: 2,
                buttons: vec![],
                down: false,
            },
        };

        assert!(
            runtime
                .input_update_script_if_changed(&interaction)
                .is_some()
        );
        assert!(
            runtime
                .input_update_script_if_changed(&interaction)
                .is_none()
        );

        let changed = InteractionData {
            keyboard: crate::input::KeyboardData {
                pressed_keys: vec!["b".to_owned()],
                recent_keys: vec!["b".to_owned()],
            },
            mouse: interaction.mouse.clone(),
        };
        assert!(runtime.input_update_script_if_changed(&changed).is_some());
    }
}
