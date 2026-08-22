//! WebSocket connection lifecycle, reconnect logic, and exponential backoff.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use hypercolor_types::event::LayerHealth;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;

use hypercolor_leptos_ext::events::{
    EventHandle, document as browser_document, document_event_target, on, window as browser_window,
};
use hypercolor_leptos_ext::prelude::{
    TimeoutHandle as BrowserTimeoutHandle, now_ms, random_unit, set_timeout as browser_set_timeout,
};
use hypercolor_leptos_ext::ws::{
    ExponentialBackoff, HYPERCOLOR_WS_PROTOCOL, PreviewTransportCapability,
};
use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

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
use super::transport::{
    WebSocketConnectRequest, WebSocketConnection, WebSocketEvent, WebSocketMessage,
    connect as connect_websocket, send_json,
};
use crate::api::DeviceMetricsSnapshot;

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

struct ConnectionEventGate {
    socket_generation: StoredValue<u64>,
    generation: u64,
    ready: Cell<bool>,
    terminal: Cell<bool>,
    pending: RefCell<VecDeque<WebSocketEvent>>,
    process: Rc<dyn Fn(WebSocketEvent)>,
}

impl ConnectionEventGate {
    fn new(
        socket_generation: StoredValue<u64>,
        generation: u64,
        process: Rc<dyn Fn(WebSocketEvent)>,
    ) -> Rc<Self> {
        Rc::new(Self {
            socket_generation,
            generation,
            ready: Cell::new(false),
            terminal: Cell::new(false),
            pending: RefCell::new(VecDeque::new()),
            process,
        })
    }

    fn handler(self: &Rc<Self>) -> Rc<dyn Fn(WebSocketEvent)> {
        let gate = Rc::clone(self);
        Rc::new(move |event| gate.receive(event))
    }

    fn activate(&self) {
        self.ready.set(true);
        while let Some(event) = self.pending.borrow_mut().pop_front() {
            self.dispatch(event);
        }
    }

    fn receive(&self, event: WebSocketEvent) {
        if self.socket_generation.get_value() != self.generation || self.terminal.get() {
            return;
        }
        if self.ready.get() {
            self.dispatch(event);
        } else {
            self.pending.borrow_mut().push_back(event);
        }
    }

    fn dispatch(&self, event: WebSocketEvent) {
        if self.socket_generation.get_value() != self.generation || self.terminal.get() {
            return;
        }
        let terminal = matches!(event, WebSocketEvent::Closed { .. });
        if terminal {
            self.terminal.set(true);
        }
        (self.process)(event);
        if terminal {
            self.socket_generation
                .set_value(self.generation.wrapping_add(1));
        }
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
        let ws_handle: StoredValue<Option<Rc<dyn WebSocketConnection>>, LocalStorage> =
            StoredValue::new_local(None);
        let socket_generation = StoredValue::new(0_u64);
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
            dispose_existing_socket(ws_handle, socket_generation);
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

            let generation = socket_generation.get_value().wrapping_add(1);
            socket_generation.set_value(generation);
            let preview_decoder = Rc::new(RefCell::new(PreviewBinaryDecoder::default()));
            let preview_expiry_timeout = Rc::new(RefCell::new(None::<BrowserTimeoutHandle>));
            let event_preview_decoder = Rc::clone(&preview_decoder);
            let event_preview_expiry_timeout = Rc::clone(&preview_expiry_timeout);
            let process_event: Rc<dyn Fn(WebSocketEvent)> = Rc::new(move |event| {
                if socket_generation.get_value() != generation {
                    return;
                }

                match event {
                    WebSocketEvent::Opened => {
                        awaiting_initial_subscription.set_value(true);
                        let timeout =
                            browser_set_timeout(INITIAL_SUBSCRIPTION_TIMEOUT, move || {
                                if socket_generation.get_value() == generation
                                    && awaiting_initial_subscription.get_value()
                                {
                                    awaiting_initial_subscription.set_value(false);
                                    set_connection_state.set(ConnectionState::Error);
                                    schedule_reconnect(
                                        reconnect_attempts,
                                        reconnect_timeout,
                                        connect,
                                    );
                                    if let Some(connection) = ws_handle.get_value() {
                                        let _ = connection.close();
                                    }
                                }
                            });
                        initial_subscription_timeout.set_value(Some(timeout));

                        let subscribe_msg = serde_json::json!({
                            "type": "subscribe",
                            "preview_transport": PreviewTransportCapability::default().encode(),
                            "topics": [
                                { "topic": "events" },
                                { "topic": "metrics", "config": { "interval_ms": 500 } },
                                { "topic": "sensors" }
                            ]
                        });
                        if let Some(connection) = ws_handle.get_value() {
                            let _ = send_json(connection.as_ref(), &subscribe_msg);
                        }
                    }
                    WebSocketEvent::Closed { .. } => {
                        socket_generation.set_value(generation.wrapping_add(1));
                        clear_preview_decoder(
                            &event_preview_decoder,
                            &event_preview_expiry_timeout,
                        );
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
                        interactive_preview_tracker
                            .update_value(InteractivePreviewLifecycleTracker::clear);
                        set_interactive_preview_lifecycles.set(HashMap::new());
                        set_sensors.set(None);
                        set_layer_health.update(reset_layer_health_cache);
                        output_power_reconciler.update_value(|reconciler| {
                            reconciler.begin();
                        });
                        schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
                    }
                    WebSocketEvent::Error { .. } => {
                        clear_preview_decoder(
                            &event_preview_decoder,
                            &event_preview_expiry_timeout,
                        );
                        clear_timeout(initial_subscription_timeout);
                        awaiting_initial_subscription.set_value(false);
                        set_connection_state.set(ConnectionState::Error);
                        schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
                    }
                    WebSocketEvent::Message(WebSocketMessage::Binary(frame)) => {
                        let message = event_preview_decoder
                            .borrow_mut()
                            .decode_at(frame, preview_now_ms());
                        schedule_preview_expiry(
                            &event_preview_decoder,
                            &event_preview_expiry_timeout,
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
                                                let mut instant_fps = f64::from(frame_delta)
                                                    * 1000.0
                                                    / f64::from(elapsed_ms);
                                                if target_fps > 0 {
                                                    instant_fps = instant_fps
                                                        .clamp(0.0, f64::from(target_fps));
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
                    }
                    WebSocketEvent::Message(WebSocketMessage::Text(text)) => {
                        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) else {
                            return;
                        };
                        if awaiting_initial_subscription.get_value() {
                            match initial_subscription_admission(&msg) {
                                InitialSubscriptionAdmission::Admitted => {
                                    awaiting_initial_subscription.set_value(false);
                                    clear_timeout(initial_subscription_timeout);
                                    set_connection_state.set(ConnectionState::Connected);
                                    set_connection_generation.update(|connection_generation| {
                                        *connection_generation += 1
                                    });
                                    reconnect_attempts.set_value(0);
                                    clear_timeout(reconnect_timeout);
                                }
                                InitialSubscriptionAdmission::Rejected => {
                                    awaiting_initial_subscription.set_value(false);
                                    clear_timeout(initial_subscription_timeout);
                                    set_connection_state.set(ConnectionState::Error);
                                    schedule_reconnect(
                                        reconnect_attempts,
                                        reconnect_timeout,
                                        connect,
                                    );
                                    if let Some(connection) = ws_handle.get_value() {
                                        let _ = connection.close();
                                    }
                                    return;
                                }
                                InitialSubscriptionAdmission::Pending => {}
                            }
                        }
                        if is_resync_required(&msg) {
                            schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
                            if let Some(connection) = ws_handle.get_value() {
                                let _ = connection.close();
                            }
                            return;
                        }
                        if msg.get("type").and_then(serde_json::Value::as_str) == Some("hello") {
                            event_preview_decoder
                                .borrow_mut()
                                .apply_hello_capabilities(&msg);
                            schedule_preview_expiry(
                                &event_preview_decoder,
                                &event_preview_expiry_timeout,
                            );
                            set_interactive_preview_available
                                .set(interactive_preview_supported(&msg));
                        }
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
                }
            });

            let event_gate = ConnectionEventGate::new(socket_generation, generation, process_event);
            let event_handler = event_gate.handler();
            let request = WebSocketConnectRequest {
                path: "/api/v1/ws".to_owned(),
                protocol: HYPERCOLOR_WS_PROTOCOL.to_owned(),
            };
            let connection = match connect_websocket(request, event_handler) {
                Ok(connection) => connection,
                Err(_) => {
                    socket_generation.set_value(generation.wrapping_add(1));
                    set_connection_state.set(ConnectionState::Error);
                    schedule_reconnect(reconnect_attempts, reconnect_timeout, connect);
                    return;
                }
            };
            ws_handle.set_value(Some(connection));
            event_gate.activate();
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
                    send_canvas_unsubscribe(ws.as_ref());
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_preview_subscription(
                    ws.as_ref(),
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
                    send_screen_canvas_unsubscribe(ws.as_ref());
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_screen_preview_subscription(
                    ws.as_ref(),
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
                    send_screen_zones_subscribe(ws.as_ref());
                    screen_zones_requested.set_value(true);
                }
            } else if screen_zones_requested.get_value() {
                if let Some(ws) = ws_handle.get_value() {
                    send_screen_zones_unsubscribe(ws.as_ref());
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
                    send_web_viewport_canvas_unsubscribe(ws.as_ref());
                }
                return;
            }

            if let Some(ws) = ws_handle.get_value() {
                request_web_viewport_preview_subscription(
                    ws.as_ref(),
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
                        "config": { "interval_ms": 500 }
                    }]
                });
                let _ = send_json(ws.as_ref(), &msg);
                device_metrics_requested.set_value(true);
            } else if !want && have {
                let msg = serde_json::json!({
                    "type": "unsubscribe",
                    "topics": [{ "topic": "device_metrics" }]
                });
                let _ = send_json(ws.as_ref(), &msg);
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
                super::preview::send_display_preview_unsubscribe(ws.as_ref(), &previous);
                set_display_preview_frames.update(|frames| {
                    frames.remove(&previous);
                });
            }
            if let Some(device_id) = wanted.as_deref() {
                super::preview::send_display_preview_subscribe(ws.as_ref(), device_id, 15);
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
                    super::preview::send_zone_layout_preview(ws.as_ref(), &zone_id, &layout);
                }
            });
        let clear_zone_layout_preview = Callback::new(move |zone_id: String| {
            if let Some(ws) = ws_handle.get_value() {
                super::preview::send_zone_layout_preview_clear(ws.as_ref(), &zone_id);
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
                super::interactive_preview::send_open(ws.as_ref(), &request);
            }
        });
        let close_interactive_preview = Callback::new(move |preview_id: String| {
            interactive_preview_tracker.update_value(|tracker| tracker.request_close(&preview_id));
            set_interactive_preview_lifecycles.set(
                interactive_preview_tracker
                    .with_value(InteractivePreviewLifecycleTracker::lifecycles),
            );
            if let Some(ws) = ws_handle.get_value() {
                super::interactive_preview::send_close(ws.as_ref(), &preview_id);
            }
            set_interactive_preview_frames.update(|frames| {
                frames.remove(&preview_id);
            });
        });
        let send_input_inject = Callback::new(
            move |(preview_id, events): (String, Vec<InputInjectEdge>)| {
                if let Some(ws) = ws_handle.get_value() {
                    super::interactive_preview::send_input(ws.as_ref(), &preview_id, &events);
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
    if reconnect_timeout.with_value(Option::is_some) {
        return;
    }
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
    ws_handle: StoredValue<Option<Rc<dyn WebSocketConnection>>, LocalStorage>,
    socket_generation: StoredValue<u64>,
) {
    socket_generation.set_value(socket_generation.get_value().wrapping_add(1));
    if let Some(existing_connection) = ws_handle.get_value() {
        let _ = existing_connection.close();
    }
    ws_handle.set_value(None);
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
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use leptos::prelude::{GetValue, LocalStorage, Owner, SetValue, StoredValue};

    use super::{
        ConnectionEventGate, WebSocketConnection, WebSocketEvent, WebSocketMessage,
        dispose_existing_socket,
    };
    use crate::ws::transport::WebSocketTransportError;

    fn closed_event() -> WebSocketEvent {
        WebSocketEvent::Closed {
            code: 1006,
            reason: "transport closed".to_owned(),
        }
    }

    #[test]
    fn connection_gate_buffers_synchronous_connect_events_in_order() {
        Owner::new().with(|| {
            let socket_generation = StoredValue::new(4_u64);
            let received = Rc::new(RefCell::new(Vec::new()));
            let received_for_process = Rc::clone(&received);
            let gate = ConnectionEventGate::new(
                socket_generation,
                4,
                Rc::new(move |event| received_for_process.borrow_mut().push(event)),
            );
            let handler = gate.handler();

            handler(WebSocketEvent::Opened);
            handler(WebSocketEvent::Message(WebSocketMessage::Text(
                r#"{"type":"hello"}"#.to_owned(),
            )));
            assert!(received.borrow().is_empty());

            gate.activate();
            assert_eq!(
                received.borrow().as_slice(),
                [
                    WebSocketEvent::Opened,
                    WebSocketEvent::Message(WebSocketMessage::Text(
                        r#"{"type":"hello"}"#.to_owned()
                    )),
                ]
            );
        });
    }

    #[test]
    fn connection_gate_ignores_stale_callbacks() {
        Owner::new().with(|| {
            let socket_generation = StoredValue::new(7_u64);
            let received = Rc::new(RefCell::new(Vec::new()));
            let received_for_process = Rc::clone(&received);
            let gate = ConnectionEventGate::new(
                socket_generation,
                7,
                Rc::new(move |event| received_for_process.borrow_mut().push(event)),
            );
            gate.activate();
            let handler = gate.handler();

            socket_generation.set_value(8);
            handler(WebSocketEvent::Opened);
            handler(closed_event());

            assert!(received.borrow().is_empty());
        });
    }

    #[test]
    fn connection_gate_preserves_closed_cleanup_after_error() {
        Owner::new().with(|| {
            let socket_generation = StoredValue::new(11_u64);
            let received = Rc::new(RefCell::new(Vec::new()));
            let received_for_process = Rc::clone(&received);
            let gate = ConnectionEventGate::new(
                socket_generation,
                11,
                Rc::new(move |event| received_for_process.borrow_mut().push(event)),
            );
            gate.activate();
            let handler = gate.handler();

            handler(WebSocketEvent::Error {
                message: "transport failed".to_owned(),
            });
            assert_eq!(received.borrow().len(), 1);
            assert_eq!(socket_generation.get_value(), 11);

            handler(closed_event());
            assert_eq!(received.borrow().len(), 2);
            assert_eq!(socket_generation.get_value(), 12);

            handler(WebSocketEvent::Error {
                message: "late transport failure".to_owned(),
            });
            assert_eq!(received.borrow().len(), 2);
        });
    }

    #[test]
    fn connection_gate_fences_error_after_closed() {
        Owner::new().with(|| {
            let socket_generation = StoredValue::new(14_u64);
            let received = Rc::new(RefCell::new(Vec::new()));
            let received_for_process = Rc::clone(&received);
            let gate = ConnectionEventGate::new(
                socket_generation,
                14,
                Rc::new(move |event| received_for_process.borrow_mut().push(event)),
            );
            gate.activate();
            let handler = gate.handler();

            handler(closed_event());
            handler(WebSocketEvent::Error {
                message: "late transport failure".to_owned(),
            });

            assert_eq!(received.borrow().as_slice(), [closed_event()]);
            assert_eq!(socket_generation.get_value(), 15);
        });
    }

    struct SynchronousCloseConnection {
        handler: Rc<dyn Fn(WebSocketEvent)>,
        close_count: Rc<Cell<u32>>,
    }

    impl WebSocketConnection for SynchronousCloseConnection {
        fn send(&self, _message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
            Ok(())
        }

        fn close(&self) -> Result<(), WebSocketTransportError> {
            self.close_count.set(self.close_count.get() + 1);
            (self.handler)(closed_event());
            Ok(())
        }
    }

    #[test]
    fn disposal_fences_a_synchronous_close_callback() {
        Owner::new().with(|| {
            let socket_generation = StoredValue::new(21_u64);
            let received = Rc::new(RefCell::new(Vec::new()));
            let received_for_process = Rc::clone(&received);
            let gate = ConnectionEventGate::new(
                socket_generation,
                21,
                Rc::new(move |event| received_for_process.borrow_mut().push(event)),
            );
            gate.activate();
            let close_count = Rc::new(Cell::new(0));
            let connection: Rc<dyn WebSocketConnection> = Rc::new(SynchronousCloseConnection {
                handler: gate.handler(),
                close_count: Rc::clone(&close_count),
            });
            let ws_handle: StoredValue<Option<Rc<dyn WebSocketConnection>>, LocalStorage> =
                StoredValue::new_local(Some(connection));

            dispose_existing_socket(ws_handle, socket_generation);

            assert_eq!(close_count.get(), 1);
            assert!(received.borrow().is_empty());
            assert_eq!(socket_generation.get_value(), 22);
            assert!(ws_handle.get_value().is_none());
        });
    }
}
