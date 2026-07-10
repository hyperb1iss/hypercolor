//! JSON and binary message handlers for the daemon WebSocket protocol.

use std::collections::HashMap;

use hypercolor_leptos_ext::prelude::now_ms;
pub(super) use hypercolor_leptos_ext::ws::PreviewFrameChannel;
pub use hypercolor_leptos_ext::ws::ScreenZonesFrame;
pub use hypercolor_leptos_ext::ws::{
    PreviewFrameView as CanvasFrame, PreviewPixelFormat as CanvasPixelFormat,
};
use hypercolor_types::event::{LayerHealth, SceneLibraryChangeKind, ZoneChangeKind};
use hypercolor_types::scene::{SceneKind, SceneMutationMode, ZoneRole};
use hypercolor_types::sensor::SystemSnapshot;
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
pub const SCENE_EVENTS: &[&str] = &[
    "active_scene_changed",
    "render_group_changed",
    "scene_library_changed",
    "scene_settings_changed",
];
pub const CONTROL_SURFACE_EVENTS: &[&str] = &["control_surface_changed"];
pub const DEVICE_LIFECYCLE_EVENTS: &[&str] = &[
    "device_connected",
    "device_discovered",
    "device_disconnected",
    "device_state_changed",
    "device_discovery_completed",
];
pub const LAYER_HEALTH_EVENTS: &[&str] = &["layer_health_changed"];

// ── Metrics & Event Types ───────────────────────────────────────────────────

/// Live performance metrics streamed from the daemon.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PerformanceMetrics {
    pub fps: MetricsFps,
    pub frame_time: MetricsFrameTime,
    pub stages: MetricsStages,
    pub pacing: MetricsPacing,
    pub effect_health: MetricsEffectHealth,
    pub timeline: MetricsTimeline,
    pub render_surfaces: MetricsRenderSurfaces,
    pub preview: MetricsPreview,
    pub display_output: MetricsDisplayOutput,
    pub copies: MetricsCopies,
    pub memory: MetricsMemory,
    pub devices: MetricsDevices,
    pub websocket: MetricsWebsocket,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsFps {
    pub target: u32,
    pub ceiling: u32,
    pub capacity: f64,
    pub delivered: Option<f64>,
    pub actual: f64,
    pub dropped: u32,
}

impl MetricsFps {
    #[must_use]
    pub fn delivered_or_legacy(&self) -> f64 {
        self.delivered.unwrap_or(self.actual)
    }
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
    pub producer_effect_rendering_ms: f64,
    #[serde(rename = "producer_preview_compose_ms")]
    pub producer_scene_compose_ms: f64,
    pub composition_ms: f64,
    pub effect_rendering_ms: f64,
    pub spatial_sampling_ms: f64,
    pub device_output_ms: f64,
    pub preview_postprocess_ms: f64,
    pub event_bus_ms: f64,
    pub publish_frame_data_ms: f64,
    pub publish_group_canvas_ms: f64,
    pub publish_preview_ms: f64,
    pub publish_events_ms: f64,
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
    pub gpu_zone_sampling: u32,
    pub gpu_sample_deferred: u32,
    pub gpu_sample_stale: u32,
    pub gpu_sample_retry_hit: u32,
    pub gpu_sample_queue_saturated: u32,
    pub gpu_sample_wait_blocked: u32,
    pub gpu_sample_cpu_fallback: u32,
    pub cpu_sampling_late_readback: u32,
    pub led_sampling_readback: u32,
    pub preview_surface: u32,
    pub scene_canvas_forced_surface: u32,
    pub gpu_readback_failed_frames: u32,
    pub output_error_frames: u32,
    pub full_frame_copy_frames: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsEffectHealth {
    pub errors_total: u64,
    pub fallbacks_applied_total: u64,
    pub producer_gpu_readback_failures_total: u64,
    pub servo_soft_stalls_total: u64,
    pub servo_breaker_opens_total: u64,
    pub servo_session_creates_total: u64,
    pub servo_session_create_failures_total: u64,
    pub servo_session_create_wait_total_ms: f64,
    pub servo_session_create_wait_max_ms: f64,
    pub servo_page_loads_total: u64,
    pub servo_page_load_failures_total: u64,
    pub servo_page_load_wait_total_ms: f64,
    pub servo_page_load_wait_max_ms: f64,
    pub servo_renderer_loads_total: u64,
    pub servo_renderer_load_failures_total: u64,
    pub servo_renderer_load_wait_total_ms: f64,
    pub servo_renderer_load_wait_max_ms: f64,
    pub servo_detached_destroys_total: u64,
    pub servo_detached_destroy_failures_total: u64,
    pub servo_destroy_wait_total_ms: f64,
    pub servo_destroy_wait_max_ms: f64,
    pub servo_render_requests_total: u64,
    pub servo_render_queue_wait_total_ms: f64,
    pub servo_render_queue_wait_max_ms: f64,
    pub servo_render_queue_depth: u64,
    pub servo_render_queue_depth_max: u64,
    pub servo_render_superseded_total: u64,
    pub servo_render_pending_age_max_ms: f64,
    pub servo_render_cpu_frames_total: u64,
    pub servo_render_cached_frames_total: u64,
    pub servo_render_gpu_frames_total: u64,
    pub servo_gpu_import_failures_total: u64,
    pub servo_gpu_import_fallbacks_total: u64,
    pub servo_gpu_import_fallback_reason: Option<String>,
    pub servo_gpu_import_windows_sync_mode: Option<String>,
    pub servo_gpu_import_stale_frame_total: u64,
    pub servo_gpu_import_adapter_mismatch_total: u64,
    pub servo_gpu_import_blit_total_ms: f64,
    pub servo_gpu_import_blit_max_ms: f64,
    pub servo_gpu_import_sync_total_ms: f64,
    pub servo_gpu_import_sync_max_ms: f64,
    pub servo_gpu_import_total_ms: f64,
    pub servo_gpu_import_max_ms: f64,
    pub producer_cpu_frames_total: u64,
    pub producer_gpu_frames_total: u64,
    pub sparkleflinger_gpu_source_upload_skipped_total: u64,
    pub servo_render_evaluate_scripts_total_ms: f64,
    pub servo_render_evaluate_scripts_max_ms: f64,
    pub servo_render_event_loop_total_ms: f64,
    pub servo_render_event_loop_max_ms: f64,
    pub servo_render_paint_total_ms: f64,
    pub servo_render_paint_max_ms: f64,
    pub servo_render_readback_total_ms: f64,
    pub servo_render_readback_max_ms: f64,
    pub servo_render_frame_total_ms: f64,
    pub servo_render_frame_max_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsTimeline {
    pub frame_token: u64,
    pub compositor_backend: String,
    pub gpu_zone_sampling: bool,
    pub gpu_sample_deferred: bool,
    pub gpu_sample_stale: bool,
    pub gpu_sample_retry_hit: bool,
    pub gpu_sample_queue_saturated: bool,
    pub gpu_sample_wait_blocked: bool,
    pub gpu_sample_cpu_fallback: bool,
    pub cpu_sampling_late_readback: bool,
    pub led_sampling_readback: bool,
    pub preview_surface: bool,
    pub scene_canvas_forced_surface: bool,
    pub cpu_readback_skipped: bool,
    pub gpu_readback_failed: bool,
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
pub struct MetricsRenderSurfaces {
    pub slot_count: u32,
    pub free_slots: u32,
    pub published_slots: u32,
    pub dequeued_slots: u32,
    pub canvas_receivers: u32,
    #[serde(rename = "preview_pool_saturation_reallocs")]
    pub scene_pool_saturation_reallocs: u64,
    pub direct_pool_saturation_reallocs: u64,
    #[serde(rename = "preview_pool_grown_slots")]
    pub scene_pool_grown_slots: u32,
    pub direct_pool_grown_slots: u32,
    pub scene_pool_slot_count: u32,
    pub scene_pool_max_slots: u32,
    pub direct_pool_slot_count: u32,
    pub direct_pool_max_slots: u32,
    pub scene_pool_shared_published_slots: u32,
    pub scene_pool_max_ref_count: u32,
    pub direct_pool_shared_published_slots: u32,
    pub direct_pool_max_ref_count: u32,
    pub scene_pool_free_slots: u32,
    pub scene_pool_published_slots: u32,
    pub scene_pool_dequeued_slots: u32,
    pub direct_pool_free_slots: u32,
    pub direct_pool_published_slots: u32,
    pub direct_pool_dequeued_slots: u32,
    pub preview_pool_slot_count: u32,
    pub preview_pool_free_slots: u32,
    pub preview_pool_published_slots: u32,
    pub preview_pool_dequeued_slots: u32,
    pub compositor_pool_slot_count: u32,
    pub compositor_pool_free_slots: u32,
    pub compositor_pool_published_slots: u32,
    pub compositor_pool_dequeued_slots: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsPreview {
    pub canvas_receivers: u32,
    pub scene_canvas_receivers: u32,
    pub screen_canvas_receivers: u32,
    pub web_viewport_canvas_receivers: u32,
    pub zone_preview_receivers: u32,
    pub canvas_frames_published: u64,
    pub scene_canvas_frames_published: u64,
    pub screen_canvas_frames_published: u64,
    pub web_viewport_canvas_frames_published: u64,
    pub zone_preview_frames_published: u64,
    pub latest_canvas_frame_number: u32,
    pub latest_scene_canvas_frame_number: u32,
    pub latest_screen_canvas_frame_number: u32,
    pub latest_web_viewport_canvas_frame_number: u32,
    pub latest_zone_preview_frame_number: u32,
    pub canvas_demand: MetricsPreviewDemand,
    pub scene_canvas_demand: MetricsPreviewDemand,
    pub screen_canvas_demand: MetricsPreviewDemand,
    pub web_viewport_canvas_demand: MetricsPreviewDemand,
    pub zone_preview_demand: MetricsPreviewDemand,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MetricsPreviewDemand {
    pub subscribers: u32,
    pub max_fps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub any_full_resolution: bool,
    pub any_rgb: bool,
    pub any_rgba: bool,
    pub any_jpeg: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsDisplayOutput {
    pub captured_devices: usize,
    pub preview_subscribers: usize,
    pub write_attempts_total: u64,
    pub write_successes_total: u64,
    pub write_failures_total: u64,
    pub retry_attempts_total: u64,
    pub display_lane: MetricsDisplayLane,
    pub last_failure_age_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsDisplayLane {
    pub display_frames_total: u64,
    pub display_frames_delayed_for_led_total: u64,
    pub display_led_priority_wait_total_ms: f64,
    pub display_led_priority_wait_max_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsCopies {
    pub full_frame_count: u32,
    pub full_frame_kb: f64,
    pub producer_full_frame_count: u32,
    pub producer_full_frame_kb: f64,
    pub producer_reason: Option<String>,
    pub publication_full_frame_count: u32,
    pub publication_full_frame_kb: f64,
    pub publication_reason: Option<String>,
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
    /// Zone (render group) the event names, for zone-tagged events like
    /// `render_group_changed` and `layer_stack_changed`.
    pub group_id: Option<String>,
    pub scene_name: Option<String>,
    pub scene_kind: Option<SceneKind>,
    pub scene_mutation_mode: Option<SceneMutationMode>,
    pub scene_snapshot_locked: Option<bool>,
    pub render_group_role: Option<ZoneRole>,
    pub render_group_change_kind: Option<ZoneChangeKind>,
    /// How the saved-scene library changed, for `scene_library_changed`.
    pub library_change_kind: Option<SceneLibraryChangeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectErrorHint {
    pub event_type: String,
    pub effect_id: String,
    pub error: String,
    pub fallback: Option<String>,
}

/// Hint from an `extension_state_changed` event: a daemon extension's owned
/// state changed. UI extensions filter on `source`/`kind` and refresh the
/// matching REST resources — the push replaces any need to poll.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionEventHint {
    pub source: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlSurfaceEventHint {
    pub event_type: String,
    pub kind: String,
    pub surface_id: String,
    pub revision: Option<u64>,
    pub action_id: Option<String>,
    pub status: Option<String>,
    pub progress: Option<f32>,
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
struct SensorsMessage {
    data: SystemSnapshot,
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

pub(super) fn decode_screen_zones_frame(buffer: &js_sys::ArrayBuffer) -> Option<ScreenZonesFrame> {
    ScreenZonesFrame::decode_array_buffer(buffer).ok()
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
    set_sensors: &WriteSignal<Option<SystemSnapshot>>,
    backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    set_backpressure_notice: &WriteSignal<Option<BackpressureNotice>>,
    set_last_device_event: &WriteSignal<Option<DeviceEventHint>>,
    set_last_scene_event: &WriteSignal<Option<SceneEventHint>>,
    set_last_effect_error: &WriteSignal<Option<EffectErrorHint>>,
    set_last_control_surface_event: &WriteSignal<Option<ControlSurfaceEventHint>>,
    set_last_extension_event: &WriteSignal<Option<ExtensionEventHint>>,
    set_layer_health: &WriteSignal<HashMap<String, LayerHealth>>,
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
                let capacity = state
                    .get("fps")
                    .and_then(|fps| fps.get("capacity"))
                    .and_then(|capacity| capacity.as_f64())
                    .unwrap_or(actual);
                let delivered = state
                    .get("fps")
                    .and_then(|fps| fps.get("delivered"))
                    .and_then(|delivered| delivered.as_f64());

                if target > 0 || actual > 0.0 {
                    set_metrics.update(|metrics| {
                        let mut next = metrics.clone().unwrap_or_default();
                        next.fps.target = target;
                        next.fps.capacity = capacity;
                        next.fps.delivered = delivered;
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
            if let Ok(message) = MetricsMessage::deserialize(msg) {
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
            if let Ok(message) = DeviceMetricsMessage::deserialize(msg) {
                set_device_metrics.set(Some(message.data));
            }
        }
        "sensors" => {
            if let Ok(message) = SensorsMessage::deserialize(msg) {
                set_sensors.set(Some(message.data));
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
            if let Ok(message) = BackpressureMessage::deserialize(msg) {
                if message.channel == "canvas"
                    && message.recommendation == "reduce_fps"
                    && message.suggested_fps > 0
                {
                    set_preview_transport_cap
                        .update(|current| *current = (*current).min(message.suggested_fps));
                    set_last_backpressure_at_ms.set(Some(now_ms()));
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
                } else if CONTROL_SURFACE_EVENTS.contains(&event_type) {
                    let data = msg.get("data").unwrap_or(&serde_json::Value::Null);
                    set_last_control_surface_event
                        .set(extract_control_surface_event_hint(event_type, data));
                } else if event_type == "extension_state_changed" {
                    let data = msg.get("data").unwrap_or(&serde_json::Value::Null);
                    if let (Some(source), Some(kind)) = (
                        data.get("source").and_then(serde_json::Value::as_str),
                        data.get("kind").and_then(serde_json::Value::as_str),
                    ) {
                        set_last_extension_event.set(Some(ExtensionEventHint {
                            source: source.to_owned(),
                            kind: kind.to_owned(),
                            payload: data
                                .get("payload")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        }));
                    }
                } else if DEVICE_LIFECYCLE_EVENTS.contains(&event_type)
                    && let Some(hint) = extract_device_event_hint(event_type, msg.get("data"))
                {
                    set_last_device_event.set(Some(hint));
                } else if LAYER_HEALTH_EVENTS.contains(&event_type)
                    && let Some((key, health)) =
                        extract_layer_health(msg.get("data").unwrap_or(&serde_json::Value::Null))
                {
                    set_layer_health.update(|map| {
                        map.insert(key, health);
                    });
                }
            }
        }
        _ => {}
    }
}

pub fn extract_control_surface_event_hint(
    event_type: &str,
    data: &serde_json::Value,
) -> Option<ControlSurfaceEventHint> {
    let surface_id = data.get("surface_id")?.as_str()?.to_owned();
    Some(ControlSurfaceEventHint {
        event_type: event_type.to_owned(),
        kind: data
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("surface_changed")
            .to_owned(),
        surface_id,
        revision: data.get("revision").and_then(serde_json::Value::as_u64),
        action_id: data
            .get("action_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        status: data
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        progress: data
            .get("progress")
            .and_then(serde_json::Value::as_f64)
            .map(|progress| {
                #[allow(clippy::cast_possible_truncation)]
                {
                    progress.clamp(0.0, 1.0) as f32
                }
            }),
    })
}

pub fn extract_effect_error_hint(
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

/// Compose the health-map key for a layer. A `SceneLayerId` is unique only
/// within its zone — two groups can carry the same layer id — and
/// the daemon keys health by group as well, so scene and group ride along
/// or one group's health would clobber another group's row.
pub fn layer_health_key(scene_id: &str, group_id: &str, layer_id: &str) -> String {
    format!("{scene_id}/{group_id}/{layer_id}")
}

/// Decode a `layer_health_changed` event into its `(health-map key, health)`.
/// All three identity fields are required: the daemon always sends them, and
/// without scene + group the key would collide across zones.
pub fn extract_layer_health(data: &serde_json::Value) -> Option<(String, LayerHealth)> {
    let scene_id = data.get("scene_id")?.as_str()?;
    let group_id = data.get("group_id")?.as_str()?;
    let layer_id = data.get("layer_id")?.as_str()?;
    let health = LayerHealth::deserialize(data.get("health")?).ok()?;
    Some((layer_health_key(scene_id, group_id, layer_id), health))
}

/// Whether any *current* layer in a zone is in a degraded health
/// state. "Degraded" is the alarming end of `LayerHealth` — a failed
/// producer or a missing asset; transient `Loading`/`Stalled` states do not
/// count, so the §6.7 Screen-row and Stage indicators stay meaningful.
///
/// The health map is append-only and the daemon drops a layer's runtime
/// state on reconcile without a recovery event, so a removed-but-failed
/// layer leaves a stale entry behind. `current_layer_ids` is the group's
/// live layer set; an entry for a layer no longer in it is ignored, so a
/// deleted failed layer cannot keep the surface flagged.
pub fn group_has_degraded_layer(
    layer_health: &HashMap<String, LayerHealth>,
    scene_id: &str,
    group_id: &str,
    current_layer_ids: &[String],
) -> bool {
    layer_health.iter().any(|(key, health)| {
        if !matches!(
            health,
            LayerHealth::Failed { .. } | LayerHealth::AssetMissing
        ) {
            return false;
        }
        let mut parts = key.splitn(3, '/');
        if parts.next() != Some(scene_id) || parts.next() != Some(group_id) {
            return false;
        }
        parts
            .next()
            .is_some_and(|layer| current_layer_ids.iter().any(|id| id.as_str() == layer))
    })
}

pub fn extract_scene_event_hint(
    event_type: &str,
    scene_data: &serde_json::Value,
) -> SceneEventHint {
    // `kind` is overloaded across the scene event family: a ZoneChangeKind
    // on render_group_changed, a SceneLibraryChangeKind on
    // scene_library_changed, and a SceneKind elsewhere. Scope each parse
    // to its event so the fields can't shadow one another.
    let is_render_group_changed = event_type == "render_group_changed";
    let is_library_changed = event_type == "scene_library_changed";
    let generic_kind = (!is_render_group_changed && !is_library_changed)
        .then(|| scene_data.get("kind"))
        .flatten();

    SceneEventHint {
        event_type: event_type.to_owned(),
        scene_id: scene_data
            .get("current")
            .or_else(|| scene_data.get("scene_id"))
            .or_else(|| scene_data.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        group_id: scene_data
            .get("group_id")
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
            .or(generic_kind)
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
        render_group_change_kind: is_render_group_changed
            .then(|| scene_data.get("kind"))
            .flatten()
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        library_change_kind: is_library_changed
            .then(|| scene_data.get("kind"))
            .flatten()
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    }
}

pub fn scene_event_affects_active_effect(hint: &SceneEventHint) -> bool {
    match hint.event_type.as_str() {
        // Library CRUD and scene-settings tweaks never change what's
        // rendering right now.
        "scene_library_changed" | "scene_settings_changed" => false,
        "render_group_changed" => hint.render_group_role != Some(ZoneRole::Display),
        _ => true,
    }
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
