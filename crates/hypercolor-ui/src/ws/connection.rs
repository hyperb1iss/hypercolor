//! WebSocket connection lifecycle, reconnect logic, and exponential backoff.

use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use super::messages::{
    AudioLevel, BackpressureNotice, CanvasFrame, ConnectionState, DeviceEventHint, EffectErrorHint,
    PerformanceMetrics, PreviewFrameChannel, SceneEventHint, decode_preview_frame,
    handle_json_message,
};
use super::preview::{
    DEFAULT_PREVIEW_FPS_CAP, PreviewSubscriptionRequest, clear_preview_subscription,
    clear_screen_preview_subscription, clear_web_viewport_preview_subscription,
    request_preview_subscription, request_screen_preview_subscription,
    request_web_viewport_preview_subscription, send_canvas_unsubscribe,
    send_screen_canvas_unsubscribe, send_web_viewport_canvas_unsubscribe,
};
use crate::api::DeviceMetricsSnapshot;

/// Reconnection delay bounds (milliseconds).
const RECONNECT_BASE_MS: i32 = 500;
const RECONNECT_MAX_MS: i32 = 15_000;
const BACKPRESSURE_RECOVERY_MS: f64 = 2_000.0;

fn quantize_preview_fps(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        ((value * 10.0).round() / 10.0) as f32
    }
}

struct SocketCallbacks {
    _on_open: Closure<dyn FnMut()>,
    _on_close: Closure<dyn FnMut()>,
    _on_error: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

// ── WebSocket Manager ───────────────────────────────────────────────────────

/// Reactive WebSocket connection to the daemon.
///
/// Returns signals for canvas data, preview FPS, and daemon performance
/// metrics. Canvas streaming is subscribed on demand.
pub struct WsManager {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub screen_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub web_viewport_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    /// Latest JPEG frame from the per-display `display_preview` WS
    /// channel. `None` until the UI selects a display and the first
    /// frame arrives; reset to `None` when the target changes or the
    /// connection drops.
    pub display_preview_frame: ReadSignal<Option<CanvasFrame>>,
    pub preview_fps: ReadSignal<f32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    /// Latest per-device output telemetry snapshot. `None` until the devices
    /// page (or any other consumer) subscribes via
    /// `set_device_metrics_consumers`.
    pub device_metrics: ReadSignal<Option<DeviceMetricsSnapshot>>,
    /// Bump when a view needs live per-device metrics; drop on cleanup.
    /// The daemon subscription turns on when the count transitions 0→n and
    /// off when it drops back to zero.
    pub set_device_metrics_consumers: WriteSignal<u32>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub last_scene_event: ReadSignal<Option<SceneEventHint>>,
    pub last_effect_error: ReadSignal<Option<EffectErrorHint>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub set_preview_width_cap: WriteSignal<u32>,
    pub set_preview_consumers: WriteSignal<u32>,
    pub set_screen_preview_consumers: WriteSignal<u32>,
    pub set_web_viewport_preview_consumers: WriteSignal<u32>,
    /// Set to `Some(device_id)` to subscribe the `display_preview`
    /// channel to that device, or `None` to unsubscribe. The subscription
    /// effect inside `WsManager` sends the actual WS messages.
    pub set_display_preview_device: WriteSignal<Option<String>>,
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (screen_canvas_frame, set_screen_canvas_frame) = signal(None::<CanvasFrame>);
        let (web_viewport_canvas_frame, set_web_viewport_canvas_frame) =
            signal(None::<CanvasFrame>);
        let (display_preview_frame, set_display_preview_frame) = signal(None::<CanvasFrame>);
        let (display_preview_device, set_display_preview_device) = signal(None::<String>);
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let (preview_fps, set_preview_fps) = signal(0.0_f32);
        let (metrics, set_metrics) = signal(None::<PerformanceMetrics>);
        let (device_metrics, set_device_metrics) = signal(None::<DeviceMetricsSnapshot>);
        let (device_metrics_consumers, set_device_metrics_consumers) = signal(0_u32);
        let device_metrics_requested: StoredValue<bool> = StoredValue::new(false);
        let (backpressure_notice, set_backpressure_notice) = signal(None::<BackpressureNotice>);
        let (active_effect, set_active_effect) = signal(None::<String>);
        let (last_device_event, set_last_device_event) = signal(None::<DeviceEventHint>);
        let (last_scene_event, set_last_scene_event) = signal(None::<SceneEventHint>);
        let (last_effect_error, set_last_effect_error) = signal(None::<EffectErrorHint>);
        let (audio_level, set_audio_level) = signal(AudioLevel::default());
        let (preview_target_fps, set_preview_target_fps) = signal(0_u32);
        let (engine_preview_target, set_engine_preview_target) = signal(0_u32);
        let (preview_page_cap, set_preview_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (preview_width_cap, set_preview_width_cap) = signal(0_u32);
        let (preview_consumers, set_preview_consumers) = signal(0_u32);
        let (screen_preview_consumers, set_screen_preview_consumers) = signal(0_u32);
        let (web_viewport_preview_consumers, set_web_viewport_preview_consumers) = signal(0_u32);
        let (preview_transport_cap, set_preview_transport_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (page_visible, set_page_visible) = signal(document_is_visible());
        let (last_backpressure_at_ms, set_last_backpressure_at_ms) = signal(None::<f64>);
        let (backpressure_probe_epoch, set_backpressure_probe_epoch) = signal(0_u64);

        // Track authoritative canvas cadence from backend frame metadata.
        let last_frame_number = StoredValue::new(None::<u32>);
        let last_frame_timestamp = StoredValue::new(None::<u32>);
        let smoothed_fps = StoredValue::new(0.0_f64);
        let requested_preview = StoredValue::new(None::<PreviewSubscriptionRequest>);
        let requested_screen_preview = StoredValue::new(None::<PreviewSubscriptionRequest>);
        let requested_web_viewport_preview = StoredValue::new(None::<PreviewSubscriptionRequest>);

        // Shared WebSocket handle for preview subscription effect.
        let ws_handle: StoredValue<Option<web_sys::WebSocket>> = StoredValue::new(None);
        let socket_callbacks: StoredValue<Option<SocketCallbacks>, LocalStorage> =
            StoredValue::new_local(None);
        let visibility_change_callback: StoredValue<Option<Closure<dyn FnMut()>>, LocalStorage> =
            StoredValue::new_local(None);
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
            dispose_existing_socket(ws_handle, socket_callbacks);
            set_connection_state.set(ConnectionState::Connecting);
            set_backpressure_notice.set(None);
            set_preview_transport_cap.set(preview_page_cap.get_untracked());
            set_last_backpressure_at_ms.set(None);

            // Reset frame-tracking state so FPS doesn't glitch after reconnect
            last_frame_number.set_value(None);
            last_frame_timestamp.set_value(None);
            smoothed_fps.set_value(0.0);
            requested_preview.set_value(None);
            requested_screen_preview.set_value(None);
            requested_web_viewport_preview.set_value(None);
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

            // onclose — schedule reconnect with backoff
            let on_close = Closure::<dyn FnMut()>::new(move || {
                set_connection_state.set(ConnectionState::Disconnected);
                ws_handle.set_value(None);
                clear_preview_subscription(
                    requested_preview,
                    &set_preview_target_fps,
                    &set_preview_fps,
                    &set_canvas_frame,
                );
                clear_screen_preview_subscription(
                    requested_screen_preview,
                    &set_screen_canvas_frame,
                );
                clear_web_viewport_preview_subscription(
                    requested_web_viewport_preview,
                    &set_web_viewport_canvas_frame,
                );
                set_display_preview_frame.set(None);
                schedule_reconnect(reconnect_attempts, reconnect_timeout_id, connect);
            });
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

            // onerror (browser fires close after error, so reconnect triggers there)
            let on_error = Closure::<dyn FnMut()>::new(move || {
                set_connection_state.set(ConnectionState::Error);
                ws_handle.set_value(None);
            });
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

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
                                        let quantized_fps = quantize_preview_fps(next);
                                        if preview_fps.get_untracked() != quantized_fps {
                                            set_preview_fps.set(quantized_fps);
                                        }
                                    }
                                }

                                last_frame_number.set_value(Some(current_frame_number));
                                last_frame_timestamp.set_value(Some(current_timestamp_ms));
                            }
                            PreviewFrameChannel::ScreenCanvas => {
                                set_screen_canvas_frame.set(Some(frame));
                            }
                            PreviewFrameChannel::WebViewportCanvas => {
                                set_web_viewport_canvas_frame.set(Some(frame));
                            }
                            PreviewFrameChannel::DisplayPreview => {
                                set_display_preview_frame.set(Some(frame));
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
                        &set_device_metrics,
                        backpressure_notice,
                        &set_backpressure_notice,
                        &set_last_device_event,
                        &set_last_scene_event,
                        &set_last_effect_error,
                        &set_audio_level,
                        &set_engine_preview_target,
                        &set_preview_target_fps,
                        &set_preview_transport_cap,
                        &set_last_backpressure_at_ms,
                        &set_backpressure_probe_epoch,
                    );
                }
            });
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            socket_callbacks.set_value(Some(SocketCallbacks {
                _on_open: on_open,
                _on_close: on_close,
                _on_error: on_error,
                _on_message: on_message,
            }));
        });

        connect.set_value(Some(connect_fn));

        // Preview subscription effect — reacts to FPS cap / visibility changes
        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = preview_consumers.get();
            let client_cap = preview_page_cap.get().min(preview_transport_cap.get());
            let width_cap = preview_width_cap.get();
            let is_visible = page_visible.get();
            if engine_target == 0 || consumer_count == 0 {
                if let Some(ws) = ws_handle.get_value() {
                    clear_preview_subscription(
                        requested_preview,
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
                    requested_preview,
                    set_preview_target_fps,
                    engine_target,
                    client_cap,
                    width_cap,
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
                        requested_screen_preview,
                        &set_screen_canvas_frame,
                    );
                    send_screen_canvas_unsubscribe(&ws);
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_screen_preview_subscription(
                    &ws,
                    requested_screen_preview,
                    engine_target,
                    is_visible,
                );
            }
        });

        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = web_viewport_preview_consumers.get();
            let is_visible = page_visible.get();
            if engine_target == 0 || consumer_count == 0 {
                if let Some(ws) = ws_handle.get_value() {
                    clear_web_viewport_preview_subscription(
                        requested_web_viewport_preview,
                        &set_web_viewport_canvas_frame,
                    );
                    send_web_viewport_canvas_unsubscribe(&ws);
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_web_viewport_preview_subscription(
                    &ws,
                    requested_web_viewport_preview,
                    engine_target,
                    is_visible,
                );
            }
        });

        Effect::new(move |_| {
            set_preview_transport_cap.set(preview_page_cap.get());
        });

        // Per-device metrics subscription — opt-in via the consumer counter.
        // Re-subscribes after reconnect because the effect depends on
        // `connection_state` and we reset the requested flag when the
        // connection drops.
        Effect::new(move |_| {
            let state = connection_state.get();
            let consumers = device_metrics_consumers.get();

            if state != ConnectionState::Connected {
                device_metrics_requested.set_value(false);
                set_device_metrics.set(None);
                return;
            }

            let Some(ws) = ws_handle.get_value() else {
                return;
            };

            let want = consumers > 0;
            let have = device_metrics_requested.get_value();

            if want && !have {
                let msg = serde_json::json!({
                    "type": "subscribe",
                    "channels": ["device_metrics"],
                    "config": {
                        "device_metrics": { "interval_ms": 500 }
                    }
                });
                let _ = ws.send_with_str(&msg.to_string());
                device_metrics_requested.set_value(true);
            } else if !want && have {
                let msg = serde_json::json!({
                    "type": "unsubscribe",
                    "channels": ["device_metrics"]
                });
                let _ = ws.send_with_str(&msg.to_string());
                device_metrics_requested.set_value(false);
                set_device_metrics.set(None);
            }
        });

        Effect::new(move |_| {
            let _probe = backpressure_probe_epoch.get();
            let Some(last_backpressure_at_ms) = last_backpressure_at_ms.get() else {
                return;
            };
            if js_sys::Date::now() - last_backpressure_at_ms < BACKPRESSURE_RECOVERY_MS {
                return;
            }

            let page_cap = preview_page_cap.get_untracked();
            if preview_transport_cap.get_untracked() != page_cap {
                set_preview_transport_cap.set(page_cap);
            }
            if backpressure_notice.get_untracked().is_some() {
                set_backpressure_notice.set(None);
            }
            set_last_backpressure_at_ms.set(None);
        });

        // Display-preview subscription effect.
        //
        // Watches `display_preview_device` — whenever the UI changes the
        // selected display, re-subscribe the `display_preview` channel
        // with the new device_id; setting `None` unsubscribes and clears
        // the cached frame so the UI doesn't flash a stale image for the
        // old device.
        Effect::new(move |_| {
            let state = connection_state.get();
            let device = display_preview_device.get();
            let is_visible = page_visible.get();
            if state != ConnectionState::Connected {
                set_display_preview_frame.set(None);
                return;
            }
            let Some(ws) = ws_handle.get_value() else {
                return;
            };
            match (is_visible, device) {
                (true, Some(device_id)) if !device_id.is_empty() => {
                    super::preview::send_display_preview_subscribe(&ws, &device_id, 15);
                }
                _ => {
                    super::preview::send_display_preview_unsubscribe(&ws);
                    set_display_preview_frame.set(None);
                }
            }
        });

        // Visibility change listener
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_onvisibilitychange(None);
            visibility_change_callback.set_value(None);
            let visibility_document = document.clone();
            let on_visibility_change = Closure::<dyn FnMut()>::new(move || {
                set_page_visible.set(!visibility_document.hidden());
            });
            document.set_onvisibilitychange(Some(on_visibility_change.as_ref().unchecked_ref()));
            visibility_change_callback.set_value(Some(on_visibility_change));
        }

        // Initial connection
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }

        Self {
            canvas_frame,
            screen_canvas_frame,
            web_viewport_canvas_frame,
            display_preview_frame,
            preview_fps,
            metrics,
            device_metrics,
            set_device_metrics_consumers,
            backpressure_notice,
            active_effect,
            last_device_event,
            last_scene_event,
            last_effect_error,
            audio_level,
            preview_target_fps,
            set_preview_cap,
            set_preview_width_cap,
            set_preview_consumers,
            set_screen_preview_consumers,
            set_web_viewport_preview_consumers,
            set_display_preview_device,
        }
    }
}

// ── Connection Lifecycle Helpers ────────────────────────────────────────────

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

    let callback = Closure::once_into_js(move || {
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }
    });

    if let Some(window) = web_sys::window()
        && let Ok(timeout_id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.unchecked_ref(),
            final_delay,
        )
    {
        reconnect_timeout_id.set_value(Some(timeout_id));
    }
}

fn clear_reconnect_timer(reconnect_timeout_id: StoredValue<Option<i32>>) {
    clear_timeout_handle(reconnect_timeout_id);
}

fn clear_timeout_handle(timeout_handle: StoredValue<Option<i32>>) {
    let Some(timeout_id) = timeout_handle.get_value() else {
        return;
    };

    if let Some(window) = web_sys::window() {
        window.clear_timeout_with_handle(timeout_id);
    }
    timeout_handle.set_value(None);
}

fn dispose_existing_socket(
    ws_handle: StoredValue<Option<web_sys::WebSocket>>,
    socket_callbacks: StoredValue<Option<SocketCallbacks>, LocalStorage>,
) {
    let Some(existing_ws) = ws_handle.get_value() else {
        socket_callbacks.set_value(None);
        return;
    };

    existing_ws.set_onopen(None);
    existing_ws.set_onclose(None);
    existing_ws.set_onerror(None);
    existing_ws.set_onmessage(None);
    let _ = existing_ws.close();
    ws_handle.set_value(None);
    socket_callbacks.set_value(None);
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
