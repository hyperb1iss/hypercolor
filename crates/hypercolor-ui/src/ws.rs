//! WebSocket connection manager — connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary preview frames.

use std::rc::Rc;

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

pub const DEFAULT_PREVIEW_FPS_CAP: u32 = 30;
const HIDDEN_TAB_PREVIEW_FPS_CAP: u32 = 6;
const SCREEN_PREVIEW_FPS_CAP: u32 = 15;
const CANVAS_FRAME_HEADER: u8 = 0x03;
const SCREEN_CANVAS_FRAME_HEADER: u8 = 0x05;

/// Reconnection delay bounds (milliseconds).
const RECONNECT_BASE_MS: i32 = 500;
const RECONNECT_MAX_MS: i32 = 15_000;

const EFFECT_STARTED_EVENTS: &[&str] = &["effect_started", "effect_activated", "effect_changed"];
const EFFECT_STOPPED_EVENTS: &[&str] = &["effect_stopped", "effect_deactivated"];
const DEVICE_LIFECYCLE_EVENTS: &[&str] = &[
    "device_connected",
    "device_discovered",
    "device_disconnected",
    "device_state_changed",
    "device_discovery_completed",
];

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
    pub pacing: MetricsPacing,
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
/// performance metrics. Canvas streaming is subscribed on demand.
pub struct WsManager {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub screen_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub preview_fps: ReadSignal<f32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub set_preview_consumers: WriteSignal<u32>,
    pub set_screen_preview_consumers: WriteSignal<u32>,
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (screen_canvas_frame, set_screen_canvas_frame) = signal(None::<CanvasFrame>);
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
        let (preview_consumers, set_preview_consumers) = signal(0_u32);
        let (screen_preview_consumers, set_screen_preview_consumers) = signal(0_u32);
        let (preview_transport_cap, set_preview_transport_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (page_visible, set_page_visible) = signal(document_is_visible());

        // Track authoritative canvas cadence from backend frame metadata.
        let last_frame_number = StoredValue::new(None::<u32>);
        let last_frame_timestamp = StoredValue::new(None::<u32>);
        let smoothed_fps = StoredValue::new(0.0_f64);
        let requested_preview_fps = StoredValue::new(0_u32);
        let requested_screen_preview_fps = StoredValue::new(0_u32);

        // Shared WebSocket handle for preview subscription effect.
        let ws_handle: StoredValue<Option<web_sys::WebSocket>> = StoredValue::new(None);
        let reconnect_timeout_id: StoredValue<Option<i32>> = StoredValue::new(None);

        // Reconnection attempt counter for exponential backoff.
        let reconnect_attempts = StoredValue::new(0_u32);

        // Build WebSocket URL relative to page origin
        let ws_url = build_ws_url();
        let ws_url = StoredValue::new(ws_url);

        // ── connect() ──────────────────────────────────────────────────────
        // Callable multiple times: creates a fresh WebSocket and wires the
        // same signal writers. Called once at startup and again on close/error
        // after a backoff delay.

        let connect: StoredValue<Option<Rc<dyn Fn()>>, LocalStorage> = StoredValue::new_local(None);

        let connect_fn: Rc<dyn Fn()> = Rc::new(move || {
            clear_reconnect_timer(reconnect_timeout_id);
            dispose_existing_socket(ws_handle);
            set_connection_state.set(ConnectionState::Connecting);

            // Reset frame-tracking state so FPS doesn't glitch after reconnect
            last_frame_number.set_value(None);
            last_frame_timestamp.set_value(None);
            smoothed_fps.set_value(0.0);
            requested_preview_fps.set_value(0);
            set_preview_fps.set(0.0);

            let url = ws_url.get_value();
            let ws = match web_sys::WebSocket::new_with_str(&url, "hypercolor-v1") {
                Ok(ws) => ws,
                Err(_) => {
                    set_connection_state.set(ConnectionState::Error);
                    schedule_reconnect(reconnect_attempts, reconnect_timeout_id, connect);
                    return;
                }
            };
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            ws_handle.set_value(Some(ws.clone()));

            // onopen — subscribe to events + metrics
            let ws_clone = ws.clone();
            let on_open = Closure::<dyn FnMut()>::new(move || {
                set_connection_state.set(ConnectionState::Connected);
                reconnect_attempts.set_value(0);
                clear_reconnect_timer(reconnect_timeout_id);

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

            // onclose — schedule reconnect with backoff
            let on_close = Closure::<dyn FnMut()>::new(move || {
                set_connection_state.set(ConnectionState::Disconnected);
                ws_handle.set_value(None);
                clear_preview_subscription(
                    requested_preview_fps,
                    &set_preview_target_fps,
                    &set_preview_fps,
                    &set_canvas_frame,
                );
                clear_screen_preview_subscription(
                    requested_screen_preview_fps,
                    &set_screen_canvas_frame,
                );
                schedule_reconnect(reconnect_attempts, reconnect_timeout_id, connect);
            });
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            on_close.forget();

            // onerror (browser fires close after error, so reconnect triggers there)
            let on_error = Closure::<dyn FnMut()>::new(move || {
                set_connection_state.set(ConnectionState::Error);
                ws_handle.set_value(None);
            });
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            on_error.forget();

            // onmessage — handle both JSON and binary frames
            let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                // Binary frame (ArrayBuffer)
                if let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                    if let Some((channel, frame)) = decode_preview_frame(buffer) {
                        match channel {
                            PreviewFrameChannel::Canvas => {
                                let current_frame_number = frame.frame_number;
                                let current_timestamp_ms = frame.timestamp_ms;
                                set_canvas_frame.set(Some(frame));

                                if let (Some(previous_frame_number), Some(previous_timestamp_ms)) = (
                                    last_frame_number.get_value(),
                                    last_frame_timestamp.get_value(),
                                ) {
                                    let frame_delta =
                                        current_frame_number.saturating_sub(previous_frame_number);
                                    let elapsed_ms =
                                        current_timestamp_ms.saturating_sub(previous_timestamp_ms);

                                    if frame_delta > 0 && elapsed_ms > 0 {
                                        let target_fps = preview_target_fps.get_untracked();
                                        let mut instant_fps =
                                            f64::from(frame_delta) * 1000.0 / f64::from(elapsed_ms);
                                        if target_fps > 0 {
                                            instant_fps =
                                                instant_fps.clamp(0.0, f64::from(target_fps));
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
                            PreviewFrameChannel::ScreenCanvas => {
                                set_screen_canvas_frame.set(Some(frame));
                            }
                        }
                    }
                    return;
                }

                // JSON message (String)
                if let Some(text) = event.data().as_string()
                    && let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text)
                {
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
            });
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            on_message.forget();
        });

        connect.set_value(Some(connect_fn));

        // Preview subscription effect — reacts to FPS cap / visibility changes
        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = preview_consumers.get();
            let client_cap = preview_page_cap.get().min(preview_transport_cap.get());
            let is_visible = page_visible.get();
            if engine_target == 0 || consumer_count == 0 {
                if let Some(ws) = ws_handle.get_value() {
                    clear_preview_subscription(
                        requested_preview_fps,
                        &set_preview_target_fps,
                        &set_preview_fps,
                        &set_canvas_frame,
                    );
                    send_canvas_unsubscribe(&ws);
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_preview_subscription(
                    &ws,
                    requested_preview_fps,
                    set_preview_target_fps,
                    engine_target,
                    client_cap,
                    is_visible,
                );
            }
        });

        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = screen_preview_consumers.get();
            let is_visible = page_visible.get();
            if engine_target == 0 || consumer_count == 0 {
                if let Some(ws) = ws_handle.get_value() {
                    clear_screen_preview_subscription(
                        requested_screen_preview_fps,
                        &set_screen_canvas_frame,
                    );
                    send_screen_canvas_unsubscribe(&ws);
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_screen_preview_subscription(
                    &ws,
                    requested_screen_preview_fps,
                    engine_target,
                    is_visible,
                );
            }
        });

        Effect::new(move |_| {
            set_preview_transport_cap.set(preview_page_cap.get());
        });

        // Visibility change listener
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            let visibility_document = document.clone();
            let on_visibility_change = Closure::<dyn FnMut()>::new(move || {
                set_page_visible.set(!visibility_document.hidden());
            });
            document.set_onvisibilitychange(Some(on_visibility_change.as_ref().unchecked_ref()));
            on_visibility_change.forget();
        }

        // Initial connection
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }

        Self {
            canvas_frame,
            screen_canvas_frame,
            connection_state,
            preview_fps,
            metrics,
            backpressure_notice,
            active_effect,
            last_device_event,
            audio_level,
            preview_target_fps,
            set_preview_cap,
            set_preview_consumers,
            set_screen_preview_consumers,
        }
    }
}

/// Schedule a reconnection attempt with exponential backoff + jitter.
fn schedule_reconnect(
    reconnect_attempts: StoredValue<u32>,
    reconnect_timeout_id: StoredValue<Option<i32>>,
    connect: StoredValue<Option<Rc<dyn Fn()>>, LocalStorage>,
) {
    clear_reconnect_timer(reconnect_timeout_id);
    let attempt = reconnect_attempts.get_value();
    reconnect_attempts.set_value(attempt.saturating_add(1));

    // Exponential backoff: 500ms, 1s, 2s, 4s, 8s, capped at 15s
    let base_delay = RECONNECT_BASE_MS.saturating_mul(1_i32.wrapping_shl(attempt.min(5)));
    let delay = base_delay.min(RECONNECT_MAX_MS);

    // Add jitter (±25%) to prevent thundering herd on daemon restart
    let jitter = (js_sys::Math::random() * 0.5 - 0.25) * f64::from(delay);
    #[allow(clippy::cast_possible_truncation)]
    let final_delay = (f64::from(delay) + jitter).max(100.0) as i32;

    let callback = Closure::<dyn FnMut()>::new(move || {
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }
    });

    if let Some(window) = web_sys::window() {
        if let Ok(timeout_id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            final_delay,
        ) {
            reconnect_timeout_id.set_value(Some(timeout_id));
        }
    }
    callback.forget();
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn clear_reconnect_timer(reconnect_timeout_id: StoredValue<Option<i32>>) {
    let Some(timeout_id) = reconnect_timeout_id.get_value() else {
        return;
    };

    if let Some(window) = web_sys::window() {
        window.clear_timeout_with_handle(timeout_id);
    }
    reconnect_timeout_id.set_value(None);
}

fn dispose_existing_socket(ws_handle: StoredValue<Option<web_sys::WebSocket>>) {
    let Some(existing_ws) = ws_handle.get_value() else {
        return;
    };

    existing_ws.set_onopen(None);
    existing_ws.set_onclose(None);
    existing_ws.set_onerror(None);
    existing_ws.set_onmessage(None);
    let _ = existing_ws.close();
    ws_handle.set_value(None);
}

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
    let port = location.port().unwrap_or_default();

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
            "canvas": { "fps": desired_fps, "format": "rgb" }
        }
    });
    let _ = ws.send_with_str(&subscribe_msg.to_string());
}

fn request_screen_preview_subscription(
    ws: &web_sys::WebSocket,
    requested_preview_fps: StoredValue<u32>,
    engine_target_fps: u32,
    page_visible: bool,
) {
    let desired_fps =
        desired_preview_fps(engine_target_fps, SCREEN_PREVIEW_FPS_CAP, page_visible);
    if desired_fps == requested_preview_fps.get_value() {
        return;
    }

    requested_preview_fps.set_value(desired_fps);

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "channels": ["screen_canvas"],
        "config": {
            "screen_canvas": { "fps": desired_fps, "format": "rgb" }
        }
    });
    let _ = ws.send_with_str(&subscribe_msg.to_string());
}

fn clear_preview_subscription(
    requested_preview_fps: StoredValue<u32>,
    set_preview_target_fps: &WriteSignal<u32>,
    set_preview_fps: &WriteSignal<f32>,
    set_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_fps.set_value(0);
    set_preview_target_fps.set(0);
    set_preview_fps.set(0.0);
    set_canvas_frame.set(None);
}

fn clear_screen_preview_subscription(
    requested_preview_fps: StoredValue<u32>,
    set_screen_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_fps.set_value(0);
    set_screen_canvas_frame.set(None);
}

fn send_canvas_unsubscribe(ws: &web_sys::WebSocket) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "channels": ["canvas"]
    });
    let _ = ws.send_with_str(&unsubscribe_msg.to_string());
}

fn send_screen_canvas_unsubscribe(ws: &web_sys::WebSocket) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "channels": ["screen_canvas"]
    });
    let _ = ws.send_with_str(&unsubscribe_msg.to_string());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewFrameChannel {
    Canvas,
    ScreenCanvas,
}

/// Decode a binary preview frame.
///
/// Format: `[header:u8][frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8][pixels...]`
fn decode_preview_frame(buffer: js_sys::ArrayBuffer) -> Option<(PreviewFrameChannel, CanvasFrame)> {
    let data = js_sys::Uint8Array::new(&buffer);
    if data.length() < 14 {
        return None;
    }

    let channel = match data.get_index(0) {
        CANVAS_FRAME_HEADER => PreviewFrameChannel::Canvas,
        SCREEN_CANVAS_FRAME_HEADER => PreviewFrameChannel::ScreenCanvas,
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
