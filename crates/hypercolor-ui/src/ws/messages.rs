//! JSON and binary message handlers for the daemon WebSocket protocol.

use hypercolor_types::scene::{SceneKind, SceneMutationMode};
use leptos::prelude::*;
use serde::Deserialize;

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

// ── Protocol Constants ──────────────────────────────────────────────────────

pub const CANVAS_FRAME_HEADER: u8 = 0x03;
pub const SCREEN_CANVAS_FRAME_HEADER: u8 = 0x05;
pub const WEB_VIEWPORT_CANVAS_FRAME_HEADER: u8 = 0x06;
/// Per-display JPEG preview frames. The body format matches the canvas
/// header (14-byte preamble) and always carries JPEG pixel data so the
/// UI can decode via `createImageBitmap` and paint to a `<canvas>`.
pub const DISPLAY_PREVIEW_FRAME_HEADER: u8 = 0x07;

pub const EFFECT_STARTED_EVENTS: &[&str] =
    &["effect_started", "effect_activated", "effect_changed"];
pub const EFFECT_STOPPED_EVENTS: &[&str] = &["effect_stopped", "effect_deactivated"];
pub const SCENE_EVENTS: &[&str] = &["active_scene_changed", "render_group_changed"];
pub const DEVICE_LIFECYCLE_EVENTS: &[&str] = &[
    "device_connected",
    "device_discovered",
    "device_disconnected",
    "device_state_changed",
    "device_discovery_completed",
];

// ── Canvas Data ─────────────────────────────────────────────────────────────

/// Decoded canvas frame from a binary WebSocket message.
#[derive(Debug, Clone)]
pub struct CanvasFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    format: CanvasPixelFormat,
    pixels: js_sys::Uint8Array,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasPixelFormat {
    Rgb,
    Rgba,
    Jpeg,
}

impl CanvasPixelFormat {
    pub(crate) fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgb => Some(3),
            Self::Rgba => Some(4),
            Self::Jpeg => None,
        }
    }
}

impl CanvasFrame {
    /// Number of pixels in the frame.
    pub fn pixel_count(&self) -> usize {
        let width = usize::try_from(self.width).unwrap_or(0);
        let height = usize::try_from(self.height).unwrap_or(0);
        width.saturating_mul(height)
    }

    /// Sample a pixel as RGBA without copying the full buffer.
    pub fn rgba_at(&self, pixel_index: usize) -> Option<[u8; 4]> {
        let bytes_per_pixel = self.format.bytes_per_pixel()?;
        let offset = u32::try_from(pixel_index.checked_mul(bytes_per_pixel)?).ok()?;
        let last_component = offset.checked_add(match self.format {
            CanvasPixelFormat::Rgb => 2,
            CanvasPixelFormat::Rgba => 3,
            CanvasPixelFormat::Jpeg => return None,
        })?;
        if last_component >= self.pixels.length() {
            return None;
        }

        Some(match self.format {
            CanvasPixelFormat::Rgb => [
                self.pixels.get_index(offset),
                self.pixels.get_index(offset + 1),
                self.pixels.get_index(offset + 2),
                255,
            ],
            CanvasPixelFormat::Rgba => [
                self.pixels.get_index(offset),
                self.pixels.get_index(offset + 1),
                self.pixels.get_index(offset + 2),
                self.pixels.get_index(offset + 3),
            ],
            CanvasPixelFormat::Jpeg => return None,
        })
    }

    /// Borrow the upload-ready pixel buffer for WebGL.
    pub fn pixels_js(&self) -> &js_sys::Uint8Array {
        &self.pixels
    }

    pub fn pixel_format(&self) -> CanvasPixelFormat {
        self.format
    }
}

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
}

#[derive(Debug, Deserialize)]
struct MetricsMessage {
    data: PerformanceMetrics,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewFrameChannel {
    Canvas,
    ScreenCanvas,
    WebViewportCanvas,
    DisplayPreview,
}

/// Decode a binary preview frame.
///
/// Format: `[header:u8][frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8][payload...]`
pub(super) fn decode_preview_frame(
    buffer: js_sys::ArrayBuffer,
) -> Option<(PreviewFrameChannel, CanvasFrame)> {
    let data = js_sys::Uint8Array::new(&buffer);
    if data.length() < 14 {
        return None;
    }

    let channel = match data.get_index(0) {
        CANVAS_FRAME_HEADER => PreviewFrameChannel::Canvas,
        SCREEN_CANVAS_FRAME_HEADER => PreviewFrameChannel::ScreenCanvas,
        WEB_VIEWPORT_CANVAS_FRAME_HEADER => PreviewFrameChannel::WebViewportCanvas,
        DISPLAY_PREVIEW_FRAME_HEADER => PreviewFrameChannel::DisplayPreview,
        _ => return None,
    };

    let frame_number = u32::from_le_bytes([
        data.get_index(1),
        data.get_index(2),
        data.get_index(3),
        data.get_index(4),
    ]);
    let timestamp_ms = u32::from_le_bytes([
        data.get_index(5),
        data.get_index(6),
        data.get_index(7),
        data.get_index(8),
    ]);
    let width = u16::from_le_bytes([data.get_index(9), data.get_index(10)]) as u32;
    let height = u16::from_le_bytes([data.get_index(11), data.get_index(12)]) as u32;
    let format = match data.get_index(13) {
        0 => CanvasPixelFormat::Rgb,
        1 => CanvasPixelFormat::Rgba,
        2 => CanvasPixelFormat::Jpeg,
        _ => return None,
    };
    let pixel_offset = 14_u32;
    let end = match format.bytes_per_pixel() {
        Some(bytes_per_pixel) => {
            let expected_size = usize::try_from(width)
                .ok()?
                .checked_mul(usize::try_from(height).ok()?)?;
            let expected_len = u32::try_from(expected_size.checked_mul(bytes_per_pixel)?).ok()?;
            pixel_offset.checked_add(expected_len)?
        }
        None => data.length(),
    };
    if data.length() < end {
        return None;
    }
    let pixels = data.subarray(pixel_offset, end);

    Some((
        channel,
        CanvasFrame {
            frame_number,
            timestamp_ms,
            width,
            height,
            format,
            pixels,
        },
    ))
}

// ── JSON Message Handler ────────────────────────────────────────────────────

/// Handle incoming JSON events from the daemon.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_json_message(
    msg: &serde_json::Value,
    set_active: &WriteSignal<Option<String>>,
    metrics: ReadSignal<Option<PerformanceMetrics>>,
    set_metrics: &WriteSignal<Option<PerformanceMetrics>>,
    backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    set_backpressure_notice: &WriteSignal<Option<BackpressureNotice>>,
    set_last_device_event: &WriteSignal<Option<DeviceEventHint>>,
    set_last_scene_event: &WriteSignal<Option<SceneEventHint>>,
    set_audio_level: &WriteSignal<AudioLevel>,
    set_engine_preview_target: &WriteSignal<u32>,
    set_preview_target_fps: &WriteSignal<u32>,
    set_preview_transport_cap: &WriteSignal<u32>,
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
                if message.data.fps.target > 0 {
                    set_engine_preview_target.set(message.data.fps.target.min(60));
                }
                // Gate on equality — skip notification when data hasn't changed
                if metrics.get_untracked().as_ref() != Some(&message.data) {
                    set_metrics.set(Some(message.data));
                }
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
                    set_last_scene_event.set(Some(SceneEventHint {
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
                    }));
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
