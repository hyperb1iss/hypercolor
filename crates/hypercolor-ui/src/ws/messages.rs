//! JSON and binary message handlers for the daemon WebSocket protocol.

pub use hypercolor_leptos_ext::ws::{
    PreviewFrameView as CanvasFrame, PreviewPixelFormat as CanvasPixelFormat,
};
pub(super) use hypercolor_leptos_ext::ws::PreviewFrameChannel;
use hypercolor_types::event::RenderGroupChangeKind;
use hypercolor_types::scene::{RenderGroupRole, SceneKind, SceneMutationMode};
use leptos::prelude::*;
use serde::Deserialize;

use crate::api::DeviceMetricsSnapshot;

// ── Connection State ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Error => write!(f, "Error"),
        }
    }
}

pub const EFFECT_STARTED_EVENTS: &[&str] =
    &["effect_started", "effect_activated", "effect_changed"];
pub const EFFECT_STOPPED_EVENTS: &[&str] = &["effect_stopped", "effect_deactivated"];
pub const EFFECT_ERROR_EVENTS: &[&str] = &["effect_error"];
pub const SCENE_EVENTS: &[&str] = &["active_scene_changed", "render_group_changed"];
pub const DEVICE_LIFECYCLE_EVENTS: &[&str] = &[
    "device_connected",
    "device_discovered",
    "device_disconnected",
    "device_state_changed",
    "device_discovery_completed",
];

// ── Metrics & Event Types ───────────────────────────────────────────────────

/// Live performance metrics streamed from the daemon.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PerformanceMetrics {
    pub fps: MetricsFps,
    pub frame_time: MetricsFrameTime,
    pub stages: MetricsStages,
    pub pacing: MetricsPacing,
    pub timeline: MetricsTimeline,
    pub memory: MetricsMemory,
    pub devices: MetricsDevices,
    pub websocket: MetricsWebsocket,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsFps {
    pub target: u32,
    pub actual: f64,
    pub dropped: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsFrameTime {
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsStages {
    pub input_sampling_ms: f64,
    pub producer_rendering_ms: f64,
    pub composition_ms: f64,
    pub effect_rendering_ms: f64,
    pub spatial_sampling_ms: f64,
    pub device_output_ms: f64,
    pub preview_postprocess_ms: f64,
    pub event_bus_ms: f64,
    pub coordination_overhead_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsPacing {
    pub jitter_avg_ms: f64,
    pub jitter_p95_ms: f64,
    pub jitter_max_ms: f64,
    pub wake_delay_avg_ms: f64,
    pub wake_delay_p95_ms: f64,
    pub wake_delay_max_ms: f64,
    pub frame_age_ms: f64,
    pub reused_inputs: u32,
    pub reused_canvas: u32,
    pub retained_effect: u32,
    pub retained_screen: u32,
    pub composition_bypassed: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsTimeline {
    pub frame_token: u64,
    pub compositor_backend: String,
    pub budget_ms: f64,
    pub wake_late_ms: f64,
    pub logical_layer_count: u32,
    pub render_group_count: u32,
    pub scene_active: bool,
    pub scene_transition_active: bool,
    pub scene_snapshot_done_ms: f64,
    pub input_done_ms: f64,
    pub producer_done_ms: f64,
    pub composition_done_ms: f64,
    pub sampling_done_ms: f64,
    pub output_done_ms: f64,
    pub publish_done_ms: f64,
    pub frame_done_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsMemory {
    pub daemon_rss_mb: f64,
    pub servo_rss_mb: f64,
    pub canvas_buffer_kb: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsDevices {
    pub connected: usize,
    pub total_leds: usize,
    pub output_errors: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsWebsocket {
    pub client_count: usize,
    pub bytes_sent_per_sec: f64,
}

/// Latest backpressure notice from the daemon for preview/frame streams.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackpressureNotice {
    pub dropped_frames: u32,
    pub channel: String,
    pub recommendation: String,
    pub suggested_fps: u32,
}

/// Lightweight device event hint used to decide whether the devices list
/// actually needs a refetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEventHint {
    pub event_type: String,
    pub device_id: Option<String>,
    pub found_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneEventHint {
    pub event_type: String,
    pub scene_id: Option<String>,
    pub scene_name: Option<String>,
    pub scene_kind: Option<SceneKind>,
    pub scene_mutation_mode: Option<SceneMutationMode>,
    pub scene_snapshot_locked: Option<bool>,
    pub render_group_role: Option<RenderGroupRole>,
    pub render_group_change_kind: Option<RenderGroupChangeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectErrorHint {
    pub event_type: String,
    pub effect_id: String,
    pub error: String,
    pub fallback: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MetricsMessage {
    data: PerformanceMetrics,
}

#[derive(Debug, Deserialize)]
struct DeviceMetricsMessage {
    data: DeviceMetricsSnapshot,
}

#[derive(Debug, Deserialize)]
struct BackpressureMessage {
    dropped_frames: u32,
    channel: String,
    recommendation: String,
    suggested_fps: u32,
}

// ── Audio Level ─────────────────────────────────────────────────────────────

/// Live audio levels from `AudioLevelUpdate` events (~10 Hz).
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioLevel {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat: bool,
}

// ── Binary Frame Decoder ────────────────────────────────────────────────────

pub(super) fn decode_preview_frame(
    buffer: js_sys::ArrayBuffer,
) -> Option<(PreviewFrameChannel, CanvasFrame)> {
    let frame = CanvasFrame::decode_array_buffer(&buffer).ok()?;
    Some((frame.channel, frame))
}

// ── JSON Message Handler ────────────────────────────────────────────────────

/// Handle incoming JSON events from the daemon.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_json_message(
    msg: &serde_json::Value,
    set_active: &WriteSignal<Option<String>>,
    metrics: ReadSignal<Option<PerformanceMetrics>>,
    set_metrics: &WriteSignal<Option<PerformanceMetrics>>,
    set_device_metrics: &WriteSignal<Option<DeviceMetricsSnapshot>>,
    backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    set_backpressure_notice: &WriteSignal<Option<BackpressureNotice>>,
    set_last_device_event: &WriteSignal<Option<DeviceEventHint>>,
    set_last_scene_event: &WriteSignal<Option<SceneEventHint>>,
    set_last_effect_error: &WriteSignal<Option<EffectErrorHint>>,
    set_audio_level: &WriteSignal<AudioLevel>,
    set_engine_preview_target: &WriteSignal<u32>,
    set_preview_target_fps: &WriteSignal<u32>,
    set_preview_transport_cap: &WriteSignal<u32>,
    set_last_backpressure_at_ms: &WriteSignal<Option<f64>>,
    set_backpressure_probe_epoch: &WriteSignal<u64>,
) {
    let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "hello" => {
            // Extract active effect from hello state
            if let Some(state) = msg.get("state") {
                set_active.set(extract_active_effect_name(state));

                let target = state
                    .get("fps")
                    .and_then(|fps| fps.get("target"))
                    .and_then(|target| target.as_u64())
                    .and_then(|target| u32::try_from(target).ok())
                    .unwrap_or_default();
                let actual = state
                    .get("fps")
                    .and_then(|fps| fps.get("actual"))
                    .and_then(|actual| actual.as_f64())
                    .unwrap_or_default();

                if target > 0 || actual > 0.0 {
                    set_metrics.update(|metrics| {
                        let mut next = metrics.clone().unwrap_or_default();
                        next.fps.target = target;
                        next.fps.actual = actual;
                        *metrics = Some(next);
                    });
                }

                if target > 0 {
                    set_engine_preview_target.set(target.min(60));
                }
            }
        }
        "metrics" => {
            if let Ok(message) = serde_json::from_value::<MetricsMessage>(msg.clone()) {
                set_backpressure_probe_epoch.update(|epoch| *epoch = epoch.saturating_add(1));
                if message.data.fps.target > 0 {
                    set_engine_preview_target.set(message.data.fps.target.min(60));
                }
                // Gate on equality — skip notification when data hasn't changed
                if metrics.get_untracked().as_ref() != Some(&message.data) {
                    set_metrics.set(Some(message.data));
                }
            }
        }
        "device_metrics" => {
            if let Ok(message) = serde_json::from_value::<DeviceMetricsMessage>(msg.clone()) {
                set_device_metrics.set(Some(message.data));
            }
        }
        "subscribed" => {
            let preview_target = msg
                .get("config")
                .and_then(|config| config.get("canvas"))
                .and_then(|canvas| canvas.get("fps"))
                .and_then(|fps| fps.as_u64())
                .and_then(|fps| u32::try_from(fps).ok())
                .unwrap_or_default();
            if preview_target > 0 {
                set_preview_target_fps.set(preview_target.min(60));
            }
        }
        "backpressure" => {
            if let Ok(message) = serde_json::from_value::<BackpressureMessage>(msg.clone()) {
                if message.channel == "canvas"
                    && message.recommendation == "reduce_fps"
                    && message.suggested_fps > 0
                {
                    set_preview_transport_cap
                        .update(|current| *current = (*current).min(message.suggested_fps));
                    set_last_backpressure_at_ms.set(Some(js_sys::Date::now()));
                }
                let notice = BackpressureNotice {
                    dropped_frames: message.dropped_frames,
                    channel: message.channel,
                    recommendation: message.recommendation,
                    suggested_fps: message.suggested_fps,
                };
                if backpressure_notice.get_untracked().as_ref() != Some(&notice) {
                    set_backpressure_notice.set(Some(notice));
                }
            }
        }
        "event" => {
            if let Some(event_type) = msg.get("event").and_then(|e| e.as_str()) {
                if EFFECT_STARTED_EVENTS.contains(&event_type) {
                    set_active.set(extract_effect_name_from_event(
                        msg.get("data").unwrap_or(&serde_json::Value::Null),
                    ));
                } else if EFFECT_STOPPED_EVENTS.contains(&event_type) {
                    set_active.set(None);
                } else if event_type == "audio_level_update" {
                    if let Some(data) = msg.get("data") {
                        let f = |key| data.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        set_audio_level.set(AudioLevel {
                            level: f("level"),
                            bass: f("bass"),
                            mid: f("mid"),
                            treble: f("treble"),
                            beat: data.get("beat").and_then(|v| v.as_bool()).unwrap_or(false),
                        });
                    }
                } else if SCENE_EVENTS.contains(&event_type) {
                    let scene_data = msg.get("data").unwrap_or(&serde_json::Value::Null);
                    set_last_scene_event
                        .set(Some(extract_scene_event_hint(event_type, scene_data)));
                } else if EFFECT_ERROR_EVENTS.contains(&event_type) {
                    let effect_data = msg.get("data").unwrap_or(&serde_json::Value::Null);
                    set_last_effect_error.set(extract_effect_error_hint(event_type, effect_data));
                } else if DEVICE_LIFECYCLE_EVENTS.contains(&event_type)
                    && let Some(hint) = extract_device_event_hint(event_type, msg.get("data"))
                {
                    set_last_device_event.set(Some(hint));
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn extract_effect_error_hint(
    event_type: &str,
    effect_data: &serde_json::Value,
) -> Option<EffectErrorHint> {
    let effect_id = effect_data
        .get("effect_id")
        .or_else(|| effect_data.get("id"))
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let error = effect_data
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let fallback = effect_data
        .get("fallback")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);

    Some(EffectErrorHint {
        event_type: event_type.to_owned(),
        effect_id,
        error,
        fallback,
    })
}

pub(crate) fn extract_scene_event_hint(
    event_type: &str,
    scene_data: &serde_json::Value,
) -> SceneEventHint {
    SceneEventHint {
        event_type: event_type.to_owned(),
        scene_id: scene_data
            .get("current")
            .or_else(|| scene_data.get("scene_id"))
            .or_else(|| scene_data.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        scene_name: scene_data
            .get("current_name")
            .or_else(|| scene_data.get("scene_name"))
            .or_else(|| scene_data.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        scene_kind: scene_data
            .get("current_kind")
            .or_else(|| scene_data.get("kind"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        scene_mutation_mode: scene_data
            .get("current_mutation_mode")
            .or_else(|| scene_data.get("mutation_mode"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        scene_snapshot_locked: scene_data
            .get("current_snapshot_locked")
            .or_else(|| scene_data.get("snapshot_locked"))
            .and_then(serde_json::Value::as_bool),
        render_group_role: scene_data
            .get("role")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        render_group_change_kind: scene_data
            .get("kind")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    }
}

pub(crate) fn scene_event_affects_active_effect(hint: &SceneEventHint) -> bool {
    hint.event_type != "render_group_changed"
        || hint.render_group_role != Some(RenderGroupRole::Display)
}

fn extract_active_effect_name(state: &serde_json::Value) -> Option<String> {
    let active = state.get("effect").or_else(|| state.get("active_effect"))?;
    active
        .get("name")
        .or_else(|| active.get("effect_name"))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .or_else(|| active.as_str().map(String::from))
}

fn extract_effect_name_from_event(data: &serde_json::Value) -> Option<String> {
    data.get("name")
        .or_else(|| data.get("effect_name"))
        .or_else(|| data.get("effect").and_then(|effect| effect.get("name")))
        .or_else(|| data.get("current").and_then(|effect| effect.get("name")))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .or_else(|| {
            data.get("effect")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
}

fn extract_device_event_hint(
    event_type: &str,
    data: Option<&serde_json::Value>,
) -> Option<DeviceEventHint> {
    let data = data.unwrap_or(&serde_json::Value::Null);
    let device_id = data
        .get("device_id")
        .or_else(|| data.get("id"))
        .or_else(|| data.get("device").and_then(|device| device.get("id")))
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let found_count = data.get("found").and_then(|found| {
        found
            .as_array()
            .map(std::vec::Vec::len)
            .or_else(|| found.as_u64().and_then(|count| usize::try_from(count).ok()))
    });

    Some(DeviceEventHint {
        event_type: event_type.to_owned(),
        device_id,
        found_count,
    })
}
