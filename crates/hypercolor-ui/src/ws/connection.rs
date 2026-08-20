//! WebSocket connection lifecycle, reconnect logic, and exponential backoff.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use hypercolor_types::event::LayerHealth;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;

use hypercolor_leptos_ext::events::{
    EventHandle, document as browser_document, document_event_target, on, window as browser_window,
};
use hypercolor_leptos_ext::prelude::{
    TimeoutHandle as BrowserTimeoutHandle, current_page_location, now_ms, random_unit,
    set_timeout as browser_set_timeout,
};
use hypercolor_leptos_ext::ws::transport::{
    WebSocketEventHandlers, arraybuffer_websocket, message_array_buffer, send_websocket_json,
};
use hypercolor_leptos_ext::ws::{
    ExponentialBackoff, HYPERCOLOR_WS_PROTOCOL, PreviewTransportCapability,
};
use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::MessageEvent;

use super::input::InputInjectEdge;
use super::interactive_preview::{
    InteractivePreviewLifecycle, InteractivePreviewLifecycleTracker, InteractivePreviewRequest,
    closed_previews, server_updates,
};
use super::messages::{
    AudioLevel, BackpressureNotice, CanvasFrame, ConnectionState, ControlSurfaceEventHint,
    DeviceEventHint, EffectErrorHint, ExtensionEventHint, InitialSubscriptionAdmission,
    InputSourceStatusEventHint, MacosDaemonOwnershipEventHint, OutputPowerReconciler,
    PerformanceMetrics, PreviewBinaryDecoder, PreviewBinaryMessage, PreviewFrameChannel,
    SceneEventHint, ScreenZonesFrame, handle_json_message, initial_subscription_admission,
    interactive_preview_supported, is_resync_required, reset_layer_health_cache,
};
use super::preview::{
    DEFAULT_PREVIEW_FPS_CAP, PreviewSubscriptionRequest, clear_preview_subscription,
    clear_screen_preview_subscription, clear_web_viewport_preview_subscription,
    request_preview_subscription, request_screen_preview_subscription,
    request_web_viewport_preview_subscription, send_canvas_unsubscribe,
    send_screen_canvas_unsubscribe, send_screen_zones_subscribe, send_screen_zones_unsubscribe,
    send_web_viewport_canvas_unsubscribe, should_stream_preview,
};
use crate::api::DeviceMetricsSnapshot;
use crate::api::client;

const BACKPRESSURE_RECOVERY_MS: f64 = 2_000.0;
const INITIAL_SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(5);
const TAURI_WINDOW_VISIBILITY_EVENT: &str = "hypercolor-window-visibility";
const VERIFIED_DAEMON_CONNECTION_EVENT: &str = "hypercolor-verified-daemon-connection-changed";
const TAURI_WINDOW_VISIBLE_GLOBAL: &str = "__HYPERCOLOR_TAURI_WINDOW_VISIBLE";

fn preview_now_ms() -> u64 {
    let milliseconds = now_ms();
    if !milliseconds.is_finite() || milliseconds <= 0.0 {
        return 0;
    }
    Duration::try_from_secs_f64(milliseconds / 1_000.0)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

fn clear_preview_decoder(
    decoder: &Rc<RefCell<PreviewBinaryDecoder>>,
    timeout: &Rc<RefCell<Option<BrowserTimeoutHandle>>>,
) {
    timeout.borrow_mut().take();
    decoder.borrow_mut().clear();
}

fn schedule_preview_expiry(
    decoder: &Rc<RefCell<PreviewBinaryDecoder>>,
    timeout: &Rc<RefCell<Option<BrowserTimeoutHandle>>>,
) {
    timeout.borrow_mut().take();
    let Some(deadline_ms) = decoder.borrow().next_expiry_ms() else {
        return;
    };
    let delay = Duration::from_millis(deadline_ms.saturating_sub(preview_now_ms()));
    let decoder_for_timeout = Rc::clone(decoder);
    let timeout_for_callback = Rc::clone(timeout);
    let timeout_for_schedule = Rc::clone(timeout);
    let handle = browser_set_timeout(delay, move || {
        timeout_for_callback.borrow_mut().take();
        decoder_for_timeout.borrow_mut().expire_at(preview_now_ms());
        schedule_preview_expiry(&decoder_for_timeout, &timeout_for_schedule);
    });
    *timeout.borrow_mut() = Some(handle);
}

fn quantize_preview_fps(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        ((value * 10.0).round() / 10.0) as f32
    }
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
    pub display_preview_frames: ReadSignal<HashMap<String, CanvasFrame>>,
    pub interactive_preview_frames: ReadSignal<HashMap<String, CanvasFrame>>,
    pub interactive_preview_lifecycles: ReadSignal<HashMap<String, InteractivePreviewLifecycle>>,
    pub interactive_preview_available: ReadSignal<bool>,
    pub preview_fps: ReadSignal<f32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub sensors: ReadSignal<Option<SystemSnapshot>>,
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
    pub output_paused: ReadSignal<bool>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub last_scene_event: ReadSignal<Option<SceneEventHint>>,
    pub last_effect_error: ReadSignal<Option<EffectErrorHint>>,
    pub last_control_surface_event: ReadSignal<Option<ControlSurfaceEventHint>>,
    /// Latest `extension_state_changed` event from a daemon extension.
    /// UI extensions filter on `source`/`kind` and refetch instead of
    /// polling.
    pub last_extension_event: ReadSignal<Option<ExtensionEventHint>>,
    /// Latest safe input-source health transition. REST remains canonical;
    /// consumers use this only to invalidate their status resources.
    pub last_input_source_status_event: ReadSignal<Option<InputSourceStatusEventHint>>,
    /// Latest authoritative macOS daemon-owner transition. REST remains
    /// canonical; consumers use this only to invalidate their snapshots.
    pub last_macos_daemon_ownership_event: ReadSignal<Option<MacosDaemonOwnershipEventHint>>,
    /// Increments each time the daemon socket (re)opens. Bus events fired
    /// while the socket was down are not replayed, so resources mirroring
    /// daemon state over REST should fold this into their fetcher epochs
    /// to heal after a reconnect gap.
    pub connection_generation: ReadSignal<u64>,
    pub layer_health: ReadSignal<HashMap<String, LayerHealth>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub set_preview_width_cap: WriteSignal<u32>,
    pub set_preview_consumers: WriteSignal<u32>,
    pub set_screen_preview_consumers: WriteSignal<u32>,
    /// Latest ambilight zone grid from the `screen_zones` WS channel —
    /// the smoothed, color-tuned colors that screen-reactive effects see.
    pub screen_zones_frame: ReadSignal<Option<ScreenZonesFrame>>,
    /// Bump while a view renders the ambilight zone preview; the channel
    /// subscription turns on at 0→n and off back at zero.
    pub set_screen_zones_consumers: WriteSignal<u32>,
    pub set_web_viewport_preview_consumers: WriteSignal<u32>,
    /// Set to `Some(device_id)` to subscribe the `display_preview`
    /// channel to that device, or `None` to unsubscribe. The subscription
    /// effect inside `WsManager` sends the actual WS messages.
    pub set_display_preview_device: WriteSignal<Option<String>>,
    pub send_zone_layout_preview: Callback<(String, SpatialLayout)>,
    pub clear_zone_layout_preview: Callback<String>,
    pub open_interactive_preview: Callback<InteractivePreviewRequest>,
    pub close_interactive_preview: Callback<String>,
    /// Send addressed browser-preview input edges as one `input_inject` message.
    pub send_input_inject: Callback<(String, Vec<InputInjectEdge>)>,
}

impl Default for WsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (screen_canvas_frame, set_screen_canvas_frame) = signal(None::<CanvasFrame>);
        let (web_viewport_canvas_frame, set_web_viewport_canvas_frame) =
            signal(None::<CanvasFrame>);
        let (display_preview_frames, set_display_preview_frames) =
            signal(HashMap::<String, CanvasFrame>::new());
        let (interactive_preview_frames, set_interactive_preview_frames) =
            signal(HashMap::<String, CanvasFrame>::new());
        let (interactive_preview_lifecycles, set_interactive_preview_lifecycles) =
            signal(HashMap::<String, InteractivePreviewLifecycle>::new());
        let (interactive_preview_available, set_interactive_preview_available) = signal(false);
        let interactive_preview_tracker =
            StoredValue::new(InteractivePreviewLifecycleTracker::default());
        let (display_preview_device, set_display_preview_device) = signal(None::<String>);
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let (connection_generation, set_connection_generation) = signal(0_u64);
        let (preview_fps, set_preview_fps) = signal(0.0_f32);
        let (metrics, set_metrics) = signal(None::<PerformanceMetrics>);
        let (sensors, set_sensors) = signal(None::<SystemSnapshot>);
        let (device_metrics, set_device_metrics) = signal(None::<DeviceMetricsSnapshot>);
        let (device_metrics_consumers, set_device_metrics_consumers) = signal(0_u32);
        let device_metrics_requested: StoredValue<bool> = StoredValue::new(false);
        let (backpressure_notice, set_backpressure_notice) = signal(None::<BackpressureNotice>);
        let (active_effect, set_active_effect) = signal(None::<String>);
        let (output_paused, set_output_paused) = signal(false);
        let output_power_reconciler = StoredValue::new(OutputPowerReconciler::default());
        let (last_device_event, set_last_device_event) = signal(None::<DeviceEventHint>);
        let (last_extension_event, set_last_extension_event) = signal(None::<ExtensionEventHint>);
        let (last_input_source_status_event, set_last_input_source_status_event) =
            signal(None::<InputSourceStatusEventHint>);
        let (last_macos_daemon_ownership_event, set_last_macos_daemon_ownership_event) =
            signal(None::<MacosDaemonOwnershipEventHint>);
        let (last_scene_event, set_last_scene_event) = signal(None::<SceneEventHint>);
        let (last_effect_error, set_last_effect_error) = signal(None::<EffectErrorHint>);
        let (last_control_surface_event, set_last_control_surface_event) =
            signal(None::<ControlSurfaceEventHint>);
        // Per-layer health accumulates from `layer_health_changed` events as
        // they arrive. The map starts empty and the daemon does not replay a
        // snapshot on connect, so a layer that failed before this session
        // connected reads as healthy until its next health transition. A
        // health snapshot in `hello`/`list_layers` is the daemon-side fix.
        let (layer_health, set_layer_health) = signal(HashMap::<String, LayerHealth>::new());
        let (audio_level, set_audio_level) = signal(AudioLevel::default());
        let (preview_target_fps, set_preview_target_fps) = signal(0_u32);
        let (engine_preview_target, set_engine_preview_target) = signal(0_u32);
        let (preview_page_cap, set_preview_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (preview_width_cap, set_preview_width_cap) = signal(0_u32);
        let (preview_consumers, set_preview_consumers) = signal(0_u32);
        let (screen_preview_consumers, set_screen_preview_consumers) = signal(0_u32);
        let (screen_zones_frame, set_screen_zones_frame) = signal(None::<ScreenZonesFrame>);
        let (screen_zones_consumers, set_screen_zones_consumers) = signal(0_u32);
        let screen_zones_requested: StoredValue<bool> = StoredValue::new(false);
        let (web_viewport_preview_consumers, set_web_viewport_preview_consumers) = signal(0_u32);
        let (preview_transport_cap, set_preview_transport_cap) = signal(DEFAULT_PREVIEW_FPS_CAP);
        let (page_visible, set_page_visible) = signal(document_is_visible());
        let (app_window_visible, set_app_window_visible) = signal(tauri_window_is_visible());
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
        let socket_callbacks: StoredValue<Option<WebSocketEventHandlers>, LocalStorage> =
            StoredValue::new_local(None);
        let visibility_change_callback: StoredValue<Option<EventHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let tauri_visibility_change_callback: StoredValue<Option<EventHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let daemon_connection_change_callback: StoredValue<Option<EventHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let reconnect_timeout: StoredValue<Option<BrowserTimeoutHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let initial_subscription_timeout: StoredValue<Option<BrowserTimeoutHandle>, LocalStorage> =
            StoredValue::new_local(None);
        let awaiting_initial_subscription = StoredValue::new(false);

        // Reconnection attempt counter for exponential backoff.
        let reconnect_attempts = StoredValue::new(0_u32);

        // ── connect() ──────────────────────────────────────────────────────
        // Callable multiple times: creates a fresh WebSocket and wires the
        // same signal writers. Called once at startup and again on close/error
        // after a backoff delay.

        let connect: StoredValue<Option<Rc<dyn Fn()>>, LocalStorage> = StoredValue::new_local(None);

        let connect_fn: Rc<dyn Fn()> = Rc::new(move || {
            clear_timeout(reconnect_timeout);
            clear_timeout(initial_subscription_timeout);
            awaiting_initial_subscription.set_value(false);
            dispose_existing_socket(ws_handle, socket_callbacks);
            set_connection_state.set(ConnectionState::Connecting);
            set_backpressure_notice.set(None);
            set_interactive_preview_available.set(false);
            set_interactive_preview_frames.set(HashMap::new());
            interactive_preview_tracker.update_value(InteractivePreviewLifecycleTracker::clear);
            set_interactive_preview_lifecycles.set(HashMap::new());
            set_preview_transport_cap.set(preview_page_cap.get_untracked());
            set_last_backpressure_at_ms.set(None);
            set_layer_health.update(reset_layer_health_cache);
            output_power_reconciler.update_value(|reconciler| {
                reconciler.begin();
            });

            // Reset frame-tracking state so FPS doesn't glitch after reconnect
            last_frame_number.set_value(None);
            last_frame_timestamp.set_value(None);
            smoothed_fps.set_value(0.0);
            requested_preview.set_value(None);
            requested_screen_preview.set_value(None);
            requested_web_viewport_preview.set_value(None);
            screen_zones_requested.set_value(false);
            set_preview_fps.set(0.0);
            set_sensors.set(None);

            let Some(url) = build_ws_url() else {
                set_connection_state.set(ConnectionState::Disconnected);
                return;
            };
            let ws = match arraybuffer_websocket(&url, HYPERCOLOR_WS_PROTOCOL) {
                Ok(ws) => ws,
                Err(_) => {
                    set_connection_state.set(ConnectionState::Error);
                    schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
                    return;
                }
            };
            ws_handle.set_value(Some(ws.clone()));
            let preview_decoder = Rc::new(RefCell::new(PreviewBinaryDecoder::default()));
            let preview_expiry_timeout = Rc::new(RefCell::new(None::<BrowserTimeoutHandle>));

            // onopen — subscribe to events, metrics, and host sensors
            let ws_clone = ws.clone();
            let on_open = move |_| {
                awaiting_initial_subscription.set_value(true);
                let timeout_ws = ws_clone.clone();
                let timeout = browser_set_timeout(INITIAL_SUBSCRIPTION_TIMEOUT, move || {
                    if awaiting_initial_subscription.get_value() {
                        awaiting_initial_subscription.set_value(false);
                        set_connection_state.set(ConnectionState::Error);
                        let _ = timeout_ws.close();
                    }
                });
                initial_subscription_timeout.set_value(Some(timeout));

                let subscribe_msg = serde_json::json!({
                    "type": "subscribe",
                    "preview_transport": PreviewTransportCapability::default().encode(),
                    "topics": [
                        { "topic": "events" },
                        { "topic": "metrics", "config": { "fps": 2.0 } },
                        { "topic": "sensors" }
                    ]
                });
                let _ = send_websocket_json(&ws_clone, &subscribe_msg);
            };

            // onclose — schedule reconnect with backoff
            let close_preview_decoder = Rc::clone(&preview_decoder);
            let close_preview_expiry_timeout = Rc::clone(&preview_expiry_timeout);
            let on_close = move |_| {
                clear_preview_decoder(&close_preview_decoder, &close_preview_expiry_timeout);
                clear_timeout(initial_subscription_timeout);
                awaiting_initial_subscription.set_value(false);
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
                screen_zones_requested.set_value(false);
                set_screen_zones_frame.set(None);
                set_display_preview_frames.update(HashMap::clear);
                set_interactive_preview_available.set(false);
                set_interactive_preview_frames.set(HashMap::new());
                interactive_preview_tracker.update_value(InteractivePreviewLifecycleTracker::clear);
                set_interactive_preview_lifecycles.set(HashMap::new());
                set_sensors.set(None);
                set_layer_health.update(reset_layer_health_cache);
                output_power_reconciler.update_value(|reconciler| {
                    reconciler.begin();
                });
                schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
            };

            // onerror (browser fires close after error, so reconnect triggers there)
            let error_preview_decoder = Rc::clone(&preview_decoder);
            let error_preview_expiry_timeout = Rc::clone(&preview_expiry_timeout);
            let on_error = move |_| {
                clear_preview_decoder(&error_preview_decoder, &error_preview_expiry_timeout);
                clear_timeout(initial_subscription_timeout);
                awaiting_initial_subscription.set_value(false);
                set_connection_state.set(ConnectionState::Error);
                ws_handle.set_value(None);
            };

            // onmessage — handle both JSON and binary frames
            let message_preview_decoder = Rc::clone(&preview_decoder);
            let message_preview_expiry_timeout = Rc::clone(&preview_expiry_timeout);
            let message_ws = ws.clone();
            let on_message = move |event: MessageEvent| {
                // Binary frame (ArrayBuffer)
                if let Some(buffer) = message_array_buffer(&event) {
                    let message = message_preview_decoder
                        .borrow_mut()
                        .decode_at(buffer, preview_now_ms());
                    schedule_preview_expiry(
                        &message_preview_decoder,
                        &message_preview_expiry_timeout,
                    );
                    if let Some(message) = message {
                        match message {
                            PreviewBinaryMessage::Zone(_) => {}
                            PreviewBinaryMessage::Interactive(preview_id, frame) => {
                                if interactive_preview_lifecycles.with_untracked(|lifecycles| {
                                    matches!(
                                        lifecycles.get(&preview_id),
                                        Some(InteractivePreviewLifecycle::Opened { .. })
                                    )
                                }) {
                                    set_interactive_preview_frames.update(|frames| {
                                        frames.insert(preview_id, frame);
                                    });
                                }
                            }
                            PreviewBinaryMessage::Display(device_id, frame) => {
                                set_display_preview_frames.update(|frames| {
                                    frames.insert(device_id, frame);
                                });
                            }
                            PreviewBinaryMessage::ScreenZones(zones) => {
                                set_screen_zones_frame.set(Some(zones));
                            }
                            PreviewBinaryMessage::Frame(channel, frame) => match channel {
                                PreviewFrameChannel::Canvas => {
                                    let current_frame_number = frame.frame_number;
                                    let current_timestamp_ms = frame.timestamp_ms;
                                    set_canvas_frame.set(Some(frame));

                                    if let (
                                        Some(previous_frame_number),
                                        Some(previous_timestamp_ms),
                                    ) = (
                                        last_frame_number.get_value(),
                                        last_frame_timestamp.get_value(),
                                    ) {
                                        let frame_delta = current_frame_number
                                            .saturating_sub(previous_frame_number);
                                        let elapsed_ms = current_timestamp_ms
                                            .saturating_sub(previous_timestamp_ms);

                                        if frame_delta > 0 && elapsed_ms > 0 {
                                            let target_fps = preview_target_fps.get_untracked();
                                            let mut instant_fps = f64::from(frame_delta) * 1000.0
                                                / f64::from(elapsed_ms);
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
                            },
                        }
                    }
                    return;
                }

                // JSON message (String)
                if let Some(text) = event.data().as_string()
                    && let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text)
                {
                    if awaiting_initial_subscription.get_value() {
                        match initial_subscription_admission(&msg) {
                            InitialSubscriptionAdmission::Admitted => {
                                awaiting_initial_subscription.set_value(false);
                                clear_timeout(initial_subscription_timeout);
                                set_connection_state.set(ConnectionState::Connected);
                                set_connection_generation.update(|generation| *generation += 1);
                                reconnect_attempts.set_value(0);
                                clear_timeout(reconnect_timeout);
                            }
                            InitialSubscriptionAdmission::Rejected => {
                                awaiting_initial_subscription.set_value(false);
                                clear_timeout(initial_subscription_timeout);
                                set_connection_state.set(ConnectionState::Error);
                                let _ = message_ws.close();
                                return;
                            }
                            InitialSubscriptionAdmission::Pending => {}
                        }
                    }
                    if is_resync_required(&msg) {
                        let _ = message_ws.close();
                        return;
                    }
                    if msg.get("type").and_then(serde_json::Value::as_str) == Some("hello") {
                        message_preview_decoder
                            .borrow_mut()
                            .apply_hello_capabilities(&msg);
                        schedule_preview_expiry(
                            &message_preview_decoder,
                            &message_preview_expiry_timeout,
                        );
                        set_interactive_preview_available.set(interactive_preview_supported(&msg));
                    }
                    // A subscription acknowledgment reports the whole live
                    // set, so a preview missing from it has closed. Both
                    // halves are read before either is applied, because the
                    // second reads the tracker's own view of what is open.
                    let known = interactive_preview_tracker
                        .with_value(InteractivePreviewLifecycleTracker::known_preview_ids);
                    let mut updates = closed_previews(&msg, &known);
                    updates.extend(server_updates(&msg));
                    if !updates.is_empty() {
                        for update in updates {
                            let preview_id = update.preview_id().to_owned();
                            interactive_preview_tracker
                                .update_value(|tracker| tracker.apply(update));
                            set_interactive_preview_frames.update(|frames| {
                                frames.remove(&preview_id);
                            });
                        }
                        set_interactive_preview_lifecycles.set(
                            interactive_preview_tracker
                                .with_value(InteractivePreviewLifecycleTracker::lifecycles),
                        );
                    }
                    handle_json_message(
                        &msg,
                        &set_active_effect,
                        &set_output_paused,
                        output_power_reconciler,
                        metrics,
                        &set_metrics,
                        &set_device_metrics,
                        &set_sensors,
                        backpressure_notice,
                        &set_backpressure_notice,
                        &set_last_device_event,
                        &set_last_scene_event,
                        &set_last_effect_error,
                        &set_last_control_surface_event,
                        &set_last_extension_event,
                        &set_last_input_source_status_event,
                        &set_last_macos_daemon_ownership_event,
                        &set_layer_health,
                        &set_audio_level,
                        &set_engine_preview_target,
                        &set_preview_target_fps,
                        &set_preview_transport_cap,
                        &set_last_backpressure_at_ms,
                        &set_backpressure_probe_epoch,
                    );
                }
            };
            socket_callbacks.set_value(Some(WebSocketEventHandlers::attach(
                &ws, on_open, on_close, on_error, on_message,
            )));
        });

        connect.set_value(Some(connect_fn));

        // Preview subscription effect — reacts to FPS cap / visibility changes
        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = preview_consumers.get();
            let client_cap = preview_page_cap.get().min(preview_transport_cap.get());
            let width_cap = preview_width_cap.get();
            let is_visible = page_visible.get();
            let window_visible = app_window_visible.get();
            if !should_stream_preview(window_visible, engine_target, consumer_count) {
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
            let window_visible = app_window_visible.get();
            if !should_stream_preview(window_visible, engine_target, consumer_count) {
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

        // Zone frames are tiny and carry no per-client config, so the
        // subscription is a simple on/off keyed to consumers, connection
        // state, and app window visibility.
        Effect::new(move |_| {
            let connected = connection_state.get() == ConnectionState::Connected;
            let consumer_count = screen_zones_consumers.get();
            let window_visible = app_window_visible.get();
            let wants_stream = connected && window_visible && consumer_count > 0;

            if wants_stream {
                if !screen_zones_requested.get_value()
                    && let Some(ws) = ws_handle.get_value()
                {
                    send_screen_zones_subscribe(&ws);
                    screen_zones_requested.set_value(true);
                }
            } else if screen_zones_requested.get_value() {
                if let Some(ws) = ws_handle.get_value() {
                    send_screen_zones_unsubscribe(&ws);
                }
                screen_zones_requested.set_value(false);
                set_screen_zones_frame.set(None);
            }
        });

        Effect::new(move |_| {
            let engine_target = engine_preview_target.get();
            let consumer_count = web_viewport_preview_consumers.get();
            let is_visible = page_visible.get();
            let window_visible = app_window_visible.get();
            if !should_stream_preview(window_visible, engine_target, consumer_count) {
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
                    "topics": [{
                        "topic": "device_metrics",
                        "config": { "fps": 2.0 }
                    }]
                });
                let _ = send_websocket_json(&ws, &msg);
                device_metrics_requested.set_value(true);
            } else if !want && have {
                let msg = serde_json::json!({
                    "type": "unsubscribe",
                    "topics": [{ "topic": "device_metrics" }]
                });
                let _ = send_websocket_json(&ws, &msg);
                device_metrics_requested.set_value(false);
                set_device_metrics.set(None);
            }
        });

        Effect::new(move |_| {
            let _probe = backpressure_probe_epoch.get();
            let Some(last_backpressure_at_ms) = last_backpressure_at_ms.get() else {
                return;
            };
            if now_ms() - last_backpressure_at_ms < BACKPRESSURE_RECOVERY_MS {
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
        // The device is the subscription key, so switching displays is an
        // unsubscribe of the old key and a subscribe of the new one. The
        // followed key is remembered because only it can be unsubscribed,
        // and its cached frame is dropped with it so the UI never flashes
        // a stale image for a display it no longer follows.
        let followed_display = StoredValue::new(None::<String>);
        Effect::new(move |_| {
            let state = connection_state.get();
            let device = display_preview_device.get();
            let is_visible = page_visible.get();
            let window_visible = app_window_visible.get();

            // A dropped socket takes its subscriptions with it, so the
            // followed key is forgotten here, above the handle guard. The
            // handle is nulled on close without notifying anything, so a
            // reset below the guard would never run, and the effect would
            // come back from a reconnect believing it still followed the
            // right device and skip the re-subscribe.
            if state != ConnectionState::Connected {
                if followed_display.get_value().is_some() {
                    followed_display.set_value(None);
                    set_display_preview_frames.update(HashMap::clear);
                }
                return;
            }

            let wanted = (window_visible && is_visible)
                .then_some(device)
                .flatten()
                .filter(|device_id| !device_id.is_empty());
            if followed_display.get_value() == wanted {
                return;
            }
            let Some(ws) = ws_handle.get_value() else {
                return;
            };

            if let Some(previous) = followed_display.get_value() {
                super::preview::send_display_preview_unsubscribe(&ws, &previous);
                set_display_preview_frames.update(|frames| {
                    frames.remove(&previous);
                });
            }
            if let Some(device_id) = wanted.as_deref() {
                super::preview::send_display_preview_subscribe(&ws, device_id, 15);
            }
            followed_display.set_value(wanted);
        });

        // Visibility change listener
        if let Some(document) = browser_document() {
            visibility_change_callback.update_value(|handle| {
                if let Some(mut handle) = handle.take() {
                    handle.cancel();
                }
            });
            let visibility_document = document.clone();
            let on_visibility_change = on(
                document_event_target(&document),
                "visibilitychange",
                move |_| {
                    set_page_visible.set(!visibility_document.hidden());
                },
            );
            visibility_change_callback.set_value(Some(on_visibility_change));
        }

        // Tauri native-window visibility listener. Browser tab visibility only
        // caps preview FPS; a hidden app window should unsubscribe entirely.
        if let Some(window) = browser_window() {
            tauri_visibility_change_callback.update_value(|handle| {
                if let Some(mut handle) = handle.take() {
                    handle.cancel();
                }
            });
            let on_tauri_visibility_change = on(
                window.unchecked_ref(),
                TAURI_WINDOW_VISIBILITY_EVENT,
                move |_| {
                    set_app_window_visible.set(tauri_window_is_visible());
                },
            );
            tauri_visibility_change_callback.set_value(Some(on_tauri_visibility_change));

            daemon_connection_change_callback.update_value(|handle| {
                if let Some(mut handle) = handle.take() {
                    handle.cancel();
                }
            });
            let on_daemon_connection_change = on(
                window.unchecked_ref(),
                VERIFIED_DAEMON_CONNECTION_EVENT,
                move |_| {
                    if let Some(connect_fn) = connect.get_value() {
                        connect_fn();
                    }
                },
            );
            daemon_connection_change_callback.set_value(Some(on_daemon_connection_change));
        }

        // Initial connection
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }

        let send_zone_layout_preview =
            Callback::new(move |(zone_id, layout): (String, SpatialLayout)| {
                if let Some(ws) = ws_handle.get_value() {
                    super::preview::send_zone_layout_preview(&ws, &zone_id, &layout);
                }
            });
        let clear_zone_layout_preview = Callback::new(move |zone_id: String| {
            if let Some(ws) = ws_handle.get_value() {
                super::preview::send_zone_layout_preview_clear(&ws, &zone_id);
            }
        });
        let open_interactive_preview = Callback::new(move |request: InteractivePreviewRequest| {
            set_interactive_preview_frames.update(|frames| {
                frames.remove(&request.preview_id);
            });
            interactive_preview_tracker
                .update_value(|tracker| tracker.request_open(&request.preview_id));
            set_interactive_preview_lifecycles.set(
                interactive_preview_tracker
                    .with_value(InteractivePreviewLifecycleTracker::lifecycles),
            );
            if let Some(ws) = ws_handle.get_value() {
                super::interactive_preview::send_open(&ws, &request);
            }
        });
        let close_interactive_preview = Callback::new(move |preview_id: String| {
            interactive_preview_tracker.update_value(|tracker| tracker.request_close(&preview_id));
            set_interactive_preview_lifecycles.set(
                interactive_preview_tracker
                    .with_value(InteractivePreviewLifecycleTracker::lifecycles),
            );
            if let Some(ws) = ws_handle.get_value() {
                super::interactive_preview::send_close(&ws, &preview_id);
            }
            set_interactive_preview_frames.update(|frames| {
                frames.remove(&preview_id);
            });
        });
        let send_input_inject = Callback::new(
            move |(preview_id, events): (String, Vec<InputInjectEdge>)| {
                if let Some(ws) = ws_handle.get_value() {
                    super::interactive_preview::send_input(&ws, &preview_id, &events);
                }
            },
        );

        Self {
            canvas_frame,
            screen_canvas_frame,
            web_viewport_canvas_frame,
            display_preview_frames,
            interactive_preview_frames,
            interactive_preview_lifecycles,
            interactive_preview_available,
            preview_fps,
            metrics,
            sensors,
            device_metrics,
            set_device_metrics_consumers,
            backpressure_notice,
            active_effect,
            output_paused,
            last_device_event,
            last_scene_event,
            last_effect_error,
            last_control_surface_event,
            last_extension_event,
            last_input_source_status_event,
            last_macos_daemon_ownership_event,
            connection_generation,
            layer_health,
            audio_level,
            preview_target_fps,
            set_preview_cap,
            set_preview_width_cap,
            set_preview_consumers,
            set_screen_preview_consumers,
            screen_zones_frame,
            set_screen_zones_consumers,
            set_web_viewport_preview_consumers,
            set_display_preview_device,
            send_zone_layout_preview,
            clear_zone_layout_preview,
            open_interactive_preview,
            close_interactive_preview,
            send_input_inject,
        }
    }
}

// ── Connection Lifecycle Helpers ────────────────────────────────────────────

/// Schedule a reconnection attempt with exponential backoff + jitter.
fn schedule_reconnect(
    reconnect_attempts: StoredValue<u32>,
    reconnect_timeout: StoredValue<Option<BrowserTimeoutHandle>, LocalStorage>,
    connect: StoredValue<Option<Rc<dyn Fn()>>, LocalStorage>,
) {
    clear_timeout(reconnect_timeout);
    let attempt = reconnect_attempts.get_value();
    reconnect_attempts.set_value(attempt.saturating_add(1));

    let delay = ExponentialBackoff::HYPERCOLOR_DEFAULT
        .delay_for_attempt_with_sample(attempt, random_unit())
        .unwrap_or(ExponentialBackoff::HYPERCOLOR_DEFAULT.base);
    let final_delay = delay.max(Duration::from_millis(100));

    let timeout = browser_set_timeout(final_delay, move || {
        if let Some(connect_fn) = connect.get_value() {
            connect_fn();
        }
    });
    reconnect_timeout.set_value(Some(timeout));
}

fn clear_timeout(timeout_handle: StoredValue<Option<BrowserTimeoutHandle>, LocalStorage>) {
    timeout_handle.update_value(|timeout| {
        if let Some(mut timeout) = timeout.take() {
            timeout.cancel();
        }
    });
}

fn dispose_existing_socket(
    ws_handle: StoredValue<Option<web_sys::WebSocket>>,
    socket_callbacks: StoredValue<Option<WebSocketEventHandlers>, LocalStorage>,
) {
    let Some(existing_ws) = ws_handle.get_value() else {
        socket_callbacks.set_value(None);
        return;
    };

    socket_callbacks.update_value(|callbacks| {
        if let Some(callbacks) = callbacks.take() {
            callbacks.detach_from(&existing_ws);
        }
    });
    let _ = existing_ws.close();
    ws_handle.set_value(None);
}

/// Build WS URL from current page origin.
///
/// Dev builds (Trunk dev server, any port) connect directly to the daemon
/// (:9420) since Trunk's proxy doesn't handle WebSocket upgrades. Release
/// builds are served by the daemon itself, so same-origin works.
fn build_ws_url() -> Option<String> {
    let routed = client::daemon_url("/api/v1/ws")?;
    let native_base = native_websocket_url(&routed);
    let location = current_page_location();
    let ws_protocol = location.websocket_protocol();

    // Dev builds only ever run on the Trunk dev server, whose proxy
    // can't upgrade WebSockets — connect straight to the daemon. The
    // dev port is configurable (`just ui-dev 9431`), so the split keys
    // on build profile, not a port literal. Release builds are served
    // by the daemon itself, where same-origin always works.
    let host = if cfg!(debug_assertions) {
        format!("{}:9420", location.hostname)
    } else {
        location.host()
    };

    let base = native_base.unwrap_or_else(|| format!("{ws_protocol}//{host}/api/v1/ws"));
    Some(authenticated_websocket_url(
        base,
        client::authorization_token().as_deref(),
    ))
}

fn authenticated_websocket_url(base: String, token: Option<&str>) -> String {
    token.map_or(base.clone(), |token| {
        format!("{base}?token={}", percent_encode(token))
    })
}

fn native_websocket_url(routed: &str) -> Option<String> {
    routed
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            routed
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = std::fmt::Write::write_fmt(&mut encoded, format_args!("%{byte:02X}"));
        }
    }
    encoded
}

fn document_is_visible() -> bool {
    browser_document().is_none_or(|document| !document.hidden())
}

fn tauri_window_is_visible() -> bool {
    let Some(window) = browser_window() else {
        return true;
    };

    js_sys::Reflect::get(
        window.as_ref(),
        &JsValue::from_str(TAURI_WINDOW_VISIBLE_GLOBAL),
    )
    .ok()
    .and_then(|value| value.as_bool())
    .unwrap_or(true)
}

#[cfg(test)]
mod transport_tests {
    #[test]
    fn native_daemon_routes_convert_http_schemes_for_websocket() {
        assert_eq!(
            super::native_websocket_url("http://127.0.0.1:9420/api/v1/ws").as_deref(),
            Some("ws://127.0.0.1:9420/api/v1/ws")
        );
        assert_eq!(
            super::native_websocket_url("https://daemon.test/api/v1/ws").as_deref(),
            Some("wss://daemon.test/api/v1/ws")
        );
        assert!(super::native_websocket_url("/api/v1/ws").is_none());
    }

    #[test]
    fn every_connection_uses_the_current_verified_websocket_token() {
        let base = "ws://127.0.0.1:9420/api/v1/ws".to_owned();
        let first = super::authenticated_websocket_url(base.clone(), Some("session-one"));
        let rotated = super::authenticated_websocket_url(base, Some("session-two"));
        assert_eq!(first, "ws://127.0.0.1:9420/api/v1/ws?token=session-one");
        assert_eq!(rotated, "ws://127.0.0.1:9420/api/v1/ws?token=session-two");
        assert_ne!(first, rotated);
    }

    #[test]
    fn health_verified_native_base_enables_websocket_without_session_token() {
        crate::api::client::reset_daemon_transport_for_test();
        crate::api::client::begin_native_daemon_verification();
        assert!(crate::api::client::daemon_url("/api/v1/ws").is_none());

        crate::api::client::install_verified_daemon_connection("https://daemon.lan:19420", None);
        let routed = crate::api::client::daemon_url("/api/v1/ws")
            .expect("health-proven native route should exist");
        let websocket =
            super::native_websocket_url(&routed).expect("HTTPS daemon route should convert to WSS");
        assert_eq!(
            super::authenticated_websocket_url(websocket, None),
            "wss://daemon.lan:19420/api/v1/ws"
        );
        crate::api::client::reset_daemon_transport_for_test();
    }
}
