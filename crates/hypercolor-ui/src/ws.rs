//! WebSocket connection manager — connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary canvas frames (0x03 header).

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

pub const DEFAULT_PREVIEW_FPS_CAP: u32 = 30;
const HIDDEN_TAB_PREVIEW_FPS_CAP: u32 = 6;

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
}

impl CanvasPixelFormat {
    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
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
        let offset = u32::try_from(pixel_index.checked_mul(self.format.bytes_per_pixel())?).ok()?;
        let last_component = offset.checked_add(match self.format {
            CanvasPixelFormat::Rgb => 2,
            CanvasPixelFormat::Rgba => 3,
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

/// Live performance metrics streamed from the daemon.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PerformanceMetrics {
    pub fps: MetricsFps,
    pub frame_time: MetricsFrameTime,
    pub stages: MetricsStages,
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
    pub effect_rendering_ms: f64,
    pub spatial_sampling_ms: f64,
    pub device_output_ms: f64,
    pub event_bus_ms: f64,
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

// ── WebSocket Manager ───────────────────────────────────────────────────────

/// Reactive WebSocket connection to the daemon.
///
/// Returns signals for canvas data, connection state, preview FPS, and daemon
/// performance metrics. Automatically subscribes to canvas + events + metrics.
pub struct WsManager {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub preview_fps: ReadSignal<f32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let (preview_fps, set_preview_fps) = signal(0.0_f32);
        let (metrics, set_metrics) = signal(None::<PerformanceMetrics>);
        let (backpressure_notice, set_backpressure_notice) = signal(None::<BackpressureNotice>);
        let (active_effect, set_active_effect) = signal(None::<String>);
        let (last_device_event, set_last_device_event) = signal(None::<DeviceEventHint>);
        let (audio_level, set_audio_level) = signal(AudioLevel::default());
        let (preview_target_fps, set_preview_target_fps) = signal(0_u32);
        let (engine_preview_target, set_engine_preview_target) = signal(0_u32);
        let (preview_page_cap, set_preview_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (preview_transport_cap, set_preview_transport_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (page_visible, set_page_visible) = signal(document_is_visible());

        // Build WebSocket URL relative to page origin
        let ws_url = build_ws_url();

        set_connection_state.set(ConnectionState::Connecting);

        // Track authoritative canvas cadence from backend frame metadata.
        let last_frame_number = StoredValue::new(None::<u32>);
        let last_frame_timestamp = StoredValue::new(None::<u32>);
        let smoothed_fps = StoredValue::new(0.0_f64);
        let requested_preview_fps = StoredValue::new(0_u32);

        // Create WebSocket
        let ws = web_sys::WebSocket::new_with_str(&ws_url, "hypercolor-v1");
        let ws = match ws {
            Ok(ws) => ws,
            Err(_) => {
                set_connection_state.set(ConnectionState::Error);
                return Self {
                    canvas_frame,
                    connection_state,
                    preview_fps,
                    metrics,
                    backpressure_notice,
                    active_effect,
                    last_device_event,
                    audio_level,
                    preview_target_fps,
                    set_preview_cap,
                };
            }
        };

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let preview_ws = ws.clone();
        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let client_cap = preview_page_cap.get().min(preview_transport_cap.get());
            let is_visible = page_visible.get();
            if engine_target == 0 {
                return;
            }

            request_preview_subscription(
                &preview_ws,
                requested_preview_fps,
                set_preview_target_fps,
                engine_target,
                client_cap,
                is_visible,
            );
        });

        Effect::new(move |_| {
            set_preview_transport_cap.set(preview_page_cap.get());
        });

        // onopen — subscribe to events + metrics, then pace preview after hello.
        let ws_clone = ws.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Connected);

            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "channels": ["events", "metrics"],
                "config": {
                    "metrics": { "interval_ms": 500 }
                }
            });
            let _ = ws_clone.send_with_str(&subscribe_msg.to_string());
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        // onclose
        let on_close = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Disconnected);
        });
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        // onerror
        let on_error = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Error);
        });
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            let visibility_document = document.clone();
            let on_visibility_change = Closure::<dyn FnMut()>::new(move || {
                set_page_visible.set(!visibility_document.hidden());
            });
            document.set_onvisibilitychange(Some(on_visibility_change.as_ref().unchecked_ref()));
            on_visibility_change.forget();
        }

        // onmessage — handle both JSON and binary frames
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            // Binary frame (ArrayBuffer)
            if let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                if let Some(frame) = decode_canvas_frame(buffer) {
                    let current_frame_number = frame.frame_number;
                    let current_timestamp_ms = frame.timestamp_ms;
                    set_canvas_frame.set(Some(frame));

                    if let (Some(previous_frame_number), Some(previous_timestamp_ms)) = (
                        last_frame_number.get_value(),
                        last_frame_timestamp.get_value(),
                    ) {
                        let frame_delta =
                            current_frame_number.saturating_sub(previous_frame_number);
                        let elapsed_ms = current_timestamp_ms.saturating_sub(previous_timestamp_ms);

                        if frame_delta > 0 && elapsed_ms > 0 {
                            let target_fps = preview_target_fps.get_untracked();
                            let mut instant_fps =
                                f64::from(frame_delta) * 1000.0 / f64::from(elapsed_ms);
                            if target_fps > 0 {
                                instant_fps = instant_fps.clamp(0.0, f64::from(target_fps));
                            } else {
                                instant_fps = instant_fps.clamp(0.0, 120.0);
                            }

                            let previous = smoothed_fps.get_value();
                            let next = if previous <= 0.0 {
                                instant_fps
                            } else {
                                previous * 0.82 + instant_fps * 0.18
                            };
                            smoothed_fps.set_value(next);
                            #[allow(clippy::cast_possible_truncation)]
                            set_preview_fps.set(next as f32);
                        }
                    }

                    last_frame_number.set_value(Some(current_frame_number));
                    last_frame_timestamp.set_value(Some(current_timestamp_ms));
                }
                return;
            }

            // JSON message (String)
            if let Some(text) = event.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    handle_json_message(
                        &msg,
                        &set_active_effect,
                        metrics,
                        &set_metrics,
                        backpressure_notice,
                        &set_backpressure_notice,
                        &set_last_device_event,
                        &set_audio_level,
                        &set_engine_preview_target,
                        &set_preview_target_fps,
                        &set_preview_transport_cap,
                    );
                }
            }
        });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        Self {
            canvas_frame,
            connection_state,
            preview_fps,
            metrics,
            backpressure_notice,
            active_effect,
            last_device_event,
            audio_level,
            preview_target_fps,
            set_preview_cap,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build WS URL from current page origin.
///
/// When running on the Trunk dev server (:9430), connects directly to the
/// daemon (:9420) since Trunk's proxy doesn't reliably handle WebSocket
/// upgrades. In production the daemon serves the UI itself, so same-origin works.
fn build_ws_url() -> String {
    let window = web_sys::window().expect("no window");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let hostname = location
        .hostname()
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = location
        .port()
        .unwrap_or_default();

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };

    // Trunk dev server → bypass proxy, connect directly to daemon
    let host = if port == "9430" {
        format!("{hostname}:9420")
    } else if port.is_empty() {
        hostname
    } else {
        format!("{hostname}:{port}")
    };

    format!("{ws_protocol}//{host}/api/v1/ws")
}

fn document_is_visible() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_none_or(|document| !document.hidden())
}

fn desired_preview_fps(engine_target_fps: u32, client_cap: u32, page_visible: bool) -> u32 {
    let capped_target = engine_target_fps.clamp(1, 60).min(client_cap.clamp(1, 60));
    if page_visible {
        capped_target
    } else {
        capped_target.min(HIDDEN_TAB_PREVIEW_FPS_CAP)
    }
}

fn request_preview_subscription(
    ws: &web_sys::WebSocket,
    requested_preview_fps: StoredValue<u32>,
    set_preview_target_fps: WriteSignal<u32>,
    engine_target_fps: u32,
    client_cap: u32,
    page_visible: bool,
) {
    let desired_fps = desired_preview_fps(engine_target_fps, client_cap, page_visible);
    if desired_fps == requested_preview_fps.get_value() {
        return;
    }

    requested_preview_fps.set_value(desired_fps);
    set_preview_target_fps.set(desired_fps);

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "channels": ["canvas"],
        "config": {
            "canvas": { "fps": desired_fps, "format": "rgba" }
        }
    });
    let _ = ws.send_with_str(&subscribe_msg.to_string());
}

/// Decode a binary canvas frame.
///
/// Format: `[0x03][frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8][pixels...]`
fn decode_canvas_frame(buffer: js_sys::ArrayBuffer) -> Option<CanvasFrame> {
    let data = js_sys::Uint8Array::new(&buffer);
    if data.length() < 14 {
        return None;
    }

    // Magic byte check
    if data.get_index(0) != 0x03 {
        return None;
    }

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
        _ => return None,
    };
    let expected_size = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    let pixel_offset = 14_u32;
    let expected_len = u32::try_from(expected_size.checked_mul(format.bytes_per_pixel())?).ok()?;
    let end = pixel_offset.checked_add(expected_len)?;
    if data.length() < end {
        return None;
    }
    let pixels = data.subarray(pixel_offset, end);

    Some(CanvasFrame {
        frame_number,
        timestamp_ms,
        width,
        height,
        format,
        pixels,
    })
}

/// Handle incoming JSON events from the daemon.
#[allow(clippy::too_many_arguments)]
fn handle_json_message(
    msg: &serde_json::Value,
    set_active: &WriteSignal<Option<String>>,
    metrics: ReadSignal<Option<PerformanceMetrics>>,
    set_metrics: &WriteSignal<Option<PerformanceMetrics>>,
    backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    set_backpressure_notice: &WriteSignal<Option<BackpressureNotice>>,
    set_last_device_event: &WriteSignal<Option<DeviceEventHint>>,
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
                if let Some(active) = state.get("effect").or_else(|| state.get("active_effect")) {
                    let name = active
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    set_active.set(name);
                }

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
                if event_type == "effect_activated" || event_type == "effect_changed" {
                    let name = msg
                        .get("data")
                        .and_then(|d| d.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    set_active.set(name);
                } else if event_type == "effect_deactivated" || event_type == "effect_stopped" {
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
                } else if matches!(
                    event_type,
                    "device_discovered"
                        | "device_disconnected"
                        | "device_state_changed"
                        | "device_discovery_completed"
                ) {
                    let device_id = msg
                        .get("data")
                        .and_then(|data| data.get("device_id"))
                        .and_then(|id| id.as_str())
                        .map(String::from);
                    let found_count = msg
                        .get("data")
                        .and_then(|data| data.get("found"))
                        .and_then(|found| found.as_array())
                        .map(std::vec::Vec::len);
                    set_last_device_event.set(Some(DeviceEventHint {
                        event_type: event_type.to_owned(),
                        device_id,
                        found_count,
                    }));
                }
            }
        }
        _ => {}
    }
}
