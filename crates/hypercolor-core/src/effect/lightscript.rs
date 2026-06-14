//! `LightScript` runtime shim helpers.
//!
//! This module builds JavaScript snippets for bootstrapping and per-frame
//! runtime injection without binding directly to any specific web engine.

use std::collections::{BTreeMap, HashMap};

use hypercolor_types::audio::{AudioData, CHROMA_BINS, MEL_BANDS, SPECTRUM_BINS};
use hypercolor_types::effect::ControlValue;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::media::MediaState;
use hypercolor_types::net::NetStats;
use hypercolor_types::sensor::{SensorReading, SystemSnapshot};

use crate::input::InteractionData;

mod payload;

use super::traits::FrameInput;
use payload::{
    LightScriptAudioPayload, LightScriptCanvasPayload, LightScriptControlValue,
    LightScriptFramePayload, LightScriptInteractionPayload, LightScriptLightingPayload,
    LightScriptMediaPayload, LightScriptNetPayload, LightScriptScreenPayload,
    LightScriptSensorPayload, LightScriptTimingPayload, sanitize_f32,
};

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

#[derive(Debug, Clone, Copy)]
pub struct LightScriptFrameUpdateOptions<'a> {
    pub include_audio: bool,
    pub include_screen: bool,
    pub include_sensors: bool,
    pub include_interaction: bool,
    pub include_media: bool,
    pub include_net: bool,
    pub include_lighting: bool,
    pub render_host_frame: bool,
    pub selected_sensor_labels: Option<&'a [String]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LightScriptFrameUpdate {
    PayloadJson(String),
    HostFrameScript(String),
}

/// Runtime state for Lightscript injection.
#[derive(Debug, Clone)]
pub struct LightscriptRuntime {
    width: u32,
    height: u32,
    last_controls: HashMap<String, ControlValue>,
    last_interaction: Option<InteractionData>,
    last_sensor_readings: Option<Vec<SensorReading>>,
    last_sensor_labels: Option<Vec<String>>,
    last_media: Option<MediaState>,
    last_media_track_key: Option<String>,
    last_net: Option<NetStats>,
    last_lighting: Option<LightingState>,
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
            last_sensor_labels: None,
            last_media: None,
            last_media_track_key: None,
            last_net: None,
            last_lighting: None,
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

        // Typed data-source defaults (media, net, lighting) so faces can read
        // them before the first gated payload arrives.
        script.push_str(
            "  if (typeof window.engine.media !== 'object' || window.engine.media === null) { window.engine.media = { available: false, playing: false, track: '', artist: '', album: '', artDataUrl: null, positionMs: 0, durationMs: 0, player: '' }; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.net !== 'object' || window.engine.net === null) { window.engine.net = { rxBps: 0, txBps: 0, iface: '' }; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.lighting !== 'object' || window.engine.lighting === null) { window.engine.lighting = { sceneName: null, effectNames: [], dominantColors: [] }; }\n",
        );

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
        script.push_str("  window.__hypercolorControlsDirty = true;\n");
        script.push_str("  window.__hypercolorLastControlUpdateTime = -Infinity;\n");
        script.push_str("  if (typeof window.__hypercolorRenderHostFrame !== 'function') {\n");
        script.push_str("  window.__hypercolorRenderHostFrame = function() {\n");
        script.push_str(
            "    if (!window.__hypercolorCaptureMode || typeof window !== 'object') return;\n",
        );
        script.push_str("    const instance = window.effectInstance;\n");
        script.push_str("    if (!instance || typeof instance.render !== 'function') return;\n");
        script.push_str(
            "    if (typeof window.currentAnimationFrame === 'number' && typeof window.cancelAnimationFrame === 'function') {\n",
        );
        script.push_str("      try { window.cancelAnimationFrame(window.currentAnimationFrame); } catch (_err) {}\n");
        script.push_str("      window.currentAnimationFrame = undefined;\n");
        script.push_str("    }\n");
        script.push_str("    if (typeof instance.syncCanvasSizeFromEngine === 'function') {\n");
        script.push_str("      try { instance.syncCanvasSizeFromEngine(); } catch (_err) {}\n");
        script.push_str("    }\n");
        script.push_str(
            "    const time = (window.performance && typeof window.performance.now === 'function') ? window.performance.now() * 0.001 : Date.now() * 0.001;\n",
        );
        script.push_str("    const shouldUpdateControls = !!window.__hypercolorControlsDirty || time - window.__hypercolorLastControlUpdateTime >= 0.1;\n");
        script.push_str("    if (shouldUpdateControls && typeof window.update === 'function') {\n");
        script.push_str("      try {\n");
        script.push_str("        window.update(false);\n");
        script.push_str("        window.__hypercolorControlsDirty = false;\n");
        script.push_str("        window.__hypercolorLastControlUpdateTime = time;\n");
        script.push_str("      } catch (_err) {}\n");
        script.push_str("    }\n");
        script.push_str("    instance.render(time);\n");
        script.push_str("  };\n");
        script.push_str("  }\n");
        script.push_str(frame_payload_adapter_script());
        script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n");
        script.push_str("})();");
        script
    }

    pub fn frame_payload_json(
        &mut self,
        input: &FrameInput<'_>,
        controls: &HashMap<String, ControlValue>,
        options: LightScriptFrameUpdateOptions<'_>,
    ) -> Option<String> {
        self.frame_payload(input, controls, options)
            .map(|payload| payload.to_json_string())
    }

    pub(crate) fn frame_update(
        &mut self,
        input: &FrameInput<'_>,
        controls: &HashMap<String, ControlValue>,
        options: LightScriptFrameUpdateOptions<'_>,
    ) -> Option<LightScriptFrameUpdate> {
        self.frame_payload(input, controls, options).map(|payload| {
            if payload.is_host_frame_only() {
                LightScriptFrameUpdate::HostFrameScript(host_frame_script(&payload))
            } else {
                LightScriptFrameUpdate::PayloadJson(payload.to_json_string())
            }
        })
    }

    fn frame_payload(
        &mut self,
        input: &FrameInput<'_>,
        controls: &HashMap<String, ControlValue>,
        options: LightScriptFrameUpdateOptions<'_>,
    ) -> Option<LightScriptFramePayload> {
        let canvas_changed = self.update_canvas_size(input.canvas_width, input.canvas_height);
        let audio = (options.include_audio && self.should_emit_audio_update(input.audio))
            .then(|| self.audio_payload(input.audio));
        let screen = options
            .include_screen
            .then(|| LightScriptScreenPayload::from_screen(input.screen));
        let sensors = options
            .include_sensors
            .then(|| self.sensor_payload(input.sensors, options.selected_sensor_labels))
            .flatten();
        let media = options
            .include_media
            .then(|| self.media_payload(input.sources.media))
            .flatten();
        let net = options
            .include_net
            .then(|| self.net_payload(input.sources.net))
            .flatten();
        let lighting = options
            .include_lighting
            .then(|| self.lighting_payload(input.sources.lighting))
            .flatten();
        let controls = self.changed_control_payload(controls);
        let interaction = (options.include_interaction
            && self.last_interaction.as_ref() != Some(input.interaction))
        .then(|| {
            self.last_interaction = Some(input.interaction.clone());
            LightScriptInteractionPayload::from_interaction(input.interaction)
        });

        let should_emit = canvas_changed
            || audio.is_some()
            || screen.is_some()
            || sensors.is_some()
            || media.is_some()
            || net.is_some()
            || lighting.is_some()
            || !controls.is_empty()
            || interaction.is_some()
            || options.render_host_frame;
        should_emit.then(|| LightScriptFramePayload {
            timing: LightScriptTimingPayload {
                time_secs: sanitize_f32(input.time_secs),
                delta_secs: sanitize_f32(input.delta_secs),
                frame_number: input.frame_number,
            },
            canvas: LightScriptCanvasPayload {
                width: self.width,
                height: self.height,
            },
            audio,
            screen,
            sensors,
            media,
            net,
            lighting,
            controls,
            interaction,
            render_host_frame: options.render_host_frame,
        })
    }

    /// Emit a media payload when the state changed; album art rides along
    /// only when the track changed so steady-state frames stay small.
    fn media_payload(&mut self, media: Option<&MediaState>) -> Option<LightScriptMediaPayload> {
        let media = media?;
        if self.last_media.as_ref() == Some(media) {
            return None;
        }

        let track_key = media.available.then(|| media.track_key());
        // Art rides along on track change, but also when it changes within
        // a track — some players publish artwork moments after the title.
        let include_art = self.last_media.is_none()
            || track_key != self.last_media_track_key
            || self
                .last_media
                .as_ref()
                .is_some_and(|previous| previous.art_data_url != media.art_data_url);
        self.last_media = Some(media.clone());
        self.last_media_track_key = track_key;
        Some(LightScriptMediaPayload::from_state(media, include_art))
    }

    fn net_payload(&mut self, net: Option<&NetStats>) -> Option<LightScriptNetPayload> {
        let net = net?;
        if self.last_net.as_ref() == Some(net) {
            return None;
        }

        self.last_net = Some(net.clone());
        Some(LightScriptNetPayload::from_stats(net))
    }

    fn lighting_payload(
        &mut self,
        lighting: Option<&LightingState>,
    ) -> Option<LightScriptLightingPayload> {
        let lighting = lighting?;
        if self.last_lighting.as_ref() == Some(lighting) {
            return None;
        }

        self.last_lighting = Some(lighting.clone());
        Some(LightScriptLightingPayload::from_state(lighting))
    }

    fn update_canvas_size(&mut self, width: u32, height: u32) -> bool {
        if self.width == width && self.height == height {
            return false;
        }

        self.width = width;
        self.height = height;
        true
    }

    fn should_emit_audio_update(&mut self, audio: &AudioData) -> bool {
        let audio_is_quiet = audio_is_quiet(audio);
        let should_emit = !audio_is_quiet || !self.audio_was_quiet;
        self.audio_was_quiet = audio_is_quiet;
        should_emit
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

    fn audio_payload(&mut self, audio: &AudioData) -> LightScriptAudioPayload {
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

        LightScriptAudioPayload {
            level_db,
            level_linear,
            level_short,
            level_long,
            raw_rms,
            peak,
            bass,
            bass_env,
            mid,
            mid_env,
            treble,
            treble_env,
            density,
            momentum,
            swell,
            width,
            bpm: audio.bpm,
            tempo,
            beat: audio.beat_detected,
            beat_pulse,
            beat_phase: clamp_unit(audio.beat_phase),
            beat_confidence: audio.beat_confidence,
            onset: audio.onset_detected,
            onset_pulse,
            spectral_flux: motion,
            spectral_flux_bands: spectral_flux_bands.map(sanitize_f32).to_vec(),
            brightness,
            spread,
            rolloff,
            roughness,
            harmonic_hue,
            chord_mood,
            dominant_pitch: f32::from(dominant_pitch),
            dominant_pitch_confidence,
            frequency_raw: padded_normalized_i8_vec(&shaped_spectrum, SPECTRUM_BINS),
            frequency: padded_f32_vec(&shaped_spectrum, SPECTRUM_BINS),
            frequency_weighted: weighted_spectrum_vec(&shaped_spectrum, SPECTRUM_BINS),
            mel_bands: padded_f32_vec(&shaped_mel, MEL_BANDS),
            mel_bands_normalized: padded_f32_vec(&mel_normalized, MEL_BANDS),
            chromagram: padded_f32_vec(&audio.chromagram, CHROMA_BINS),
        }
    }

    fn sensor_payload(
        &mut self,
        sensors: &SystemSnapshot,
        selected_sensor_labels: Option<&[String]>,
    ) -> Option<LightScriptSensorPayload> {
        let all_sensor_readings = sensors.readings();
        let sensor_labels = sensor_labels_from_readings(&all_sensor_readings);
        let sensor_labels_changed = self
            .last_sensor_labels
            .as_ref()
            .is_none_or(|previous| previous != &sensor_labels);
        let selected_sensor_readings = selected_sensor_labels
            .map(|labels| filter_sensor_readings(&all_sensor_readings, labels))
            .filter(|readings| !readings.is_empty());
        let replace_sensor_map = selected_sensor_readings.is_none()
            || self.last_sensor_readings.is_none()
            || sensor_labels_changed;
        let sensor_readings = if replace_sensor_map {
            all_sensor_readings
        } else {
            selected_sensor_readings.expect("selected readings checked as present")
        };
        if self
            .last_sensor_readings
            .as_ref()
            .is_none_or(|previous| previous != &sensor_readings)
            || sensor_labels_changed
        {
            self.last_sensor_readings = Some(sensor_readings.clone());
            self.last_sensor_labels = Some(sensor_labels.clone());
            return Some(LightScriptSensorPayload::from_readings(
                &sensor_readings,
                replace_sensor_map,
                sensor_labels_changed.then_some(sensor_labels.as_slice()),
            ));
        }

        None
    }

    fn changed_control_payload(
        &mut self,
        controls: &HashMap<String, ControlValue>,
    ) -> BTreeMap<String, LightScriptControlValue> {
        let mut changed_controls = BTreeMap::new();
        for (name, value) in controls {
            let changed = self
                .last_controls
                .get(name)
                .is_none_or(|previous| previous != value);

            if !changed {
                continue;
            }

            changed_controls.insert(
                name.clone(),
                LightScriptControlValue::from_control_value(value),
            );
            self.last_controls.insert(name.clone(), value.clone());
        }
        changed_controls
    }
}

fn host_frame_script(payload: &LightScriptFramePayload) -> String {
    let time_secs = payload.timing.time_secs;
    let delta_secs = payload.timing.delta_secs;
    let frame_number = payload.timing.frame_number;
    let width = payload.canvas.width;
    let height = payload.canvas.height;
    format!(
        "window.__hypercolorApplyHostFrame({time_secs},{delta_secs},{frame_number},{width},{height});"
    )
}

fn frame_payload_adapter_script() -> &'static str {
    include_str!("lightscript/frame_payload_adapter.js")
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

fn padded_f32_vec(values: &[f32], expected_len: usize) -> Vec<f32> {
    let mut padded = Vec::with_capacity(expected_len);
    for index in 0..expected_len {
        padded.push(sanitize_f32(values.get(index).copied().unwrap_or_default()));
    }
    padded
}

fn weighted_spectrum_vec(values: &[f32], expected_len: usize) -> Vec<f32> {
    let mut weighted = Vec::with_capacity(expected_len);
    for index in 0..expected_len {
        let value = values.get(index).copied().map_or(0.0, clamp_unit);
        let t = if expected_len <= 1 {
            0.0
        } else {
            f32::from(u16::try_from(index).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(expected_len - 1).unwrap_or(u16::MAX))
        };
        weighted.push(sanitize_f32(value * (0.82 + t * 0.2)));
    }
    weighted
}

fn padded_normalized_i8_vec(values: &[f32], expected_len: usize) -> Vec<i8> {
    let mut padded = Vec::with_capacity(expected_len);
    for index in 0..expected_len {
        padded.push(normalized_to_int8(
            values.get(index).copied().unwrap_or_default(),
        ));
    }
    padded
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

fn sensor_labels_from_readings(readings: &[SensorReading]) -> Vec<String> {
    readings
        .iter()
        .map(|reading| reading.label.clone())
        .collect()
}

fn filter_sensor_readings(
    readings: &[SensorReading],
    selected_sensor_labels: &[String],
) -> Vec<SensorReading> {
    let mut selected = Vec::with_capacity(selected_sensor_labels.len());
    for label in selected_sensor_labels {
        if selected
            .iter()
            .any(|reading: &SensorReading| reading.label.eq_ignore_ascii_case(label))
        {
            continue;
        }
        if let Some(reading) = readings
            .iter()
            .find(|reading| reading.label.eq_ignore_ascii_case(label))
        {
            selected.push(reading.clone());
        }
    }
    selected
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
mod tests;
