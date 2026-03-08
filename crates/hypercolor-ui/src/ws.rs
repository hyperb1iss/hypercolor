//! WebSocket connection manager — connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary canvas frames (0x03 header).

use std::sync::Arc;

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

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
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
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
    pub preview_target_fps: u32,
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let (preview_fps, set_preview_fps) = signal(0.0_f32);
        let (metrics, set_metrics) = signal(None::<PerformanceMetrics>);
        let (backpressure_notice, set_backpressure_notice) = signal(None::<BackpressureNotice>);
        let (active_effect, set_active_effect) = signal(None::<String>);

        // Build WebSocket URL relative to page origin
        let ws_url = build_ws_url();

        set_connection_state.set(ConnectionState::Connecting);

        // Track frame timing for a smoother preview-FPS readout.
        let last_frame_time = StoredValue::new(0.0_f64);
        let smoothed_fps = StoredValue::new(0.0_f64);

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
                    preview_target_fps: 0,
                };
            }
        };

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let preview_target_fps = 60;

        // onopen — subscribe to canvas + events + metrics
        let ws_clone = ws.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Connected);

            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "channels": ["canvas", "events", "metrics"],
                "config": {
                    "canvas": { "fps": preview_target_fps, "format": "rgba" },
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

        // onmessage — handle both JSON and binary frames
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            // Binary frame (ArrayBuffer)
            if let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                let array = js_sys::Uint8Array::new(&buffer);
                let data = array.to_vec();

                if let Some(frame) = decode_canvas_frame(data) {
                    set_canvas_frame.set(Some(frame));

                    // Use a light EWMA so the displayed preview FPS reflects
                    // sustained delivery instead of jumping between one-second buckets.
                    let now = js_sys::Date::now();
                    let last = last_frame_time.get_value();
                    if last > 0.0 {
                        let elapsed = now - last;
                        if elapsed > 0.0 {
                            let instant_fps = (1000.0 / elapsed).clamp(0.0, 120.0);
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
                    last_frame_time.set_value(now);
                }
                return;
            }

            // JSON message (String)
            if let Some(text) = event.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    handle_json_message(
                        &msg,
                        &set_active_effect,
                        &set_metrics,
                        &set_backpressure_notice,
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
            preview_target_fps,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build WS URL from current page origin.
fn build_ws_url() -> String {
    let window = web_sys::window().expect("no window");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .host()
        .unwrap_or_else(|_| "127.0.0.1:9420".to_string());

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    format!("{ws_protocol}//{host}/api/v1/ws")
}

/// Decode a binary canvas frame.
///
/// Format: `[0x03][frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8][pixels...]`
fn decode_canvas_frame(data: Vec<u8>) -> Option<CanvasFrame> {
    if data.len() < 14 {
        return None;
    }

    // Magic byte check
    if data[0] != 0x03 {
        return None;
    }

    let frame_number = u32::from_le_bytes(data[1..5].try_into().ok()?);
    let width = u16::from_le_bytes([data[9], data[10]]) as u32;
    let height = u16::from_le_bytes([data[11], data[12]]) as u32;
    let format = data[13]; // 0 = RGB, 1 = RGBA
    let expected_size = (width * height) as usize;
    let pixel_data = &data[14..];

    let pixels = if format == 1 {
        // Already RGBA
        let expected_len = expected_size * 4;
        if pixel_data.len() < expected_len {
            return None;
        }
        Arc::<[u8]>::from(&pixel_data[..expected_len])
    } else {
        // RGB → convert to RGBA
        let expected_len = expected_size * 3;
        if pixel_data.len() < expected_len {
            return None;
        }
        let mut rgba = Vec::with_capacity(expected_size * 4);
        for chunk in pixel_data[..expected_len].chunks_exact(3) {
            rgba.push(chunk[0]);
            rgba.push(chunk[1]);
            rgba.push(chunk[2]);
            rgba.push(255);
        }
        Arc::<[u8]>::from(rgba)
    };

    Some(CanvasFrame {
        frame_number,
        width,
        height,
        pixels,
    })
}

/// Handle incoming JSON events from the daemon.
fn handle_json_message(
    msg: &serde_json::Value,
    set_active: &WriteSignal<Option<String>>,
    set_metrics: &WriteSignal<Option<PerformanceMetrics>>,
    set_backpressure_notice: &WriteSignal<Option<BackpressureNotice>>,
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
            }
        }
        "metrics" => {
            if let Ok(message) = serde_json::from_value::<MetricsMessage>(msg.clone()) {
                set_metrics.set(Some(message.data));
            }
        }
        "backpressure" => {
            if let Ok(message) = serde_json::from_value::<BackpressureMessage>(msg.clone()) {
                set_backpressure_notice.set(Some(BackpressureNotice {
                    dropped_frames: message.dropped_frames,
                    channel: message.channel,
                    recommendation: message.recommendation,
                    suggested_fps: message.suggested_fps,
                }));
            }
        }
        "event" => {
            // Handle effect change events
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
                }
            }
        }
        _ => {}
    }
}
