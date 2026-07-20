use std::collections::BTreeMap;

use hypercolor_types::effect::ControlValue;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::media::MediaState;
use hypercolor_types::net::NetStats;
use hypercolor_types::sensor::{SensorReading, SensorUnit};
use serde::Serialize;

use crate::input::{InteractionData, ScreenData};

use super::{
    DEFAULT_ZONE_HEIGHT, DEFAULT_ZONE_SAMPLES, DEFAULT_ZONE_WIDTH, keyboard_lookup_keys,
    mouse_lookup_keys, rgb_to_hsl,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptFramePayload {
    pub(super) timing: LightScriptTimingPayload,
    pub(super) canvas: LightScriptCanvasPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) audio: Option<LightScriptAudioPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) screen: Option<LightScriptScreenPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) sensors: Option<LightScriptSensorPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) media: Option<LightScriptMediaPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) net: Option<LightScriptNetPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) lighting: Option<LightScriptLightingPayload>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) controls: BTreeMap<String, LightScriptControlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) interaction: Option<LightScriptInteractionPayload>,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) render_host_frame: bool,
}

impl LightScriptFramePayload {
    pub(super) fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }

    #[cfg(feature = "servo")]
    pub(super) fn is_host_frame_only(&self) -> bool {
        self.render_host_frame
            && self.audio.is_none()
            && self.screen.is_none()
            && self.sensors.is_none()
            && self.media.is_none()
            && self.net.is_none()
            && self.lighting.is_none()
            && self.controls.is_empty()
            && self.interaction.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptTimingPayload {
    pub(super) time_secs: f64,
    pub(super) delta_secs: f32,
    pub(super) frame_number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct LightScriptCanvasPayload {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptAudioPayload {
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
    pub(super) spectral_flux_bands: Vec<f32>,
    pub(super) brightness: f32,
    pub(super) spread: f32,
    pub(super) rolloff: f32,
    pub(super) roughness: f32,
    pub(super) harmonic_hue: f32,
    pub(super) chord_mood: f32,
    pub(super) dominant_pitch: f32,
    pub(super) dominant_pitch_confidence: f32,
    pub(super) frequency_raw: Vec<i8>,
    pub(super) frequency: Vec<f32>,
    pub(super) frequency_weighted: Vec<f32>,
    pub(super) mel_bands: Vec<f32>,
    pub(super) mel_bands_normalized: Vec<f32>,
    pub(super) chromagram: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct LightScriptInteractionPayload {
    pub(super) keyboard: LightScriptKeyboardPayload,
    pub(super) mouse: LightScriptMousePayload,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) events: Vec<LightScriptInputEventPayload>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub(super) dropped: u32,
}

impl LightScriptInteractionPayload {
    pub(super) fn from_interaction(interaction: &InteractionData, delta_secs: f32) -> Self {
        let motion_per_sec = if delta_secs > f32::EPSILON {
            interaction.batch.motion.distance / delta_secs
        } else {
            0.0
        };
        Self {
            keyboard: LightScriptKeyboardPayload {
                keys: keyboard_lookup_keys(&interaction.keyboard.pressed_keys),
                recent: interaction.keyboard.recent_keys.clone(),
            },
            mouse: LightScriptMousePayload {
                x: interaction.mouse.x,
                y: interaction.mouse.y,
                down: interaction.mouse.down,
                buttons: mouse_lookup_keys(&interaction.mouse.buttons),
                nx: sanitize_norm(interaction.mouse.norm_x),
                ny: sanitize_norm(interaction.mouse.norm_y),
                mode: pointer_mode_name(interaction.mouse.mode),
                wheel: interaction.batch.wheel_hi_res,
                velocity: if motion_per_sec.is_finite() {
                    motion_per_sec
                } else {
                    0.0
                },
            },
            events: interaction
                .batch
                .events
                .iter()
                .filter_map(LightScriptInputEventPayload::from_timed)
                .collect(),
            dropped: interaction.batch.dropped_events,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct LightScriptKeyboardPayload {
    pub(super) keys: Vec<String>,
    pub(super) recent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct LightScriptMousePayload {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) down: bool,
    pub(super) buttons: Vec<String>,
    pub(super) nx: f32,
    pub(super) ny: f32,
    pub(super) mode: &'static str,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub(super) wheel: i32,
    pub(super) velocity: f32,
}

/// One ordered input edge for the frame, flattened for JS ergonomics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptInputEventPayload {
    pub(super) kind: &'static str,
    pub(super) source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) button: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) delta: Option<i32>,
    pub(super) at_ms: u64,
    pub(super) seq: u64,
}

impl LightScriptInputEventPayload {
    fn from_timed(timed: &hypercolor_types::event::TimedInputEvent) -> Option<Self> {
        use hypercolor_types::event::InputEvent;

        let (kind, source, key, button, state, delta) = match &timed.event {
            InputEvent::Key {
                source_id,
                key,
                state,
            } => (
                "key",
                source_id.clone(),
                Some(key.clone()),
                None,
                Some(button_state_name(*state)),
                None,
            ),
            InputEvent::MouseButton {
                source_id,
                button,
                state,
            } => (
                "button",
                source_id.clone(),
                None,
                Some(button.clone()),
                Some(button_state_name(*state)),
                None,
            ),
            InputEvent::MouseWheel {
                source_id,
                delta_hi_res,
            } => (
                "wheel",
                source_id.clone(),
                None,
                None,
                None,
                Some(*delta_hi_res),
            ),
            // MIDI edges stay on the event bus; they are not part of the
            // effect-facing interaction contract yet.
            InputEvent::MidiNote { .. }
            | InputEvent::MidiControlChange { .. }
            | InputEvent::MidiPitchBend { .. }
            | InputEvent::MidiRealtime { .. } => return None,
        };

        Some(Self {
            kind,
            source,
            key,
            button,
            state,
            delta,
            at_ms: timed.at_ms,
            seq: timed.seq,
        })
    }
}

fn button_state_name(state: hypercolor_types::event::InputButtonState) -> &'static str {
    match state {
        hypercolor_types::event::InputButtonState::Pressed => "pressed",
        hypercolor_types::event::InputButtonState::Released => "released",
        hypercolor_types::event::InputButtonState::Repeated => "repeated",
    }
}

fn pointer_mode_name(mode: crate::input::PointerMode) -> &'static str {
    match mode {
        crate::input::PointerMode::None => "none",
        crate::input::PointerMode::Absolute => "absolute",
        crate::input::PointerMode::Virtual => "virtual",
    }
}

fn sanitize_norm(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// Album-art delta for one media payload: omitted from the JSON entirely
/// while the track is unchanged, `null` to clear stale art on a track
/// change without artwork, a data URL otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArtUpdate {
    Unchanged,
    Cleared,
    Set(String),
}

impl ArtUpdate {
    pub(super) fn from_state(state: &MediaState, include_art: bool) -> Self {
        if !include_art {
            return Self::Unchanged;
        }
        match state.art_data_url.as_ref() {
            Some(url) => Self::Set(url.clone()),
            None => Self::Cleared,
        }
    }

    fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
}

impl Serialize for ArtUpdate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unchanged | Self::Cleared => serializer.serialize_none(),
            Self::Set(url) => serializer.serialize_str(url),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptMediaPayload {
    pub(super) available: bool,
    pub(super) playing: bool,
    pub(super) track: String,
    pub(super) artist: String,
    pub(super) album: String,
    #[serde(skip_serializing_if = "ArtUpdate::is_unchanged")]
    pub(super) art_data_url: ArtUpdate,
    pub(super) position_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) player: String,
}

impl LightScriptMediaPayload {
    pub(super) fn from_state(state: &MediaState, include_art: bool) -> Self {
        Self {
            available: state.available,
            playing: state.playing,
            track: state.track.clone(),
            artist: state.artist.clone(),
            album: state.album.clone(),
            art_data_url: ArtUpdate::from_state(state, include_art),
            position_ms: state.position_ms,
            duration_ms: state.duration_ms,
            player: state.player.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptNetPayload {
    pub(super) rx_bps: u64,
    pub(super) tx_bps: u64,
    pub(super) iface: String,
}

impl LightScriptNetPayload {
    pub(super) fn from_stats(stats: &NetStats) -> Self {
        Self {
            rx_bps: stats.rx_bps,
            tx_bps: stats.tx_bps,
            iface: stats.iface.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptLightingPayload {
    pub(super) scene_name: Option<String>,
    pub(super) effect_names: Vec<String>,
    /// Hex `#rrggbb` strings, ready for canvas fill styles.
    pub(super) dominant_colors: Vec<String>,
}

impl LightScriptLightingPayload {
    pub(super) fn from_state(state: &LightingState) -> Self {
        Self {
            scene_name: state.scene_name.clone(),
            effect_names: state.effect_names.clone(),
            dominant_colors: state
                .dominant_colors
                .iter()
                .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptSensorPayload {
    pub(super) readings: BTreeMap<String, LightScriptSensorReading>,
    pub(super) replace_sensor_map: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) sensor_list: Option<Vec<String>>,
}

impl LightScriptSensorPayload {
    pub(super) fn from_readings(
        readings: &[SensorReading],
        replace_sensor_map: bool,
        sensor_labels: Option<&[String]>,
    ) -> Self {
        let mut payload_readings = BTreeMap::new();
        for reading in readings {
            let (default_min, default_max) = default_sensor_range(reading);
            payload_readings.insert(
                reading.label.clone(),
                LightScriptSensorReading {
                    value: sanitize_f32(reading.value),
                    min: sanitize_f32(reading.min.unwrap_or(default_min)),
                    max: sanitize_f32(reading.max.or(reading.critical).unwrap_or(default_max)),
                    unit: reading.unit.symbol(),
                },
            );
        }

        Self {
            readings: payload_readings,
            replace_sensor_map,
            sensor_list: sensor_labels.map(<[String]>::to_vec),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct LightScriptSensorReading {
    pub(super) value: f32,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) unit: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LightScriptScreenPayload {
    pub(super) grid_width: u32,
    pub(super) grid_height: u32,
    pub(super) hue: Vec<i16>,
    pub(super) saturation: Vec<i8>,
    pub(super) lightness: Vec<i8>,
}

impl LightScriptScreenPayload {
    pub(super) fn from_screen(screen: Option<&ScreenData>) -> Self {
        let Some(screen) = screen else {
            return Self {
                grid_width: DEFAULT_ZONE_WIDTH as u32,
                grid_height: DEFAULT_ZONE_HEIGHT as u32,
                hue: vec![0_i16; DEFAULT_ZONE_SAMPLES],
                saturation: vec![0_i8; DEFAULT_ZONE_SAMPLES],
                lightness: vec![0_i8; DEFAULT_ZONE_SAMPLES],
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
            hue,
            saturation,
            lightness,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum LightScriptControlValue {
    Float(f32),
    Integer(i32),
    Boolean(bool),
    Text(String),
    Gradient(Vec<LightScriptGradientStop>),
    Rect(LightScriptRect),
}

impl LightScriptControlValue {
    pub(super) fn from_control_value(value: &ControlValue) -> Self {
        match value {
            ControlValue::Float(value) => Self::Float(sanitize_f32(*value)),
            ControlValue::Integer(value) => Self::Integer(*value),
            ControlValue::Boolean(value) => Self::Boolean(*value),
            ControlValue::Color([red, green, blue, _alpha]) => Self::Text(format!(
                "#{:02x}{:02x}{:02x}",
                color_byte(*red),
                color_byte(*green),
                color_byte(*blue)
            )),
            ControlValue::Gradient(stops) => Self::Gradient(
                stops
                    .iter()
                    .map(|stop| LightScriptGradientStop {
                        pos: sanitize_f32(stop.position),
                        color: stop.color.map(sanitize_f32),
                    })
                    .collect(),
            ),
            ControlValue::Enum(value) | ControlValue::Text(value) => Self::Text(value.clone()),
            ControlValue::Rect(rect) => Self::Rect(LightScriptRect {
                x: sanitize_f32(rect.x),
                y: sanitize_f32(rect.y),
                width: sanitize_f32(rect.width),
                height: sanitize_f32(rect.height),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct LightScriptGradientStop {
    pub(super) pos: f32,
    pub(super) color: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct LightScriptRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

fn color_byte(value: f32) -> u8 {
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let scaled = (sanitize_f32(value).clamp(0.0, 1.0) * 255.0).round() as u16;
    u8::try_from(scaled).unwrap_or(u8::MAX)
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

pub(super) fn sanitize_f32(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

pub(super) fn sanitize_f64(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_f32_json_array(value: &serde_json::Value, expected: &[f64]) {
        let values = value.as_array().expect("value should be an array");
        assert_eq!(values.len(), expected.len());
        for (value, expected) in values.iter().zip(expected.iter().copied()) {
            let value = value.as_f64().expect("item should be a number");
            assert!((value - expected).abs() < 0.0001);
        }
    }

    #[test]
    fn frame_payload_serializes_stable_json_shape() {
        let mut controls = BTreeMap::new();
        controls.insert(
            "frontColor".to_owned(),
            LightScriptControlValue::Text("#00ffcc".to_owned()),
        );
        let payload = LightScriptFramePayload {
            timing: LightScriptTimingPayload {
                time_secs: 1.5,
                delta_secs: 0.016,
                frame_number: 42,
            },
            canvas: LightScriptCanvasPayload {
                width: 320,
                height: 200,
            },
            audio: Some(LightScriptAudioPayload {
                level_db: -12.5,
                level_linear: 0.5,
                level_short: 0.6,
                level_long: 0.4,
                raw_rms: 0.25,
                peak: 0.7,
                bass: 0.2,
                bass_env: 0.3,
                mid: 0.4,
                mid_env: 0.5,
                treble: 0.6,
                treble_env: 0.7,
                density: 0.8,
                momentum: -0.1,
                swell: 0.2,
                width: 0.5,
                bpm: 128.0,
                tempo: 128.0,
                beat: true,
                beat_pulse: 0.9,
                beat_phase: 0.25,
                beat_confidence: 0.75,
                onset: false,
                onset_pulse: 0.1,
                spectral_flux: 0.2,
                spectral_flux_bands: vec![0.1, 0.2, 0.3],
                brightness: 0.4,
                spread: 0.5,
                rolloff: 0.6,
                roughness: 0.7,
                harmonic_hue: 90.0,
                chord_mood: -0.2,
                dominant_pitch: 3.0,
                dominant_pitch_confidence: 0.8,
                frequency_raw: vec![-1, 2],
                frequency: vec![0.25, 0.5],
                frequency_weighted: vec![0.2],
                mel_bands: vec![0.3],
                mel_bands_normalized: vec![0.4],
                chromagram: vec![0.5],
            }),
            screen: Some(LightScriptScreenPayload {
                grid_width: 2,
                grid_height: 1,
                hue: vec![0, 120],
                saturation: vec![100, 0],
                lightness: vec![50, 25],
            }),
            sensors: Some(LightScriptSensorPayload {
                readings: BTreeMap::from([(
                    "gpu".to_owned(),
                    LightScriptSensorReading {
                        value: 42.0,
                        min: 0.0,
                        max: 100.0,
                        unit: "%",
                    },
                )]),
                replace_sensor_map: false,
                sensor_list: Some(vec!["gpu".to_owned()]),
            }),
            media: None,
            net: None,
            lighting: None,
            controls,
            interaction: Some(LightScriptInteractionPayload {
                keyboard: LightScriptKeyboardPayload {
                    keys: vec!["A".to_owned()],
                    recent: vec!["A".to_owned()],
                },
                mouse: LightScriptMousePayload {
                    x: -4,
                    y: 12,
                    down: true,
                    buttons: vec!["left".to_owned()],
                    nx: 0.25,
                    ny: 0.75,
                    mode: "virtual",
                    wheel: 120,
                    velocity: 0.5,
                },
                events: Vec::new(),
                dropped: 0,
            }),
            render_host_frame: true,
        };

        let value = serde_json::to_value(&payload).expect("payload should serialize");
        assert_eq!(value["timing"]["timeSecs"], serde_json::json!(1.5));
        let delta_secs = value["timing"]["deltaSecs"]
            .as_f64()
            .expect("deltaSecs should serialize as a number");
        assert!((delta_secs - 0.016).abs() < 0.0001);
        assert_eq!(value["timing"]["frameNumber"], serde_json::json!(42));
        assert_eq!(
            value["canvas"],
            serde_json::json!({ "width": 320, "height": 200 })
        );
        assert_eq!(value["audio"]["levelDb"], serde_json::json!(-12.5));
        assert_eq!(value["audio"]["levelLinear"], serde_json::json!(0.5));
        assert_f32_json_array(&value["audio"]["spectralFluxBands"], &[0.1, 0.2, 0.3]);
        assert_eq!(value["audio"]["frequencyRaw"], serde_json::json!([-1, 2]));
        assert_f32_json_array(&value["audio"]["frequency"], &[0.25, 0.5]);
        assert_f32_json_array(&value["audio"]["frequencyWeighted"], &[0.2]);
        assert_f32_json_array(&value["audio"]["melBands"], &[0.3]);
        assert_f32_json_array(&value["audio"]["melBandsNormalized"], &[0.4]);
        assert_f32_json_array(&value["audio"]["chromagram"], &[0.5]);
        assert_eq!(value["screen"]["gridWidth"], serde_json::json!(2));
        assert_eq!(value["screen"]["hue"], serde_json::json!([0, 120]));
        assert_eq!(value["screen"]["saturation"], serde_json::json!([100, 0]));
        assert_eq!(value["screen"]["lightness"], serde_json::json!([50, 25]));
        assert_eq!(
            value["sensors"]["readings"]["gpu"]["value"],
            serde_json::json!(42.0)
        );
        assert_eq!(
            value["sensors"]["replaceSensorMap"],
            serde_json::json!(false)
        );
        assert_eq!(value["sensors"]["sensorList"], serde_json::json!(["gpu"]));
        assert_eq!(
            value["controls"]["frontColor"],
            serde_json::json!("#00ffcc")
        );
        assert_eq!(
            value["interaction"]["keyboard"]["keys"],
            serde_json::json!(["A"])
        );
        assert_eq!(
            value["interaction"]["mouse"]["down"],
            serde_json::json!(true)
        );
        assert_eq!(value["renderHostFrame"], serde_json::json!(true));
    }

    #[test]
    fn control_values_serialize_as_lightscript_globals() {
        assert_eq!(
            serde_json::to_value(LightScriptControlValue::from_control_value(
                &ControlValue::Color([0.0, 0.5, 1.0, 1.0]),
            ))
            .expect("color should serialize"),
            serde_json::json!("#0080ff")
        );
        assert_eq!(
            serde_json::to_value(LightScriptControlValue::from_control_value(
                &ControlValue::Float(f32::NAN),
            ))
            .expect("float should serialize"),
            serde_json::json!(0.0)
        );
    }

    #[test]
    fn interaction_payload_uses_lookup_key_arrays() {
        let payload = LightScriptInteractionPayload::from_interaction(
            &InteractionData {
                keyboard: crate::input::KeyboardData {
                    pressed_keys: vec!["a".to_owned(), "Space".to_owned()],
                    recent_keys: vec!["a".to_owned()],
                },
                mouse: crate::input::MouseData {
                    x: 42,
                    y: 24,
                    buttons: vec!["left".to_owned()],
                    down: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            1.0 / 30.0,
        );

        assert!(payload.keyboard.keys.contains(&"A".to_owned()));
        assert!(payload.keyboard.keys.contains(&"KeyA".to_owned()));
        assert!(payload.keyboard.keys.contains(&"Spacebar".to_owned()));
        assert!(payload.mouse.buttons.contains(&"primary".to_owned()));
    }

    #[test]
    fn screen_payload_defaults_to_black_zone_grid() {
        let payload = LightScriptScreenPayload::from_screen(None);

        assert_eq!(payload.grid_width, DEFAULT_ZONE_WIDTH as u32);
        assert_eq!(payload.grid_height, DEFAULT_ZONE_HEIGHT as u32);
        assert_eq!(payload.hue, vec![0_i16; DEFAULT_ZONE_SAMPLES]);
        assert_eq!(payload.saturation, vec![0_i8; DEFAULT_ZONE_SAMPLES]);
        assert_eq!(payload.lightness, vec![0_i8; DEFAULT_ZONE_SAMPLES]);
    }
}

#[cfg(test)]
mod interaction_payload_v2_tests {
    use super::*;
    use crate::input::{InteractionData, MotionAggregate, PointerMode};
    use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent};

    #[test]
    fn interaction_payload_carries_events_wheel_and_velocity() {
        let mut interaction = InteractionData::default();
        interaction.mouse.norm_x = 0.5;
        interaction.mouse.norm_y = 2.0; // clamped
        interaction.mouse.mode = PointerMode::Virtual;
        interaction.batch.wheel_hi_res = -240;
        interaction.batch.motion = MotionAggregate {
            dx: 0.1,
            dy: 0.0,
            distance: 0.3,
        };
        interaction.batch.dropped_events = 2;
        interaction.batch.events = vec![
            TimedInputEvent {
                event: InputEvent::Key {
                    source_id: "kbd".into(),
                    key: "a".into(),
                    state: InputButtonState::Pressed,
                },
                at_ms: 100,
                seq: 9,
            },
            TimedInputEvent {
                event: InputEvent::MouseWheel {
                    source_id: "ptr".into(),
                    delta_hi_res: -240,
                },
                at_ms: 105,
                seq: 10,
            },
            TimedInputEvent {
                event: InputEvent::MidiRealtime {
                    source_id: "midi".into(),
                    message: hypercolor_types::event::MidiRealtimeMessage::Clock,
                },
                at_ms: 106,
                seq: 11,
            },
        ];

        let payload = LightScriptInteractionPayload::from_interaction(&interaction, 1.0 / 30.0);
        let value = serde_json::to_value(&payload).expect("payload serializes");

        assert_eq!(value["mouse"]["nx"], serde_json::json!(0.5));
        assert_eq!(value["mouse"]["ny"], serde_json::json!(1.0));
        assert_eq!(value["mouse"]["mode"], serde_json::json!("virtual"));
        assert_eq!(value["mouse"]["wheel"], serde_json::json!(-240));
        assert!(value["mouse"]["velocity"].as_f64().expect("velocity") > 8.9);
        assert_eq!(value["dropped"], serde_json::json!(2));

        let events = value["events"].as_array().expect("events array");
        assert_eq!(events.len(), 2, "MIDI edges stay off the effect contract");
        assert_eq!(events[0]["kind"], serde_json::json!("key"));
        assert_eq!(events[0]["key"], serde_json::json!("a"));
        assert_eq!(events[0]["state"], serde_json::json!("pressed"));
        assert_eq!(events[0]["atMs"], serde_json::json!(100));
        assert_eq!(events[0]["seq"], serde_json::json!(9));
        assert_eq!(events[1]["kind"], serde_json::json!("wheel"));
        assert_eq!(events[1]["delta"], serde_json::json!(-240));
    }

    #[test]
    fn idle_interaction_payload_omits_transient_fields() {
        let payload =
            LightScriptInteractionPayload::from_interaction(&InteractionData::default(), 0.0);
        let value = serde_json::to_value(&payload).expect("payload serializes");
        assert!(value.get("events").is_none());
        assert!(value.get("dropped").is_none());
        assert!(value["mouse"].get("wheel").is_none());
        assert_eq!(value["mouse"]["mode"], serde_json::json!("none"));
    }
}
