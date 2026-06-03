use std::fmt::Write as _;

use hypercolor_types::sensor::SensorReading;

use crate::input::{InteractionData, ScreenData};

use super::{
    DEFAULT_ZONE_HEIGHT, DEFAULT_ZONE_SAMPLES, DEFAULT_ZONE_WIDTH, join_i8_csv, join_i16_csv,
    js_bool, js_true_object_literal, keyboard_lookup_keys, mouse_lookup_keys,
    push_js_bool_assignment, push_js_csv_typed_array_assignment, push_js_f32_assignment,
    rgb_to_hsl, sensor_payload,
};

pub(super) struct InputScriptPayload {
    keyboard_keys: String,
    recent_keys: String,
    mouse_x: i32,
    mouse_y: i32,
    mouse_down: bool,
    mouse_buttons: String,
}

impl InputScriptPayload {
    pub(super) fn from_interaction(interaction: &InteractionData) -> Self {
        Self {
            keyboard_keys: js_true_object_literal(&keyboard_lookup_keys(
                &interaction.keyboard.pressed_keys,
            )),
            recent_keys: serde_json::to_string(&interaction.keyboard.recent_keys)
                .unwrap_or_else(|_| "[]".to_owned()),
            mouse_x: interaction.mouse.x,
            mouse_y: interaction.mouse.y,
            mouse_down: interaction.mouse.down,
            mouse_buttons: js_true_object_literal(&mouse_lookup_keys(&interaction.mouse.buttons)),
        }
    }

    pub(super) fn script(&self) -> String {
        let mut script = String::with_capacity(
            320_usize
                .saturating_add(self.keyboard_keys.len())
                .saturating_add(self.recent_keys.len())
                .saturating_add(self.mouse_buttons.len()),
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
        script.push_str(&self.keyboard_keys);
        script.push_str(";\n");
        script.push_str("  window.engine.keyboard.recent = ");
        script.push_str(&self.recent_keys);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.x = ");
        let _ = write!(&mut script, "{}", self.mouse_x);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.y = ");
        let _ = write!(&mut script, "{}", self.mouse_y);
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.down = ");
        script.push_str(js_bool(self.mouse_down));
        script.push_str(";\n");
        script.push_str("  window.engine.mouse.buttons = ");
        script.push_str(&self.mouse_buttons);
        script.push_str(";\n");
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }
}

pub(super) struct AudioScriptPayload {
    pub(super) level_db: f32,
    pub(super) level_linear: f32,
    pub(super) level_short: f32,
    pub(super) level_long: f32,
    pub(super) raw_rms: f32,
    pub(super) peak: f32,
    pub(super) bass: f32,
    pub(super) bass_env: f32,
    pub(super) mid: f32,
    pub(super) mid_env: f32,
    pub(super) treble: f32,
    pub(super) treble_env: f32,
    pub(super) density: f32,
    pub(super) momentum: f32,
    pub(super) swell: f32,
    pub(super) width: f32,
    pub(super) bpm: f32,
    pub(super) tempo: f32,
    pub(super) beat: bool,
    pub(super) beat_pulse: f32,
    pub(super) beat_phase: f32,
    pub(super) beat_confidence: f32,
    pub(super) onset: bool,
    pub(super) onset_pulse: f32,
    pub(super) spectral_flux: f32,
    pub(super) spectral_flux_bands_csv: String,
    pub(super) brightness: f32,
    pub(super) spread: f32,
    pub(super) rolloff: f32,
    pub(super) roughness: f32,
    pub(super) harmonic_hue: f32,
    pub(super) chord_mood: f32,
    pub(super) dominant_pitch: f32,
    pub(super) dominant_pitch_confidence: f32,
    pub(super) frequency_raw_csv: String,
    pub(super) spectrum_csv: String,
    pub(super) frequency_weighted_csv: String,
    pub(super) mel_csv: String,
    pub(super) mel_norm_csv: String,
    pub(super) chroma_csv: String,
}

impl AudioScriptPayload {
    pub(super) fn script(&self) -> String {
        let mut script = String::with_capacity(
            1200_usize
                .saturating_add(self.spectrum_csv.len())
                .saturating_add(self.frequency_raw_csv.len().saturating_mul(2))
                .saturating_add(self.frequency_weighted_csv.len())
                .saturating_add(self.mel_csv.len().saturating_mul(2))
                .saturating_add(self.chroma_csv.len())
                .saturating_add(self.spectral_flux_bands_csv.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.audio !== 'object' || window.engine.audio === null) { window.engine.audio = {}; }\n",
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.level", self.level_db);
        push_js_f32_assignment(&mut script, "window.engine.audio.levelRaw", self.level_db);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.levelLinear",
            self.level_linear,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.levelShort",
            self.level_short,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.levelLong",
            self.level_long,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.rms", self.raw_rms);
        push_js_f32_assignment(&mut script, "window.engine.audio.peak", self.peak);
        push_js_f32_assignment(&mut script, "window.engine.audio.bass", self.bass);
        push_js_f32_assignment(&mut script, "window.engine.audio.bassEnv", self.bass_env);
        push_js_f32_assignment(&mut script, "window.engine.audio.mid", self.mid);
        push_js_f32_assignment(&mut script, "window.engine.audio.midEnv", self.mid_env);
        push_js_f32_assignment(&mut script, "window.engine.audio.treble", self.treble);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.trebleEnv",
            self.treble_env,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.density", self.density);
        push_js_f32_assignment(&mut script, "window.engine.audio.momentum", self.momentum);
        push_js_f32_assignment(&mut script, "window.engine.audio.swell", self.swell);
        push_js_f32_assignment(&mut script, "window.engine.audio.width", self.width);
        push_js_f32_assignment(&mut script, "window.engine.audio.bpm", self.bpm);
        push_js_f32_assignment(&mut script, "window.engine.audio.tempo", self.tempo);
        push_js_bool_assignment(&mut script, "window.engine.audio.beat", self.beat);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.beatPulse",
            self.beat_pulse,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.beatPhase",
            self.beat_phase,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.beatConfidence",
            self.beat_confidence,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.confidence",
            self.beat_confidence,
        );
        push_js_bool_assignment(&mut script, "window.engine.audio.onset", self.onset);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.onsetPulse",
            self.onset_pulse,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.spectralFlux",
            self.spectral_flux,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.spectralFluxBands",
            "Float32Array",
            &self.spectral_flux_bands_csv,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.brightness",
            self.brightness,
        );
        push_js_f32_assignment(&mut script, "window.engine.audio.spread", self.spread);
        push_js_f32_assignment(&mut script, "window.engine.audio.rolloff", self.rolloff);
        push_js_f32_assignment(&mut script, "window.engine.audio.roughness", self.roughness);
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.harmonicHue",
            self.harmonic_hue,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.chordMood",
            self.chord_mood,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.dominantPitch",
            self.dominant_pitch,
        );
        push_js_f32_assignment(
            &mut script,
            "window.engine.audio.dominantPitchConfidence",
            self.dominant_pitch_confidence,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.freq",
            "Int8Array",
            &self.frequency_raw_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.frequencyRaw",
            "Int8Array",
            &self.frequency_raw_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.frequency",
            "Float32Array",
            &self.spectrum_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.frequencyWeighted",
            "Float32Array",
            &self.frequency_weighted_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.melBands",
            "Float32Array",
            &self.mel_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.melBandsNormalized",
            "Float32Array",
            &self.mel_norm_csv,
        );
        push_js_csv_typed_array_assignment(
            &mut script,
            "window.engine.audio.chromagram",
            "Float32Array",
            &self.chroma_csv,
        );
        script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n");
        script.push_str("})();");
        script
    }
}

pub(super) struct SensorScriptPayload {
    sensor_list: Option<String>,
    sensors_json: String,
    replace_sensor_map: bool,
}

impl SensorScriptPayload {
    pub(super) fn from_readings(
        readings: &[SensorReading],
        replace_sensor_map: bool,
        sensor_labels: Option<&[String]>,
    ) -> Self {
        Self {
            sensor_list: sensor_labels
                .map(|labels| serde_json::to_string(labels).unwrap_or_else(|_| "[]".to_owned())),
            sensors_json: serde_json::to_string(&sensor_payload(readings))
                .unwrap_or_else(|_| "{}".to_owned()),
            replace_sensor_map,
        }
    }

    pub(super) fn script(&self) -> String {
        let mut script = String::with_capacity(
            256_usize
                .saturating_add(self.sensor_list.as_ref().map_or(0, String::len))
                .saturating_add(self.sensors_json.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        if self.replace_sensor_map {
            script.push_str("  window.engine.sensors = ");
            script.push_str(&self.sensors_json);
            script.push_str(";\n");
        } else {
            script.push_str(
                "  if (typeof window.engine.sensors !== 'object' || window.engine.sensors === null) { window.engine.sensors = {}; }\n",
            );
            script.push_str("  Object.assign(window.engine.sensors, ");
            script.push_str(&self.sensors_json);
            script.push_str(");\n");
        }
        if let Some(sensor_list) = &self.sensor_list {
            script.push_str("  window.engine.sensorList = ");
            script.push_str(sensor_list);
            script.push_str(";\n");
        }
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }
}

pub(super) struct ScreenScriptPayload {
    grid_width: u32,
    grid_height: u32,
    hue_csv: String,
    saturation_csv: String,
    lightness_csv: String,
}

impl ScreenScriptPayload {
    pub(super) fn from_screen(screen: Option<&ScreenData>) -> Self {
        let Some(screen) = screen else {
            let sample_count = DEFAULT_ZONE_SAMPLES;
            let zero_hues = vec![0_i16; sample_count];
            let zero_channels = vec![0_i8; sample_count];
            return Self {
                grid_width: DEFAULT_ZONE_WIDTH as u32,
                grid_height: DEFAULT_ZONE_HEIGHT as u32,
                hue_csv: join_i16_csv(&zero_hues),
                saturation_csv: join_i8_csv(&zero_channels),
                lightness_csv: join_i8_csv(&zero_channels),
            };
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

        Self {
            grid_width,
            grid_height,
            hue_csv: join_i16_csv(&hue),
            saturation_csv: join_i8_csv(&saturation),
            lightness_csv: join_i8_csv(&lightness),
        }
    }

    pub(super) fn script(&self) -> String {
        let mut script = String::with_capacity(
            320_usize
                .saturating_add(self.hue_csv.len())
                .saturating_add(self.saturation_csv.len())
                .saturating_add(self.lightness_csv.len()),
        );
        script.push_str("(function(){\n");
        script.push_str(
            "  if (typeof window.engine !== 'object' || window.engine === null) { window.engine = {}; }\n",
        );
        script.push_str(
            "  if (typeof window.engine.zone !== 'object' || window.engine.zone === null) { window.engine.zone = {}; }\n",
        );
        let _ = writeln!(script, "  window.engine.zone.width = {};", self.grid_width);
        let _ = writeln!(
            script,
            "  window.engine.zone.height = {};",
            self.grid_height
        );
        let _ = writeln!(
            script,
            "  window.engine.zone.hue = new Int16Array([{}]);",
            self.hue_csv
        );
        let _ = writeln!(
            script,
            "  window.engine.zone.saturation = new Int8Array([{}]);",
            self.saturation_csv
        );
        let _ = writeln!(
            script,
            "  window.engine.zone.lightness = new Int8Array([{}]);",
            self.lightness_csv
        );
        script.push_str(
            "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.engine = window.engine; }\n",
        );
        script.push_str("})();");
        script
    }
}
