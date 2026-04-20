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
const EFFECT_SPECTRUM_GAMMA: f32 = 1.8;
const EFFECT_TRANSIENT_GAIN: f32 = 2.8;
const LEVEL_SHORT_ATTACK: f32 = 0.52;
const LEVEL_SHORT_DECAY: f32 = 0.22;
const LEVEL_LONG_ATTACK: f32 = 0.18;
const LEVEL_LONG_DECAY: f32 = 0.05;
const BAND_ENV_ATTACK: f32 = 0.46;
const BAND_ENV_DECAY: f32 = 0.16;
const SWELL_ATTACK: f32 = 0.62;
const SWELL_DECAY: f32 = 0.2;
const MOMENTUM_ATTACK: f32 = 0.28;
const MOMENTUM_DECAY: f32 = 0.16;
const FLUX_BAND_ATTACK: f32 = 0.8;
const FLUX_BAND_DECAY: f32 = 0.85;
const MEL_RUNNING_MAX_DECAY: f32 = 0.999;
const MEL_RUNNING_MAX_FLOOR: f32 = 0.001;
const SPECTRUM_BASS_END: usize = 40;
const SPECTRUM_MID_END: usize = 130;

#[derive(Debug, Clone, Default)]
struct DerivedAudioState {
    level_short: f32,
    level_long: f32,
    bass_env: f32,
    mid_env: f32,
    treble_env: f32,
    momentum: f32,
    swell: f32,
    spectral_flux_bands: [f32; 3],
    previous_band_levels: [f32; 3],
}

impl DerivedAudioState {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Runtime state for Lightscript injection.
#[derive(Debug, Clone)]
pub struct LightscriptRuntime {
    width: u32,
    height: u32,
    last_controls: HashMap<String, ControlValue>,
    last_interaction: Option<InteractionData>,
    last_sensor_readings: Option<Vec<SensorReading>>,
    audio_was_quiet: bool,
    mel_running_max: Vec<f32>,
    audio_state: DerivedAudioState,
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
            last_sensor_readings: None,
            audio_was_quiet: false,
            mel_running_max: vec![MEL_RUNNING_MAX_FLOOR; MEL_BANDS],
            audio_state: DerivedAudioState::default(),
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
            "  if (!(window.engine.audio.frequencyWeighted instanceof Float32Array) || window.engine.audio.frequencyWeighted.length !== {}) {{ window.engine.audio.frequencyWeighted = new Float32Array({}); }}\n",
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
        script.push_str("  window.engine.audio.levelShort = 0;\n");
        script.push_str("  window.engine.audio.levelLong = 0;\n");
        script.push_str("  window.engine.audio.rms = 0;\n");
        script.push_str("  window.engine.audio.peak = 0;\n");
        script.push_str("  window.engine.audio.bass = 0;\n");
        script.push_str("  window.engine.audio.bassEnv = 0;\n");
        script.push_str("  window.engine.audio.mid = 0;\n");
        script.push_str("  window.engine.audio.midEnv = 0;\n");
        script.push_str("  window.engine.audio.treble = 0;\n");
        script.push_str("  window.engine.audio.trebleEnv = 0;\n");
        script.push_str("  window.engine.audio.density = 0;\n");
        script.push_str("  window.engine.audio.momentum = 0;\n");
        script.push_str("  window.engine.audio.swell = 0;\n");
        script.push_str("  window.engine.audio.width = 0.5;\n");
        script.push_str("  window.engine.audio.bpm = 0;\n");
        script.push_str("  window.engine.audio.tempo = 120;\n");
        script.push_str("  window.engine.audio.beat = false;\n");
        script.push_str("  window.engine.audio.beatPulse = 0;\n");
        script.push_str("  window.engine.audio.beatPhase = 0;\n");
        script.push_str("  window.engine.audio.beatConfidence = 0;\n");
        script.push_str("  window.engine.audio.confidence = 0;\n");
        script.push_str("  window.engine.audio.onset = false;\n");
        script.push_str("  window.engine.audio.onsetPulse = 0;\n");
        script.push_str("  window.engine.audio.spectralFlux = 0;\n");
        script.push_str("  window.engine.audio.brightness = 0.5;\n");
        script.push_str("  window.engine.audio.spread = 0.3;\n");
        script.push_str("  window.engine.audio.rolloff = 0.5;\n");
        script.push_str("  window.engine.audio.roughness = 0.2;\n");
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
        if include_audio && self.should_emit_audio_update(audio) {
            scripts.push(self.audio_update_script(audio));
        }

        if include_screen {
            scripts.push(Self::screen_update_script(screen));
        }
        let sensor_readings = sensors.readings();
        if self
            .last_sensor_readings
            .as_ref()
            .is_none_or(|previous| previous != &sensor_readings)
        {
            scripts.push(Self::sensor_update_script_from_readings(&sensor_readings));
            self.last_sensor_readings = Some(sensor_readings);
        }
        self.push_control_update_scripts(scripts, controls);
    }

    fn should_emit_audio_update(&mut self, audio: &AudioData) -> bool {
        let audio_is_quiet = audio_is_quiet(audio);
        let should_emit = !audio_is_quiet || !self.audio_was_quiet;
        self.audio_was_quiet = audio_is_quiet;
        should_emit
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
    fn normalized_mel_bands(&mut self, values: &[f32]) -> Vec<f32> {
        if self.mel_running_max.len() != MEL_BANDS {
            self.mel_running_max
                .resize(MEL_BANDS, MEL_RUNNING_MAX_FLOOR);
        }

        let mut normalized = Vec::with_capacity(MEL_BANDS);
        for index in 0..MEL_BANDS {
            let raw = values
                .get(index)
                .copied()
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or_default();
            let running_max = (self.mel_running_max[index].max(raw) * MEL_RUNNING_MAX_DECAY)
                .max(MEL_RUNNING_MAX_FLOOR);
            self.mel_running_max[index] = running_max;
            normalized.push((raw / running_max).clamp(0.0, 1.0));
        }

        normalized
    }

    fn band_levels(spectrum: &[f32]) -> (f32, f32, f32) {
        (
            band_rms(spectrum, 0, SPECTRUM_BASS_END),
            band_rms(spectrum, SPECTRUM_BASS_END, SPECTRUM_MID_END),
            band_rms(spectrum, SPECTRUM_MID_END, SPECTRUM_BINS),
        )
    }

    fn loudness_scale(level_linear: f32, bass: f32, mid: f32, treble: f32) -> f32 {
        let band_mix = bass * 0.42 + mid * 0.34 + treble * 0.24;
        if band_mix <= f32::EPSILON {
            return 0.0;
        }

        clamp_unit(level_linear / band_mix)
    }

    fn audio_update_script(&mut self, audio: &AudioData) -> String {
        let raw_rms = clamp_unit(audio.rms_level);
        let peak = clamp_unit(audio.peak_level);
        let quiet_frame = audio_is_quiet(audio);
        if quiet_frame {
            self.audio_state.reset();
        }
        let compressed_spectrum = shape_audio_bins(&audio.spectrum);
        let compressed_mel = shape_audio_bins(&audio.mel_bands);
        let (compressed_bass, compressed_mid, compressed_treble) =
            Self::band_levels(&compressed_spectrum);
        let loudness_scale =
            Self::loudness_scale(raw_rms, compressed_bass, compressed_mid, compressed_treble);
        let transient_scale = clamp_unit(raw_rms * EFFECT_TRANSIENT_GAIN);
        let beat_pulse = clamp_unit(audio.beat_pulse * transient_scale);
        let onset_pulse = clamp_unit(audio.onset_pulse * transient_scale);
        let motion = clamp_unit(audio.spectral_flux * transient_scale);
        let shaped_spectrum = scale_audio_bins(&compressed_spectrum, loudness_scale);
        let shaped_mel = scale_audio_bins(&compressed_mel, loudness_scale);
        let mel_shape = self.normalized_mel_bands(&shaped_mel);
        let (spectrum_bass, spectrum_mid, spectrum_treble) = Self::band_levels(&shaped_spectrum);
        let bass = clamp_unit(spectrum_bass + beat_pulse * 0.24);
        let mid = clamp_unit(spectrum_mid);
        let treble = clamp_unit(spectrum_treble);
        let level_linear = clamp_unit(bass * 0.42 + mid * 0.34 + treble * 0.24 + beat_pulse * 0.08);
        let level_db = normalized_level_to_db(level_linear);
        let mel_normalized: Vec<f32> = shaped_mel
            .iter()
            .zip(mel_shape.iter())
            .map(|(value, shape)| clamp_unit(value * shape * (0.9 + beat_pulse * 0.2)))
            .collect();
        let density = clamp_unit(level_linear * 0.88 + motion * 0.12);
        let brightness = clamp_unit(0.22 + treble * 0.6);
        let level_short_target = clamp_unit(level_linear * 1.08 + beat_pulse * 0.06);
        let level_long_target = clamp_unit(level_linear * 0.92 + beat_pulse * 0.02);
        self.audio_state.level_short = smooth_unit(
            self.audio_state.level_short,
            level_short_target,
            LEVEL_SHORT_ATTACK,
            LEVEL_SHORT_DECAY,
        );
        self.audio_state.level_long = smooth_unit(
            self.audio_state.level_long,
            level_long_target,
            LEVEL_LONG_ATTACK,
            LEVEL_LONG_DECAY,
        );

        let bass_env_target = clamp_unit(bass + beat_pulse * 0.14);
        let mid_env_target = clamp_unit(mid + motion * 0.08);
        let treble_env_target = clamp_unit(treble + motion * 0.06);
        self.audio_state.bass_env = smooth_unit(
            self.audio_state.bass_env,
            bass_env_target,
            BAND_ENV_ATTACK,
            BAND_ENV_DECAY,
        );
        self.audio_state.mid_env = smooth_unit(
            self.audio_state.mid_env,
            mid_env_target,
            BAND_ENV_ATTACK,
            BAND_ENV_DECAY,
        );
        self.audio_state.treble_env = smooth_unit(
            self.audio_state.treble_env,
            treble_env_target,
            BAND_ENV_ATTACK,
            BAND_ENV_DECAY,
        );

        let momentum_target = clamp_signed_unit((mid - bass) * 0.55);
        self.audio_state.momentum = smooth_signed_unit(
            self.audio_state.momentum,
            momentum_target,
            MOMENTUM_ATTACK,
            MOMENTUM_DECAY,
        );

        let swell_target = clamp_unit(
            beat_pulse
                .max(onset_pulse * 0.85)
                .max(motion * 0.62)
                .max(level_linear * 0.46),
        );
        self.audio_state.swell = smooth_unit(
            self.audio_state.swell,
            swell_target,
            SWELL_ATTACK,
            SWELL_DECAY,
        );

        let band_levels = [bass, mid, treble];
        let mut spectral_flux_bands = [0.0; 3];
        for (index, current) in band_levels.iter().copied().enumerate() {
            let previous = self.audio_state.previous_band_levels[index];
            let target = clamp_unit((current - previous).max(0.0) * 1.9 + motion * 0.04);
            self.audio_state.spectral_flux_bands[index] = smooth_unit(
                self.audio_state.spectral_flux_bands[index],
                target,
                FLUX_BAND_ATTACK,
                FLUX_BAND_DECAY,
            );
            spectral_flux_bands[index] = self.audio_state.spectral_flux_bands[index];
        }
        self.audio_state.previous_band_levels = band_levels;

        let level_short = self.audio_state.level_short;
        let level_long = self.audio_state.level_long;
        let bass_env = self.audio_state.bass_env;
        let mid_env = self.audio_state.mid_env;
        let treble_env = self.audio_state.treble_env;
        let momentum = self.audio_state.momentum;
        let swell = self.audio_state.swell;
        let width = 0.5;
        let spread = clamp_unit(0.18 + width * 0.58);
        let rolloff = clamp_unit(0.46 + treble * 0.32);
        let roughness = clamp_unit(0.15 + (mid - treble).abs() * 0.45);
        let chord_mood = clamp_signed_unit(mid - bass * 0.48);
        let (dominant_pitch, dominant_pitch_confidence) = dominant_pitch_metrics(&audio.chromagram);
        let harmonic_hue = harmonic_hue(&audio.chromagram, bass, mid, treble);
        let tempo = if audio.bpm.is_finite() && audio.bpm > 0.0 {
            audio.bpm
        } else {
            120.0
        };

        let spectrum_csv = join_padded_f32_csv(&shaped_spectrum, SPECTRUM_BINS);
        let frequency_raw_csv = join_padded_normalized_i8_csv(&shaped_spectrum, SPECTRUM_BINS);
        let frequency_weighted_csv = join_weighted_spectrum_csv(&shaped_spectrum, SPECTRUM_BINS);
        let mel_csv = join_padded_f32_csv(&shaped_mel, MEL_BANDS);
        let mel_norm_csv = join_padded_f32_csv(&mel_normalized, MEL_BANDS);
        let chroma_csv = join_padded_f32_csv(&audio.chromagram, CHROMA_BINS);
        let flux_bands_csv = join_f32_csv(&spectral_flux_bands);

        let mut script = String::with_capacity(
            1200_usize
                .saturating_add(spectrum_csv.len())
                .saturating_add(frequency_raw_csv.len().saturating_mul(2))
                .saturating_add(frequency_weighted_csv.len())
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
        push_js_f32_assignment(&mut script, "window.engine.audio.levelShort", level_short);
        push_js_f32_assignment(&mut script, "window.engine.audio.levelLong", level_long);
        push_js_f32_assignment(&mut script, "window.engine.audio.rms", raw_rms);
        push_js_f32_assignment(&mut script, "window.engine.audio.peak", peak);
        push_js_f32_assignment(&mut script, "window.engine.audio.bass", bass);
        push_js_f32_assignment(&mut script, "window.engine.audio.bassEnv", bass_env);
        push_js_f32_assignment(&mut script, "window.engine.audio.mid", mid);
        push_js_f32_assignment(&mut script, "window.engine.audio.midEnv", mid_env);
        push_js_f32_assignment(&mut script, "window.engine.audio.treble", treble);
        push_js_f32_assignment(&mut script, "window.engine.audio.trebleEnv", treble_env);
        push_js_f32_assignment(&mut script, "window.engine.audio.density", density);
        push_js_f32_assignment(&mut script, "window.engine.audio.momentum", momentum);
        push_js_f32_assignment(&mut script, "window.engine.audio.swell", swell);
        push_js_f32_assignment(&mut script, "window.engine.audio.width", width);
        push_js_f32_assignment(&mut script, "window.engine.audio.bpm", audio.bpm);
        push_js_f32_assignment(&mut script, "window.engine.audio.tempo", tempo);
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
        push_js_f32_assignment(&mut script, "window.engine.audio.spectralFlux", motion);
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.spectralFluxBands",
            "Float32Array",
            &flux_bands_csv,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.brightness", brightness);
        push_js_f32_assignment(&mut script, "window.engine.audio.spread", spread);
        push_js_f32_assignment(&mut script, "window.engine.audio.rolloff", rolloff);
        push_js_f32_assignment(&mut script, "window.engine.audio.roughness", roughness);
        push_js_f32_assignment(&mut script, "window.engine.audio.harmonicHue", harmonic_hue);
        push_js_f32_assignment(&mut script, "window.engine.audio.chordMood", chord_mood);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.dominantPitch",
            f32::from(dominant_pitch),
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.dominantPitchConfidence",
            dominant_pitch_confidence,
        );
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
            "window.engine.audio.frequencyWeighted",
            "Float32Array",
            &frequency_weighted_csv,
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
            &mel_norm_csv,
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

    fn sensor_update_script_from_readings(readings: &[SensorReading]) -> String {
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

fn clamp_signed_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn smooth_unit(current: f32, target: f32, attack: f32, decay: f32) -> f32 {
    let factor = if target > current { attack } else { decay };
    clamp_unit(current + (target - current) * factor)
}

fn smooth_signed_unit(current: f32, target: f32, attack: f32, decay: f32) -> f32 {
    let factor = if target.abs() > current.abs() {
        attack
    } else {
        decay
    };
    clamp_signed_unit(current + (target - current) * factor)
}

fn shape_audio_bins(values: &[f32]) -> Vec<f32> {
    values
        .iter()
        .map(|value| clamp_unit(*value).powf(EFFECT_SPECTRUM_GAMMA))
        .collect()
}

fn scale_audio_bins(values: &[f32], scale: f32) -> Vec<f32> {
    values
        .iter()
        .map(|value| clamp_unit(*value * scale))
        .collect()
}

fn band_rms(values: &[f32], start: usize, end: usize) -> f32 {
    let end = end.min(values.len());
    let start = start.min(end);
    let slice = &values[start..end];
    if slice.is_empty() {
        return 0.0;
    }

    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let count = slice.len() as f32;
    (slice.iter().map(|value| value * value).sum::<f32>() / count).sqrt()
}

fn dominant_pitch_metrics(chromagram: &[f32]) -> (u8, f32) {
    let mut dominant_index = 0_u8;
    let mut dominant_value = 0.0_f32;
    for (index, value) in chromagram.iter().enumerate() {
        let value = clamp_unit(*value);
        if value > dominant_value {
            dominant_index = u8::try_from(index).unwrap_or(u8::MAX);
            dominant_value = value;
        }
    }

    (dominant_index, dominant_value)
}

fn harmonic_hue(chromagram: &[f32], bass: f32, mid: f32, treble: f32) -> f32 {
    let (dominant_pitch, confidence) = dominant_pitch_metrics(chromagram);
    if confidence > f32::EPSILON {
        return (f32::from(dominant_pitch) * 30.0).rem_euclid(360.0);
    }

    (bass * 48.0 + mid * 158.0 + treble * 282.0).rem_euclid(360.0)
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

fn join_weighted_spectrum_csv(values: &[f32], expected_len: usize) -> String {
    let mut csv = String::with_capacity(expected_len.saturating_mul(8));
    for index in 0..expected_len {
        if index > 0 {
            csv.push(',');
        }
        let value = values.get(index).copied().map_or(0.0, clamp_unit);
        let t = if expected_len <= 1 {
            0.0
        } else {
            f32::from(u16::try_from(index).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(expected_len - 1).unwrap_or(u16::MAX))
        };
        push_js_number_literal(&mut csv, value * (0.82 + t * 0.2));
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

fn audio_is_quiet(audio: &AudioData) -> bool {
    !audio.beat_detected
        && !audio.onset_detected
        && audio.beat_pulse.abs() <= f32::EPSILON
        && audio.onset_pulse.abs() <= f32::EPSILON
        && audio.beat_phase.abs() <= f32::EPSILON
        && audio.beat_confidence.abs() <= f32::EPSILON
        && audio.bpm.abs() <= f32::EPSILON
        && audio.rms_level.abs() <= f32::EPSILON
        && audio.peak_level.abs() <= f32::EPSILON
        && audio.spectral_centroid.abs() <= f32::EPSILON
        && audio.spectral_flux.abs() <= f32::EPSILON
        && audio
            .spectrum
            .iter()
            .all(|value| value.abs() <= f32::EPSILON)
        && audio
            .mel_bands
            .iter()
            .all(|value| value.abs() <= f32::EPSILON)
        && audio
            .chromagram
            .iter()
            .all(|value| value.abs() <= f32::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_assignment(script: &str, path: &str) -> f32 {
        let marker = format!("{path} = ");
        let start = script
            .find(&marker)
            .map(|index| index + marker.len())
            .expect("assignment should exist");
        let rest = &script[start..];
        let end = rest.find(';').expect("assignment should terminate");
        rest[..end]
            .trim()
            .parse::<f32>()
            .expect("assignment should parse as f32")
    }

    fn extract_f32_array_assignment(script: &str, path: &str) -> Vec<f32> {
        let marker = format!("{path} = new Float32Array([");
        let start = script
            .find(&marker)
            .map(|index| index + marker.len())
            .expect("typed array assignment should exist");
        let rest = &script[start..];
        let end = rest
            .find("]);")
            .expect("typed array assignment should terminate");
        rest[..end]
            .split(',')
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value
                    .trim()
                    .parse::<f32>()
                    .expect("typed array item should parse as f32")
            })
            .collect()
    }

    #[test]
    fn bootstrap_script_contains_runtime_shape() {
        let runtime = LightscriptRuntime::new(320, 200);
        let script = runtime.bootstrap_script();

        assert!(script.contains("window.engine.width = 320"));
        assert!(script.contains("window.engine.height = 200"));
        assert!(script.contains("window.engine.audio.freq = new Int8Array(200)"));
        assert!(script.contains("window.engine.audio.frequencyWeighted = new Float32Array(200)"));
        assert!(script.contains("window.engine.audio.levelShort = 0"));
        assert!(script.contains("window.engine.audio.bassEnv = 0"));
        assert!(script.contains("window.engine.audio.tempo = 120"));
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
    fn push_frame_scripts_suppresses_repeated_quiet_audio_updates() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let quiet_audio = AudioData::silence();
        let sensors = SystemSnapshot::empty();
        let mut scripts = Vec::new();

        runtime.push_frame_scripts(
            &mut scripts,
            &quiet_audio,
            None,
            &sensors,
            &HashMap::new(),
            true,
            false,
        );
        assert!(
            scripts
                .iter()
                .any(|script| script.contains("window.engine.audio.level"))
        );

        scripts.clear();
        runtime.push_frame_scripts(
            &mut scripts,
            &quiet_audio,
            None,
            &sensors,
            &HashMap::new(),
            true,
            false,
        );
        assert!(
            scripts
                .iter()
                .all(|script| !script.contains("window.engine.audio.level"))
        );
    }

    #[test]
    fn push_frame_scripts_emits_audio_when_quiet_state_ends() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let quiet_audio = AudioData::silence();
        let mut active_audio = AudioData::silence();
        active_audio.rms_level = 0.2;
        let sensors = SystemSnapshot::empty();
        let mut scripts = Vec::new();

        runtime.push_frame_scripts(
            &mut scripts,
            &quiet_audio,
            None,
            &sensors,
            &HashMap::new(),
            true,
            false,
        );
        scripts.clear();

        runtime.push_frame_scripts(
            &mut scripts,
            &active_audio,
            None,
            &sensors,
            &HashMap::new(),
            true,
            false,
        );
        assert!(
            scripts
                .iter()
                .any(|script| script.contains("window.engine.audio.level"))
        );
    }

    #[test]
    fn push_frame_scripts_suppresses_unchanged_sensor_updates() {
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
                .any(|script| script.contains("window.engine.sensors ="))
        );

        scripts.clear();
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
                .all(|script| !script.contains("window.engine.sensors ="))
        );
    }

    #[test]
    fn audio_script_contains_level_and_freq_payload() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = 1.0;
        audio.beat_pulse = 0.75;
        audio.onset_pulse = 0.5;
        audio.beat_phase = 0.25;
        audio.spectrum = vec![1.0; SPECTRUM_BINS];
        audio.mel_bands = vec![1.0; MEL_BANDS];
        audio.chromagram = vec![0.1, 0.6, 0.2];

        let script = runtime.audio_update_script(&audio);
        let level_short = extract_assignment(&script, "window.engine.audio.levelShort");
        let level_long = extract_assignment(&script, "window.engine.audio.levelLong");
        assert!(script.contains("window.engine.audio.level = 0"));
        assert!(script.contains("window.engine.audio.levelRaw = 0"));
        assert!(
            level_short > 0.5 && level_short < 1.0,
            "levelShort should react quickly without being a raw copy: {level_short}"
        );
        assert!(
            level_long > 0.1 && level_long < level_short,
            "levelLong should lag behind the short envelope: {level_long}"
        );
        assert!(script.contains("window.engine.audio.bassEnv ="));
        assert!(script.contains("window.engine.audio.midEnv ="));
        assert!(script.contains("window.engine.audio.trebleEnv ="));
        assert!(script.contains("window.engine.audio.momentum ="));
        assert!(script.contains("window.engine.audio.swell ="));
        assert!(script.contains("window.engine.audio.beatPulse = 0.75"));
        assert!(script.contains("window.engine.audio.onsetPulse = 0.5"));
        assert!(script.contains("window.engine.audio.beatPhase = 0.25"));
        assert!(script.contains("window.engine.audio.freq = new Int8Array([127,127,127"));
        assert!(script.contains("window.engine.audio.frequency = new Float32Array([1,1,1"));
        assert!(script.contains("window.engine.audio.frequencyWeighted = new Float32Array([0.82,"));
        assert!(script.contains("window.engine.audio.dominantPitch = 1"));
        assert!(script.contains("window.engine.audio.melBands = new Float32Array(["));
        assert!(
            script.contains("window.engine.audio.melBandsNormalized = new Float32Array([1,1,1")
        );
        assert!(script.contains("window.engine.audio.chromagram = new Float32Array(["));
    }

    #[test]
    fn audio_script_sanitizes_non_finite_values() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = f32::NAN;
        audio.spectrum = vec![f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
        audio.mel_bands = vec![f32::INFINITY, f32::NEG_INFINITY];
        audio.chromagram = vec![f32::NAN];
        audio.bpm = f32::INFINITY;
        audio.spectral_flux = f32::NEG_INFINITY;

        let script = runtime.audio_update_script(&audio);
        assert!(!script.contains("inf"));
        assert!(!script.contains("NaN"));
        assert!(script.contains("window.engine.audio.freq = new Int8Array([0,0,0"));
        assert!(script.contains("window.engine.audio.frequency = new Float32Array([0,0,0"));
        assert!(script.contains("window.engine.audio.melBands = new Float32Array([0,0"));
    }

    #[test]
    fn normalized_mel_bands_track_running_max_decay() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let seed = vec![1.0, 0.5, 0.25];
        let initial = runtime.normalized_mel_bands(&seed);
        assert_eq!(initial[0], 1.0);
        assert_eq!(initial[1], 1.0);
        assert_eq!(initial[2], 1.0);

        let quieter = vec![0.5, 0.25, 0.125];
        let normalized = runtime.normalized_mel_bands(&quieter);
        assert!(normalized[0] > 0.49 && normalized[0] < 0.52);
        assert!(normalized[1] > 0.49 && normalized[1] < 0.52);
        assert!(normalized[2] > 0.49 && normalized[2] < 0.52);
    }

    #[test]
    fn audio_script_curves_live_like_band_energy_into_reactive_range() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = 0.08;
        for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.64;
        }
        for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.60;
        }
        for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
            *value = 0.20;
        }
        audio.mel_bands = vec![0.64; MEL_BANDS];

        let script = runtime.audio_update_script(&audio);
        let bass = extract_assignment(&script, "window.engine.audio.bass");
        let mid = extract_assignment(&script, "window.engine.audio.mid");
        let treble = extract_assignment(&script, "window.engine.audio.treble");
        let level = extract_assignment(&script, "window.engine.audio.levelLinear");

        assert!(
            bass > 0.09 && bass < 0.12,
            "bass should follow loudness instead of pinning: {bass}"
        );
        assert!(
            mid > 0.08 && mid < 0.11,
            "mid should track body without flooding the shader: {mid}"
        );
        assert!(
            treble > 0.01 && treble < 0.02,
            "treble should stay present but restrained: {treble}"
        );
        assert!(
            level > 0.07 && level < 0.09,
            "overall level should stay close to measured loudness: {level}"
        );
        assert!(
            (extract_assignment(&script, "window.engine.audio.rms") - 0.08).abs() < 0.0001,
            "raw RMS should stay available for diagnostics"
        );
    }

    #[test]
    fn audio_script_transient_channels_follow_measured_loudness() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = 0.08;
        audio.beat_pulse = 1.0;
        audio.onset_pulse = 0.9;
        audio.spectral_flux = 0.8;
        for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.64;
        }
        for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.60;
        }
        for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
            *value = 0.20;
        }

        let script = runtime.audio_update_script(&audio);
        let beat_pulse = extract_assignment(&script, "window.engine.audio.beatPulse");
        let onset_pulse = extract_assignment(&script, "window.engine.audio.onsetPulse");
        let spectral_flux = extract_assignment(&script, "window.engine.audio.spectralFlux");
        let swell = extract_assignment(&script, "window.engine.audio.swell");

        assert!(
            beat_pulse > 0.20 && beat_pulse < 0.24,
            "quiet material should not export near-full beat pulses: {beat_pulse}"
        );
        assert!(
            onset_pulse > 0.17 && onset_pulse < 0.21,
            "quiet material should keep onset pulses in check: {onset_pulse}"
        );
        assert!(
            spectral_flux > 0.17 && spectral_flux < 0.19,
            "spectral flux should track loudness instead of flooding motion: {spectral_flux}"
        );
        assert!(
            swell < 0.27,
            "swell should stay restrained for quiet content: {swell}"
        );
    }

    #[test]
    fn audio_script_keeps_strong_bass_hits_capable_of_triggering_shockwave() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = 0.28;
        audio.beat_pulse = 1.0;
        audio.onset_pulse = 1.0;
        audio.spectral_flux = 0.65;
        for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.88;
        }
        for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.60;
        }
        for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
            *value = 0.33;
        }

        let script = runtime.audio_update_script(&audio);
        let bass = extract_assignment(&script, "window.engine.audio.bass");
        let beat_pulse = extract_assignment(&script, "window.engine.audio.beatPulse");
        let onset_pulse = extract_assignment(&script, "window.engine.audio.onsetPulse");
        let level = extract_assignment(&script, "window.engine.audio.levelLinear");

        assert!(
            bass > 0.55,
            "strong bass hits should stay substantial: {bass}"
        );
        assert!(
            beat_pulse > 0.75,
            "loud beats should still export punchy beat pulses: {beat_pulse}"
        );
        assert!(
            onset_pulse > 0.75,
            "loud onsets should still export punchy onset pulses: {onset_pulse}"
        );
        assert!(
            level > 0.40,
            "overall level should rise for big transients: {level}"
        );
    }

    #[test]
    fn audio_script_derived_envelopes_keep_decay_after_a_drop() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut loud = AudioData::silence();
        loud.rms_level = 0.26;
        loud.beat_pulse = 0.7;
        loud.onset_pulse = 0.55;
        for value in &mut loud.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.88;
        }
        for value in &mut loud.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.58;
        }
        for value in &mut loud.spectrum[SPECTRUM_MID_END..] {
            *value = 0.24;
        }
        runtime.audio_update_script(&loud);

        let mut quiet = AudioData::silence();
        quiet.rms_level = 0.03;
        for value in &mut quiet.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.12;
        }
        for value in &mut quiet.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.09;
        }
        for value in &mut quiet.spectrum[SPECTRUM_MID_END..] {
            *value = 0.04;
        }

        let script = runtime.audio_update_script(&quiet);
        let bass = extract_assignment(&script, "window.engine.audio.bass");
        let bass_env = extract_assignment(&script, "window.engine.audio.bassEnv");
        let level = extract_assignment(&script, "window.engine.audio.levelLinear");
        let level_short = extract_assignment(&script, "window.engine.audio.levelShort");
        let level_long = extract_assignment(&script, "window.engine.audio.levelLong");

        assert!(
            bass_env > bass,
            "bassEnv should preserve transient decay: bass={bass}, bassEnv={bass_env}"
        );
        assert!(
            level_short > level,
            "levelShort should hold above the instantaneous level after a drop: level={level}, levelShort={level_short}"
        );
        assert!(
            level_long > level,
            "levelLong should remain above the instantaneous level after a drop: level={level}, levelLong={level_long}"
        );
    }

    #[test]
    fn audio_script_flux_bands_track_change_not_steady_loudness() {
        let mut runtime = LightscriptRuntime::new(320, 200);
        let mut audio = AudioData::silence();
        audio.rms_level = 0.2;
        for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
            *value = 0.72;
        }
        for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
            *value = 0.44;
        }
        for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
            *value = 0.18;
        }

        let first = runtime.audio_update_script(&audio);
        let first_flux =
            extract_f32_array_assignment(&first, "window.engine.audio.spectralFluxBands");
        let second = runtime.audio_update_script(&audio);
        let second_flux =
            extract_f32_array_assignment(&second, "window.engine.audio.spectralFluxBands");

        assert!(
            first_flux[0] > second_flux[0],
            "steady audio should reduce bass motion on the next frame: first={:?}, second={:?}",
            first_flux,
            second_flux
        );
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
