//! WebSocket handler — `/api/v1/ws`.
//!
//! Real-time event stream, binary frame data, and bidirectional commands.
//! Each connected client gets its own broadcast subscription with configurable
//! channel filtering. Backpressure is handled by bounded channels — slow
//! consumers get dropped frames rather than unbounded memory growth.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, State, WebSocketUpgrade};
use axum::http::{Method, Request, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{RwLock, broadcast, watch};
use tower::ServiceExt;
use tracing::{debug, warn};

use crate::api::AppState;
use crate::api::security::RequestAuthContext;
use crate::performance::FrameTimeSummary as RenderFrameTimeSummary;
use hypercolor_types::server::ServerIdentity;

/// Maximum number of events that can be buffered per WebSocket client.
const WS_BUFFER_SIZE: usize = 64;
const WS_PROTOCOL_VERSION: &str = "1.0";
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
const WS_PONG_TIMEOUT: Duration = Duration::from_secs(10);
const WS_METRICS_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WS_CANVAS_BYTES_PER_PIXEL_RGBA: u64 = 4;
const WS_CANVAS_HEADER: u8 = 0x03;
const WS_SCREEN_CANVAS_HEADER: u8 = 0x05;

static WS_CLIENT_COUNT: AtomicUsize = AtomicUsize::new(0);
static WS_TOTAL_BYTES_SENT: AtomicU64 = AtomicU64::new(0);

struct WsClientGuard;

impl WsClientGuard {
    fn register() -> Self {
        WS_CLIENT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for WsClientGuard {
    fn drop(&mut self) {
        WS_CLIENT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

// ── Subscription Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WsChannel {
    Frames,
    Spectrum,
    Events,
    Canvas,
    ScreenCanvas,
    Metrics,
}

impl WsChannel {
    const SUPPORTED: [Self; 6] = [
        Self::Frames,
        Self::Spectrum,
        Self::Events,
        Self::Canvas,
        Self::ScreenCanvas,
        Self::Metrics,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Spectrum => "spectrum",
            Self::Events => "events",
            Self::Canvas => "canvas",
            Self::ScreenCanvas => "screen_canvas",
            Self::Metrics => "metrics",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "frames" => Some(Self::Frames),
            "spectrum" => Some(Self::Spectrum),
            "events" => Some(Self::Events),
            "canvas" => Some(Self::Canvas),
            "screen_canvas" => Some(Self::ScreenCanvas),
            "metrics" => Some(Self::Metrics),
            _ => None,
        }
    }

    fn is_supported(self) -> bool {
        Self::SUPPORTED.contains(&self)
    }
}

#[derive(Debug, Clone)]
struct SubscriptionState {
    channels: HashSet<WsChannel>,
    config: ChannelConfig,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        let mut channels = HashSet::new();
        channels.insert(WsChannel::Events);
        Self {
            channels,
            config: ChannelConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
struct ChannelConfig {
    frames: FramesConfig,
    spectrum: SpectrumConfig,
    canvas: CanvasConfig,
    screen_canvas: CanvasConfig,
    metrics: MetricsConfig,
}

impl ChannelConfig {
    fn apply_patch(&mut self, patch: ChannelConfigPatch) -> Result<(), WsProtocolError> {
        if let Some(frames) = patch.frames {
            if let Some(fps) = frames.fps {
                validate_range(fps, 1, 60, "config.frames.fps", "expected 1..=60")?;
                self.frames.fps = fps;
            }
            if let Some(format) = frames.format {
                self.frames.format = format;
            }
            if let Some(zones) = frames.zones {
                if zones.is_empty() {
                    return Err(WsProtocolError::invalid_config(
                        "config.frames.zones",
                        "must not be empty",
                    ));
                }
                self.frames.zones = zones;
            }
        }

        if let Some(spectrum) = patch.spectrum {
            if let Some(fps) = spectrum.fps {
                validate_range(fps, 1, 60, "config.spectrum.fps", "expected 1..=60")?;
                self.spectrum.fps = fps;
            }
            if let Some(bins) = spectrum.bins {
                if ![8, 16, 32, 64, 128].contains(&bins) {
                    return Err(WsProtocolError::invalid_config(
                        "config.spectrum.bins",
                        "expected one of [8, 16, 32, 64, 128]",
                    ));
                }
                self.spectrum.bins = bins;
            }
        }

        if let Some(canvas) = patch.canvas {
            if let Some(fps) = canvas.fps {
                validate_range(fps, 1, 60, "config.canvas.fps", "expected 1..=60")?;
                self.canvas.fps = fps;
            }
            if let Some(format) = canvas.format {
                self.canvas.format = format;
            }
        }

        if let Some(screen_canvas) = patch.screen_canvas {
            if let Some(fps) = screen_canvas.fps {
                validate_range(fps, 1, 60, "config.screen_canvas.fps", "expected 1..=60")?;
                self.screen_canvas.fps = fps;
            }
            if let Some(format) = screen_canvas.format {
                self.screen_canvas.format = format;
            }
        }

        if let Some(metrics) = patch.metrics
            && let Some(interval_ms) = metrics.interval_ms
        {
            validate_range(
                interval_ms,
                100,
                10_000,
                "config.metrics.interval_ms",
                "expected 100..=10000",
            )?;
            self.metrics.interval_ms = interval_ms;
        }

        Ok(())
    }

    fn filtered_json(&self, channels: &HashSet<WsChannel>) -> serde_json::Value {
        let mut map = serde_json::Map::new();

        for channel in channels {
            let value = match channel {
                WsChannel::Frames => serde_json::to_value(&self.frames),
                WsChannel::Spectrum => serde_json::to_value(&self.spectrum),
                WsChannel::Canvas => serde_json::to_value(&self.canvas),
                WsChannel::ScreenCanvas => serde_json::to_value(&self.screen_canvas),
                WsChannel::Metrics => serde_json::to_value(&self.metrics),
                WsChannel::Events => continue,
            };

            if let Ok(json_value) = value {
                map.insert(channel.as_str().to_owned(), json_value);
            }
        }

        serde_json::Value::Object(map)
    }
}

#[derive(Debug, Clone, Serialize)]
struct FramesConfig {
    fps: u32,
    format: FrameFormat,
    zones: Vec<String>,
}

impl Default for FramesConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            format: FrameFormat::Binary,
            zones: vec!["all".to_owned()],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SpectrumConfig {
    fps: u32,
    bins: u16,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self { fps: 30, bins: 64 }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CanvasConfig {
    fps: u32,
    format: CanvasFormat,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            fps: 15,
            format: CanvasFormat::Rgb,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct MetricsConfig {
    interval_ms: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrameFormat {
    Binary,
    Json,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CanvasFormat {
    Rgb,
    Rgba,
}

/// Client-to-server subscription messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    /// Subscribe to one or more channels.
    Subscribe {
        channels: Vec<String>,
        #[serde(default)]
        config: Option<ChannelConfigPatch>,
    },
    /// Unsubscribe from one or more channels.
    Unsubscribe { channels: Vec<String> },
    /// REST-equivalent command execution over WS.
    Command {
        id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct ChannelConfigPatch {
    #[serde(default)]
    frames: Option<FramesConfigPatch>,
    #[serde(default)]
    spectrum: Option<SpectrumConfigPatch>,
    #[serde(default)]
    canvas: Option<CanvasConfigPatch>,
    #[serde(default)]
    screen_canvas: Option<CanvasConfigPatch>,
    #[serde(default)]
    metrics: Option<MetricsConfigPatch>,
}

#[derive(Debug, Deserialize)]
struct FramesConfigPatch {
    #[serde(default)]
    fps: Option<u32>,
    #[serde(default)]
    format: Option<FrameFormat>,
    #[serde(default)]
    zones: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct SpectrumConfigPatch {
    #[serde(default)]
    fps: Option<u32>,
    #[serde(default)]
    bins: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct CanvasConfigPatch {
    #[serde(default)]
    fps: Option<u32>,
    #[serde(default)]
    format: Option<CanvasFormat>,
}

#[derive(Debug, Deserialize)]
struct MetricsConfigPatch {
    #[serde(default)]
    interval_ms: Option<u32>,
}

/// Server-to-client acknowledgment messages.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    /// Initial hello with state snapshot.
    Hello {
        version: String,
        server: ServerIdentity,
        state: HelloState,
        capabilities: Vec<String>,
        subscriptions: Vec<String>,
    },
    /// Subscribe acknowledgment.
    Subscribed {
        channels: Vec<String>,
        config: serde_json::Value,
    },
    /// Unsubscribe acknowledgment.
    Unsubscribed {
        channels: Vec<String>,
        remaining: Vec<String>,
    },
    /// Event relay from the bus.
    Event {
        event: String,
        timestamp: String,
        data: serde_json::Value,
    },
    /// Periodic performance metrics snapshot.
    Metrics {
        timestamp: String,
        data: MetricsPayload,
    },
    /// Backpressure warning for dropped binary channel payloads.
    Backpressure {
        dropped_frames: u32,
        channel: String,
        recommendation: String,
        suggested_fps: u32,
    },
    /// Protocol-level request error.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    /// Command response envelope for WS command execution.
    Response {
        id: String,
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<serde_json::Value>,
    },
}

#[derive(Debug, Serialize)]
struct HelloState {
    running: bool,
    paused: bool,
    brightness: u8,
    fps: HelloFps,
    effect: Option<NameRef>,
    profile: Option<NameRef>,
    layout: Option<NameRef>,
    device_count: usize,
    total_leds: usize,
}

#[derive(Debug, Serialize)]
struct HelloFps {
    target: u32,
    actual: f64,
}

#[derive(Debug, Serialize)]
struct NameRef {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct MetricsPayload {
    fps: MetricsFps,
    frame_time: MetricsFrameTime,
    stages: MetricsStages,
    pacing: MetricsPacing,
    copies: MetricsCopies,
    memory: MetricsMemory,
    devices: MetricsDevices,
    websocket: MetricsWebsocket,
}

#[derive(Debug, Serialize)]
struct MetricsFps {
    target: u32,
    actual: f64,
    dropped: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
struct MetricsFrameTime {
    avg_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
struct MetricsStages {
    input_sampling_ms: f64,
    effect_rendering_ms: f64,
    spatial_sampling_ms: f64,
    device_output_ms: f64,
    preview_postprocess_ms: f64,
    event_bus_ms: f64,
    coordination_overhead_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
struct MetricsPacing {
    jitter_avg_ms: f64,
    jitter_p95_ms: f64,
    jitter_max_ms: f64,
    wake_delay_avg_ms: f64,
    wake_delay_p95_ms: f64,
    wake_delay_max_ms: f64,
    frame_age_ms: f64,
    reused_inputs: u32,
    reused_canvas: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
struct MetricsCopies {
    full_frame_count: u32,
    full_frame_kb: f64,
}

#[derive(Debug, Serialize)]
struct MetricsMemory {
    daemon_rss_mb: f64,
    servo_rss_mb: f64,
    canvas_buffer_kb: u32,
}

#[derive(Debug, Serialize)]
struct MetricsDevices {
    connected: usize,
    total_leds: usize,
    output_errors: u32,
}

#[derive(Debug, Serialize)]
struct MetricsWebsocket {
    client_count: usize,
    bytes_sent_per_sec: f64,
}

#[derive(Debug)]
struct WsProtocolError {
    code: &'static str,
    message: String,
    details: Option<serde_json::Value>,
}

impl WsProtocolError {
    fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request",
            message: message.into(),
            details: None,
        }
    }

    fn invalid_config(field: &'static str, message: &'static str) -> Self {
        Self {
            code: "invalid_config",
            message: format!("Invalid configuration for {field}: {message}"),
            details: Some(json!({"field": field, "reason": message})),
        }
    }

    fn unsupported_channel(channel: &str) -> Self {
        Self {
            code: "unsupported_channel",
            message: format!("Channel '{channel}' is not supported by this server"),
            details: Some(json!({"channel": channel})),
        }
    }

    fn into_message(self) -> ServerMessage {
        ServerMessage::Error {
            code: self.code.to_owned(),
            message: self.message,
            details: self.details,
        }
    }
}

// ── Handler ──────────────────────────────────────────────────────────────

/// `GET /api/v1/ws` — Upgrade to WebSocket.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> Response {
    let auth_context =
        auth_context.map_or_else(RequestAuthContext::unsecured, |Extension(context)| context);
    ws.protocols(["hypercolor-v1"])
        .on_upgrade(move |socket| handle_socket(socket, state, auth_context))
}

/// Process a single WebSocket connection.
#[expect(
    clippy::too_many_lines,
    reason = "Socket loop coordinates handshake, heartbeats, relay queues, and client messages"
)]
async fn handle_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    auth_context: RequestAuthContext,
) {
    let _client_guard = WsClientGuard::register();

    // Default subscriptions: events only.
    // Wrapped in Arc<RwLock<_>> so the relay task sees subscription changes.
    let subscriptions = Arc::new(RwLock::new(SubscriptionState::default()));

    // Send hello message.
    let hello = {
        let subs = subscriptions.read().await;
        ServerMessage::Hello {
            version: WS_PROTOCOL_VERSION.to_owned(),
            server: state.server_identity.clone(),
            state: build_hello_state(&state).await,
            capabilities: ws_capabilities(),
            subscriptions: sorted_channel_names(&subs.channels),
        }
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // Subscribe to the event bus and watch channels.
    let event_rx = state.event_bus.subscribe_all();
    let frame_rx = state.event_bus.frame_receiver();
    let spectrum_rx = state.event_bus.spectrum_receiver();
    let canvas_rx = state.event_bus.canvas_receiver();
    let screen_canvas_rx = state.event_bus.screen_canvas_receiver();

    // Split outbound traffic: both queues are bounded so slow clients cannot
    // grow daemon memory without limit.
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<String>(WS_BUFFER_SIZE);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(WS_BUFFER_SIZE);

    // Spawn event relay task — shares the subscription set via Arc<RwLock<_>>.
    let relay_subs = Arc::clone(&subscriptions);
    let relay_handle = tokio::spawn(relay_events(event_rx, json_tx.clone(), relay_subs));
    let frame_subs = Arc::clone(&subscriptions);
    let frame_relay_handle = tokio::spawn(relay_frames(
        frame_rx,
        json_tx.clone(),
        binary_tx.clone(),
        frame_subs,
    ));
    let spectrum_subs = Arc::clone(&subscriptions);
    let spectrum_relay_handle = tokio::spawn(relay_spectrum(
        spectrum_rx,
        json_tx.clone(),
        binary_tx.clone(),
        spectrum_subs,
    ));
    let canvas_subs = Arc::clone(&subscriptions);
    let canvas_relay_handle = tokio::spawn(relay_canvas(
        canvas_rx,
        json_tx.clone(),
        binary_tx.clone(),
        canvas_subs,
    ));
    let screen_canvas_subs = Arc::clone(&subscriptions);
    let screen_canvas_relay_handle = tokio::spawn(relay_screen_canvas(
        screen_canvas_rx,
        json_tx.clone(),
        binary_tx.clone(),
        screen_canvas_subs,
    ));
    let metrics_subs = Arc::clone(&subscriptions);
    let metrics_relay_handle =
        tokio::spawn(relay_metrics(Arc::clone(&state), json_tx, metrics_subs));

    let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut awaiting_pong = false;
    let mut ping_sent_at = Instant::now();

    // Main loop: multiplex between incoming client messages and outbound events.
    loop {
        tokio::select! {
            biased;

            // Outbound JSON: bounded queue (drop under pressure in producer tasks).
            json_msg = json_rx.recv() => {
                match json_msg {
                    Some(msg) => {
                        let sent_len = msg.len();
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                        track_ws_bytes_sent(sent_len);
                    }
                    None => break,
                }
            }

            // Outbound binary: bounded queue (drop under pressure in producer tasks).
            binary_msg = binary_rx.recv() => {
                match binary_msg {
                    Some(bytes) => {
                        let sent_len = bytes.len();
                        if socket.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                        track_ws_bytes_sent(sent_len);
                    }
                    None => break,
                }
            }

            // Keepalive heartbeat.
            _ = ping_interval.tick() => {
                if awaiting_pong && ping_sent_at.elapsed() >= WS_PONG_TIMEOUT {
                    warn!("WebSocket client timed out waiting for pong");
                    break;
                }

                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
                awaiting_pong = true;
                ping_sent_at = Instant::now();
            }

            // Inbound: process client messages.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(
                            &text,
                            &state,
                            auth_context,
                            &subscriptions,
                            &mut socket,
                        )
                        .await;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        awaiting_pong = false;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("WebSocket recv error: {e}");
                        break;
                    }
                    _ => {} // Ignore binary/ping/pong for now.
                }
            }
        }
    }

    relay_handle.abort();
    frame_relay_handle.abort();
    spectrum_relay_handle.abort();
    canvas_relay_handle.abort();
    screen_canvas_relay_handle.abort();
    metrics_relay_handle.abort();
    debug!("WebSocket client disconnected");
}

/// Relay events from the broadcast bus to a bounded mpsc channel.
/// Drops events when the consumer is slow (backpressure).
async fn relay_events(
    mut event_rx: broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    loop {
        match event_rx.recv().await {
            Ok(timestamped) => {
                let should_relay = {
                    let subs = subscriptions.read().await;
                    should_relay_event(&timestamped.event, &subs.channels)
                };
                if !should_relay {
                    continue;
                }

                let (event_name, event_data) = event_message_parts(&timestamped.event);
                let msg = ServerMessage::Event {
                    event: event_name,
                    timestamp: timestamped.timestamp.to_string(),
                    data: event_data,
                };
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };

                let _ = try_enqueue_json(&json_tx, json, "events");
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket consumer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn should_relay_event(
    event: &hypercolor_types::event::HypercolorEvent,
    channels: &HashSet<WsChannel>,
) -> bool {
    if !channels.contains(&WsChannel::Events) {
        return false;
    }

    if matches!(
        event,
        hypercolor_types::event::HypercolorEvent::FrameRendered { .. }
    ) && (channels.contains(&WsChannel::Frames)
        || channels.contains(&WsChannel::Canvas)
        || channels.contains(&WsChannel::ScreenCanvas)
        || channels.contains(&WsChannel::Metrics))
    {
        return false;
    }

    true
}

enum FrameRelayMessage {
    Json(String),
    Binary(Vec<u8>),
}

/// Relay frame watch updates to the WebSocket client.
async fn relay_frames(
    mut frame_rx: watch::Receiver<hypercolor_types::event::FrameData>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    binary_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    let mut last_sent = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);

    loop {
        if frame_rx.changed().await.is_err() {
            break;
        }

        let frame_config = {
            let subs = subscriptions.read().await;
            if !subs.channels.contains(&WsChannel::Frames) {
                continue;
            }
            subs.config.frames.clone()
        };

        if !should_emit(&mut last_sent, frame_config.fps) {
            continue;
        }

        let frame = frame_rx.borrow();
        let zones = filter_frame_zones(&frame.zones, &frame_config.zones);
        let outbound = match frame_config.format {
            FrameFormat::Binary => {
                let filtered_frame = hypercolor_types::event::FrameData {
                    frame_number: frame.frame_number,
                    timestamp_ms: frame.timestamp_ms,
                    zones,
                };
                FrameRelayMessage::Binary(encode_frame_binary(&filtered_frame))
            }
            FrameFormat::Json => {
                let zones: Vec<serde_json::Value> = zones
                    .iter()
                    .map(|zone| {
                        json!({
                            "zone_id": zone.zone_id,
                            "colors": zone.colors,
                        })
                    })
                    .collect();
                let json_frame = json!({
                    "type": "frame",
                    "frame_number": frame.frame_number,
                    "timestamp_ms": frame.timestamp_ms,
                    "zones": zones,
                });
                let Ok(text) = serde_json::to_string(&json_frame) else {
                    continue;
                };
                FrameRelayMessage::Json(text)
            }
        };

        match outbound {
            FrameRelayMessage::Json(text) => {
                let _ = try_enqueue_json(&json_tx, text, "frames");
            }
            FrameRelayMessage::Binary(bytes) => {
                if binary_tx.try_send(bytes).is_err() {
                    enqueue_backpressure_notice(&json_tx, "frames", frame_config.fps);
                    debug!("Dropping binary frame update for slow WebSocket consumer");
                }
            }
        }
    }
}

/// Relay spectrum watch updates to the WebSocket client.
async fn relay_spectrum(
    mut spectrum_rx: watch::Receiver<hypercolor_types::event::SpectrumData>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    binary_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    let mut last_sent = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);

    loop {
        if spectrum_rx.changed().await.is_err() {
            break;
        }

        let spectrum_config = {
            let subs = subscriptions.read().await;
            if !subs.channels.contains(&WsChannel::Spectrum) {
                continue;
            }
            subs.config.spectrum.clone()
        };

        if !should_emit(&mut last_sent, spectrum_config.fps) {
            continue;
        }

        let spectrum = spectrum_rx.borrow();
        if binary_tx
            .try_send(encode_spectrum_binary(&spectrum, spectrum_config.bins))
            .is_err()
        {
            enqueue_backpressure_notice(&json_tx, "spectrum", spectrum_config.fps);
            debug!("Dropping binary spectrum update for slow WebSocket consumer");
        }
    }
}

/// Relay raw canvas updates to the WebSocket client.
async fn relay_canvas(
    mut canvas_rx: watch::Receiver<hypercolor_core::bus::CanvasFrame>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    binary_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    let mut latest_canvas = canvas_rx.borrow().clone();
    let mut last_sent_frame_number: Option<u32> = None;
    let mut active_fps = 15_u32;
    let mut ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(active_fps)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let canvas_config = {
            let subs = subscriptions.read().await;
            if subs.channels.contains(&WsChannel::Canvas) {
                Some(subs.config.canvas.clone())
            } else {
                None
            }
        };

        let Some(canvas_config) = canvas_config else {
            if canvas_rx.changed().await.is_err() {
                break;
            }
            latest_canvas = canvas_rx.borrow().clone();
            continue;
        };

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
            ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(active_fps)));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        }

        tokio::select! {
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                latest_canvas = canvas_rx.borrow().clone();
            }
            _ = ticker.tick() => {
                if last_sent_frame_number == Some(latest_canvas.frame_number) {
                    continue;
                }

                if binary_tx
                    .try_send(encode_canvas_binary(&latest_canvas, canvas_config.format))
                    .is_err()
                {
                    enqueue_backpressure_notice(&json_tx, "canvas", canvas_config.fps);
                    debug!("Dropping binary canvas update for slow WebSocket consumer");
                    continue;
                }

                last_sent_frame_number = Some(latest_canvas.frame_number);
            }
        }
    }
}

/// Relay raw screen-source canvas updates to the WebSocket client.
async fn relay_screen_canvas(
    mut canvas_rx: watch::Receiver<hypercolor_core::bus::CanvasFrame>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    binary_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    let mut latest_canvas = canvas_rx.borrow().clone();
    let mut last_sent_frame_number: Option<u32> = None;
    let mut active_fps = 15_u32;
    let mut ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(active_fps)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let canvas_config = {
            let subs = subscriptions.read().await;
            if subs.channels.contains(&WsChannel::ScreenCanvas) {
                Some(subs.config.screen_canvas.clone())
            } else {
                None
            }
        };

        let Some(canvas_config) = canvas_config else {
            if canvas_rx.changed().await.is_err() {
                break;
            }
            latest_canvas = canvas_rx.borrow().clone();
            continue;
        };

        if canvas_config.fps != active_fps {
            active_fps = canvas_config.fps.max(1);
            ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(active_fps)));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        }

        tokio::select! {
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                latest_canvas = canvas_rx.borrow().clone();
            }
            _ = ticker.tick() => {
                if last_sent_frame_number == Some(latest_canvas.frame_number) {
                    continue;
                }

                if binary_tx
                    .try_send(encode_canvas_binary_with_header(
                        &latest_canvas,
                        canvas_config.format,
                        WS_SCREEN_CANVAS_HEADER,
                    ))
                    .is_err()
                {
                    enqueue_backpressure_notice(&json_tx, "screen_canvas", canvas_config.fps);
                    debug!("Dropping binary screen_canvas update for slow WebSocket consumer");
                    continue;
                }

                last_sent_frame_number = Some(latest_canvas.frame_number);
            }
        }
    }
}

/// Relay periodic metrics snapshots to the WebSocket client.
async fn relay_metrics(
    state: Arc<AppState>,
    json_tx: tokio::sync::mpsc::Sender<String>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    let mut last_total_bytes = WS_TOTAL_BYTES_SENT.load(Ordering::Relaxed);

    loop {
        let interval_ms = {
            let subs = subscriptions.read().await;
            if subs.channels.contains(&WsChannel::Metrics) {
                Some(subs.config.metrics.interval_ms)
            } else {
                None
            }
        };

        let Some(interval_ms) = interval_ms else {
            tokio::time::sleep(WS_METRICS_IDLE_POLL_INTERVAL).await;
            continue;
        };

        tokio::time::sleep(Duration::from_millis(u64::from(interval_ms))).await;

        let still_subscribed = {
            let subs = subscriptions.read().await;
            subs.channels.contains(&WsChannel::Metrics)
        };
        if !still_subscribed {
            continue;
        }

        let total_bytes = WS_TOTAL_BYTES_SENT.load(Ordering::Relaxed);
        let delta_bytes = total_bytes.saturating_sub(last_total_bytes);
        last_total_bytes = total_bytes;
        let interval_secs = f64::from(interval_ms) / 1000.0;
        let bytes_per_sec = if interval_secs > 0.0 {
            let delta_u32 = u32::try_from(delta_bytes).unwrap_or(u32::MAX);
            f64::from(delta_u32) / interval_secs
        } else {
            0.0
        };

        let message = build_metrics_message(&state, bytes_per_sec).await;
        if let Ok(text) = serde_json::to_string(&message) {
            let _ = try_enqueue_json(&json_tx, text, "metrics");
        }
    }
}

fn try_enqueue_json(
    json_tx: &tokio::sync::mpsc::Sender<String>,
    text: String,
    stream: &str,
) -> bool {
    match json_tx.try_send(text) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            debug!(
                stream,
                "Dropping queued WebSocket JSON message for slow consumer"
            );
            false
        }
        Err(TrySendError::Closed(_)) => false,
    }
}

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    subscriptions: &Arc<RwLock<SubscriptionState>>,
    socket: &mut WebSocket,
) {
    let msg = match serde_json::from_str::<ClientMessage>(text) {
        Ok(msg) => msg,
        Err(error) => {
            let _ = send_json(
                socket,
                &WsProtocolError::invalid_request(format!("Invalid JSON message: {error}"))
                    .into_message(),
            )
            .await;
            return;
        }
    };

    match msg {
        ClientMessage::Subscribe { channels, config } => {
            let parsed_channels = match parse_channels(&channels) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            let mut subs = subscriptions.write().await;

            if let Some(config_patch) = config
                && let Err(error) = subs.config.apply_patch(config_patch)
            {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            for channel in &parsed_channels {
                subs.channels.insert(*channel);
            }

            let ack = ServerMessage::Subscribed {
                channels: unique_sorted_channel_names(&parsed_channels),
                config: subs.config.filtered_json(&subs.channels),
            };
            let _ = send_json(socket, &ack).await;
        }
        ClientMessage::Unsubscribe { channels } => {
            let parsed_channels = match parse_channels(&channels) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            let remaining = {
                let mut subs = subscriptions.write().await;
                for channel in &parsed_channels {
                    subs.channels.remove(channel);
                }
                sorted_channel_names(&subs.channels)
            };

            let ack = ServerMessage::Unsubscribed {
                channels: unique_sorted_channel_names(&parsed_channels),
                remaining,
            };
            let _ = send_json(socket, &ack).await;
        }
        ClientMessage::Command {
            id,
            method,
            path,
            body,
        } => {
            let response = dispatch_command(state, auth_context, id, method, path, body).await;
            let _ = send_json(socket, &response).await;
        }
    }
}

async fn dispatch_command(
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    id: String,
    method_raw: String,
    path_raw: String,
    body: Option<serde_json::Value>,
) -> ServerMessage {
    let method = match parse_command_method(&method_raw) {
        Ok(method) => method,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(error)),
            };
        }
    };
    let path = match normalize_command_path(&path_raw) {
        Ok(path) => path,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(error)),
            };
        }
    };

    let body_bytes = match body {
        Some(payload) => serde_json::to_vec(&payload).unwrap_or_default(),
        None => Vec::new(),
    };

    let mut request_builder = Request::builder().method(method).uri(path);
    if !body_bytes.is_empty() {
        request_builder = request_builder.header(header::CONTENT_TYPE, "application/json");
    }

    let mut request = match request_builder.body(axum::body::Body::from(body_bytes)) {
        Ok(request) => request,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(WsProtocolError::invalid_request(
                    format!("Invalid command request: {error}"),
                ))),
            };
        }
    };
    request.extensions_mut().insert(auth_context);

    let response = crate::api::build_router(Arc::clone(state), None)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {});

    command_response_from_http(id, response).await
}

fn parse_command_method(method_raw: &str) -> Result<Method, WsProtocolError> {
    let method = Method::from_bytes(method_raw.trim().as_bytes()).map_err(|_| {
        WsProtocolError::invalid_request("command.method must be a valid HTTP verb")
    })?;

    if matches!(
        method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        Ok(method)
    } else {
        Err(WsProtocolError::invalid_request(
            "command.method must be one of GET, POST, PUT, PATCH, DELETE",
        ))
    }
}

fn normalize_command_path(path_raw: &str) -> Result<String, WsProtocolError> {
    let path = path_raw.trim();
    if path.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "command.path must not be empty",
        ));
    }
    if !path.starts_with('/') {
        return Err(WsProtocolError::invalid_request(
            "command.path must start with '/'",
        ));
    }
    if path.starts_with("/api/v1") {
        return Ok(path.to_owned());
    }
    Ok(format!("/api/v1{path}"))
}

async fn command_response_from_http(id: String, response: Response) -> ServerMessage {
    let status = response.status().as_u16();
    let body = response.into_body();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes).ok();

    if (200..300).contains(&status) {
        let data = parsed
            .map(|value| value.get("data").cloned().unwrap_or(value))
            .or_else(|| Some(json!({})));
        return ServerMessage::Response {
            id,
            status,
            data,
            error: None,
        };
    }

    let error = parsed
        .and_then(|value| value.get("error").cloned())
        .or_else(|| {
            Some(json!({
                "code": "internal_error",
                "message": format!("Command failed with status {status}"),
            }))
        });
    ServerMessage::Response {
        id,
        status,
        data: None,
        error,
    }
}

fn protocol_error_json(error: WsProtocolError) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "code".to_owned(),
        serde_json::Value::String(error.code.to_owned()),
    );
    payload.insert(
        "message".to_owned(),
        serde_json::Value::String(error.message),
    );
    if let Some(details) = error.details {
        payload.insert("details".to_owned(), details);
    }
    serde_json::Value::Object(payload)
}

fn parse_channels(channels: &[String]) -> Result<Vec<WsChannel>, WsProtocolError> {
    if channels.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "channels must contain at least one channel",
        ));
    }

    let mut parsed = Vec::with_capacity(channels.len());
    for channel in channels {
        let parsed_channel = WsChannel::parse(channel).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Unknown channel '{channel}'"))
        })?;

        if !parsed_channel.is_supported() {
            return Err(WsProtocolError::unsupported_channel(channel));
        }

        parsed.push(parsed_channel);
    }

    Ok(parsed)
}

fn filter_frame_zones(
    zones: &[hypercolor_types::event::ZoneColors],
    selected: &[String],
) -> Vec<hypercolor_types::event::ZoneColors> {
    if selected.iter().any(|zone| zone == "all") {
        return zones.to_vec();
    }

    let selected_set: HashSet<&str> = selected.iter().map(String::as_str).collect();
    zones
        .iter()
        .filter(|zone| selected_set.contains(zone.zone_id.as_str()))
        .cloned()
        .collect()
}

fn validate_range(
    value: u32,
    min: u32,
    max: u32,
    field: &'static str,
    message: &'static str,
) -> Result<(), WsProtocolError> {
    if !(min..=max).contains(&value) {
        return Err(WsProtocolError::invalid_config(field, message));
    }
    Ok(())
}

fn should_emit(last_sent: &mut Instant, fps: u32) -> bool {
    let clamped_fps = fps.max(1);
    let interval = Duration::from_secs_f64(1.0 / f64::from(clamped_fps));
    let now = Instant::now();
    if now.duration_since(*last_sent) < interval {
        return false;
    }
    *last_sent = now;
    true
}

fn track_ws_bytes_sent(sent_len: usize) {
    let sent_u64 = u64::try_from(sent_len).unwrap_or(u64::MAX);
    WS_TOTAL_BYTES_SENT.fetch_add(sent_u64, Ordering::Relaxed);
}

fn encode_frame_binary(frame: &hypercolor_types::event::FrameData) -> Vec<u8> {
    let max_zone_count = usize::from(u8::MAX);
    let included_zones = if frame.zones.len() > max_zone_count {
        &frame.zones[..max_zone_count]
    } else {
        &frame.zones[..]
    };

    let payload_bytes = included_zones.iter().fold(0_usize, |acc, zone| {
        let zone_id_len = zone.zone_id.len().min(usize::from(u16::MAX));
        let led_count = zone.colors.len().min(usize::from(u16::MAX));
        acc.saturating_add(
            2_usize
                .saturating_add(zone_id_len)
                .saturating_add(2)
                .saturating_add(led_count.saturating_mul(3)),
        )
    });
    let mut out = Vec::with_capacity(10_usize.saturating_add(payload_bytes));
    out.push(0x01);
    out.extend_from_slice(&frame.frame_number.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ms.to_le_bytes());
    out.push(u8::try_from(included_zones.len()).unwrap_or(u8::MAX));

    for zone in included_zones {
        let zone_id_bytes = zone.zone_id.as_bytes();
        let zone_id_len_u16 = u16::try_from(zone_id_bytes.len()).unwrap_or(u16::MAX);
        let zone_id_len = usize::from(zone_id_len_u16);
        out.extend_from_slice(&zone_id_len_u16.to_le_bytes());
        out.extend_from_slice(&zone_id_bytes[..zone_id_len]);

        let led_count_u16 = u16::try_from(zone.colors.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&led_count_u16.to_le_bytes());
        let led_count = usize::from(led_count_u16);
        for color in zone.colors.iter().take(led_count) {
            out.extend_from_slice(color);
        }
    }

    out
}

fn encode_spectrum_binary(
    spectrum: &hypercolor_types::event::SpectrumData,
    requested_bins: u16,
) -> Vec<u8> {
    let downsampled = spectrum.downsample(usize::from(requested_bins));
    let bin_count_u8 = u8::try_from(downsampled.len()).unwrap_or(u8::MAX);
    let bin_count = usize::from(bin_count_u8);

    let mut out = Vec::with_capacity(27_usize.saturating_add(bin_count.saturating_mul(4)));
    out.push(0x02);
    out.extend_from_slice(&spectrum.timestamp_ms.to_le_bytes());
    out.push(bin_count_u8);
    out.extend_from_slice(&sanitize_f32(spectrum.level).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.bass).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.mid).to_le_bytes());
    out.extend_from_slice(&sanitize_f32(spectrum.treble).to_le_bytes());
    out.push(u8::from(spectrum.beat));
    out.extend_from_slice(&sanitize_f32(spectrum.beat_confidence).to_le_bytes());

    for value in downsampled.iter().take(bin_count) {
        out.extend_from_slice(&sanitize_f32(*value).to_le_bytes());
    }

    out
}

fn encode_canvas_binary(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
) -> Vec<u8> {
    encode_canvas_binary_with_header(canvas, format, WS_CANVAS_HEADER)
}

fn encode_canvas_binary_with_header(
    canvas: &hypercolor_core::bus::CanvasFrame,
    format: CanvasFormat,
    header: u8,
) -> Vec<u8> {
    let width_u16 = u16::try_from(canvas.width).unwrap_or(u16::MAX);
    let height_u16 = u16::try_from(canvas.height).unwrap_or(u16::MAX);
    let width = usize::from(width_u16);
    let height = usize::from(height_u16);
    let px_count = width.saturating_mul(height);

    let bpp = match format {
        CanvasFormat::Rgb => 3_usize,
        CanvasFormat::Rgba => 4_usize,
    };
    let mut out = Vec::with_capacity(14_usize.saturating_add(px_count.saturating_mul(bpp)));
    out.push(header);
    out.extend_from_slice(&canvas.frame_number.to_le_bytes());
    out.extend_from_slice(&canvas.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&width_u16.to_le_bytes());
    out.extend_from_slice(&height_u16.to_le_bytes());
    out.push(match format {
        CanvasFormat::Rgb => 0,
        CanvasFormat::Rgba => 1,
    });

    let rgba = canvas.rgba_bytes();
    match format {
        CanvasFormat::Rgb => {
            for pixel in rgba.chunks_exact(4).take(px_count) {
                out.extend_from_slice(&pixel[..3]);
            }
        }
        CanvasFormat::Rgba => {
            let max_len = px_count.saturating_mul(4);
            out.extend_from_slice(&rgba[..rgba.len().min(max_len)]);
        }
    }

    out
}

fn sanitize_f32(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

async fn build_metrics_message(state: &AppState, bytes_sent_per_sec: f64) -> ServerMessage {
    let (render_stats, render_elapsed_ms) = {
        let render_loop = state.render_loop.read().await;
        (
            render_loop.stats(),
            render_loop.elapsed().as_secs_f64() * 1000.0,
        )
    };
    let performance_snapshot = state.performance.read().await.snapshot();
    let target_fps = render_stats.tier.fps();
    let avg_frame_secs = render_stats.avg_frame_time.as_secs_f64();
    let actual_fps = paced_fps(avg_frame_secs, target_fps);
    let avg_ms = avg_frame_secs * 1000.0;
    let frame_time = frame_time_summary(performance_snapshot.frame_time, avg_ms);
    let latest_frame = performance_snapshot.latest_frame.unwrap_or_default();
    let frame_age_ms = if latest_frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(latest_frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    let devices = state.device_registry.list().await;
    let total_leds = devices.iter().fold(0_usize, |acc, tracked| {
        let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
        acc.saturating_add(led_count)
    });
    let connected = devices.len();

    let (canvas_width, canvas_height) = {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };
    let canvas_buffer_bytes = u64::from(canvas_width)
        .saturating_mul(u64::from(canvas_height))
        .saturating_mul(WS_CANVAS_BYTES_PER_PIXEL_RGBA);
    let canvas_buffer_kb = u32::try_from(canvas_buffer_bytes / 1024).unwrap_or(u32::MAX);

    let daemon_rss_mb = process_rss_mb().unwrap_or(0.0);
    let client_count = WS_CLIENT_COUNT.load(Ordering::Relaxed);

    ServerMessage::Metrics {
        timestamp: format_iso8601_now(),
        data: MetricsPayload {
            fps: MetricsFps {
                target: target_fps,
                actual: round_1(actual_fps),
                dropped: render_stats.consecutive_misses,
            },
            frame_time: MetricsFrameTime {
                avg_ms: round_2(frame_time.avg_ms),
                p95_ms: round_2(frame_time.p95_ms),
                p99_ms: round_2(frame_time.p99_ms),
                max_ms: round_2(frame_time.max_ms),
            },
            stages: MetricsStages {
                input_sampling_ms: round_2(us_to_ms(latest_frame.input_us)),
                effect_rendering_ms: round_2(us_to_ms(latest_frame.render_us)),
                spatial_sampling_ms: round_2(us_to_ms(latest_frame.sample_us)),
                device_output_ms: round_2(us_to_ms(latest_frame.push_us)),
                preview_postprocess_ms: round_2(us_to_ms(latest_frame.postprocess_us)),
                event_bus_ms: round_2(us_to_ms(latest_frame.publish_us)),
                coordination_overhead_ms: round_2(us_to_ms(latest_frame.overhead_us)),
            },
            pacing: MetricsPacing {
                jitter_avg_ms: round_2(performance_snapshot.pacing.jitter_avg_ms),
                jitter_p95_ms: round_2(performance_snapshot.pacing.jitter_p95_ms),
                jitter_max_ms: round_2(performance_snapshot.pacing.jitter_max_ms),
                wake_delay_avg_ms: round_2(performance_snapshot.pacing.wake_delay_avg_ms),
                wake_delay_p95_ms: round_2(performance_snapshot.pacing.wake_delay_p95_ms),
                wake_delay_max_ms: round_2(performance_snapshot.pacing.wake_delay_max_ms),
                frame_age_ms: round_2(frame_age_ms),
                reused_inputs: performance_snapshot.pacing.reused_inputs,
                reused_canvas: performance_snapshot.pacing.reused_canvas,
            },
            copies: MetricsCopies {
                full_frame_count: latest_frame.full_frame_copy_count,
                full_frame_kb: round_2(bytes_to_kib(latest_frame.full_frame_copy_bytes)),
            },
            memory: MetricsMemory {
                daemon_rss_mb: round_1(daemon_rss_mb),
                servo_rss_mb: 0.0,
                canvas_buffer_kb,
            },
            devices: MetricsDevices {
                connected,
                total_leds,
                output_errors: latest_frame.output_errors,
            },
            websocket: MetricsWebsocket {
                client_count,
                bytes_sent_per_sec: round_1(bytes_sent_per_sec),
            },
        },
    }
}

fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

fn us_to_ms(value: u32) -> f64 {
    f64::from(value) / 1000.0
}

fn bytes_to_kib(value: u32) -> f64 {
    f64::from(value) / 1024.0
}

fn frame_time_summary(
    summary: RenderFrameTimeSummary,
    fallback_avg_ms: f64,
) -> RenderFrameTimeSummary {
    if summary.avg_ms > 0.0 {
        summary
    } else {
        RenderFrameTimeSummary {
            avg_ms: fallback_avg_ms,
            p95_ms: fallback_avg_ms,
            p99_ms: fallback_avg_ms,
            max_ms: fallback_avg_ms,
        }
    }
}

fn process_rss_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kb = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
        Some(kb / 1024.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn enqueue_backpressure_notice(
    json_tx: &tokio::sync::mpsc::Sender<String>,
    channel: &str,
    current_fps: u32,
) {
    let suggested_fps = current_fps.saturating_div(2).max(1);
    let message = ServerMessage::Backpressure {
        dropped_frames: 1,
        channel: channel.to_owned(),
        recommendation: "reduce_fps".to_owned(),
        suggested_fps,
    };

    if let Ok(text) = serde_json::to_string(&message) {
        let _ = try_enqueue_json(json_tx, text, "backpressure");
    }
}

fn sorted_channel_names(channels: &HashSet<WsChannel>) -> Vec<String> {
    let mut names: Vec<String> = channels
        .iter()
        .map(|channel| channel.as_str().to_owned())
        .collect();
    names.sort();
    names
}

fn unique_sorted_channel_names(channels: &[WsChannel]) -> Vec<String> {
    let mut unique: HashSet<WsChannel> = HashSet::new();
    for channel in channels {
        unique.insert(*channel);
    }
    sorted_channel_names(&unique)
}

fn ws_capabilities() -> Vec<String> {
    let mut capabilities: Vec<String> = WsChannel::SUPPORTED
        .iter()
        .map(|channel| channel.as_str().to_owned())
        .collect();
    capabilities.push("commands".to_owned());
    capabilities
}

fn event_message_parts(
    event: &hypercolor_types::event::HypercolorEvent,
) -> (String, serde_json::Value) {
    let serialized = serde_json::to_value(event).ok();
    let event_type = serialized
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str);

    let event_name = if let Some(event_type) = event_type {
        to_snake_case(event_type)
    } else {
        format!("{:?}", event.category()).to_lowercase()
    };
    let event_data = serialized
        .and_then(|value| value.get("data").cloned())
        .unwrap_or_else(|| json!({}));

    (event_name, event_data)
}

fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut previous_was_lower_or_digit = false;

    for ch in input.chars() {
        if ch.is_ascii_uppercase() {
            if previous_was_lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_was_lower_or_digit = false;
        } else {
            out.push(ch);
            previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }

    out
}

fn format_iso8601_now() -> String {
    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn epoch_to_utc(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86400;
    let days = epoch_secs / secs_per_day;
    let day_secs = epoch_secs % secs_per_day;

    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u32, d as u32, hour, minute, second)
}

async fn build_hello_state(state: &AppState) -> HelloState {
    let render_snapshot = state.render_loop.read().await.stats();
    let target_fps = render_snapshot.tier.fps();
    let actual_fps = paced_fps(render_snapshot.avg_frame_time.as_secs_f64(), target_fps);

    let active_effect = {
        let engine = state.effect_engine.lock().await;
        engine.active_metadata().map(|meta| NameRef {
            id: meta.id.to_string(),
            name: meta.name.clone(),
        })
    };

    let devices = state.device_registry.list().await;
    let total_leds = devices.iter().fold(0_usize, |acc, tracked| {
        let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
        acc.saturating_add(led_count)
    });

    HelloState {
        running: render_snapshot.state != hypercolor_core::engine::RenderLoopState::Stopped,
        paused: render_snapshot.state == hypercolor_core::engine::RenderLoopState::Paused,
        brightness: 100,
        fps: HelloFps {
            target: target_fps,
            actual: (actual_fps * 10.0).round() / 10.0,
        },
        effect: active_effect,
        profile: None,
        layout: None,
        device_count: devices.len(),
        total_leds,
    }
}

/// Serialize and send a JSON message over the WebSocket.
async fn send_json(socket: &mut WebSocket, msg: &impl Serialize) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).unwrap_or_default();
    socket.send(Message::Text(json.into())).await.map_err(|e| {
        debug!("WebSocket send error: {e}");
        e
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ChannelConfig, ChannelConfigPatch, ServerMessage, WsChannel, command_response_from_http,
        dispatch_command, encode_canvas_binary, encode_frame_binary, encode_spectrum_binary,
        event_message_parts, filter_frame_zones, normalize_command_path, parse_channels,
        parse_command_method, should_relay_event, to_snake_case, try_enqueue_json,
        unique_sorted_channel_names, ws_capabilities,
    };
    use crate::api::AppState;
    use crate::api::security::{RequestAuthContext, SecurityState};
    use axum::response::IntoResponse;
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::{Canvas, Rgba};
    use hypercolor_types::event::{
        FrameData, FrameTiming, HypercolorEvent, SpectrumData, ZoneColors,
    };
    use std::collections::HashSet;
    use std::sync::Arc;

    fn secured_state() -> Arc<AppState> {
        let mut state = AppState::new();
        state.security_state =
            SecurityState::with_keys(Some("hc_ak_control_test"), Some("hc_ak_r_read_test"));
        Arc::new(state)
    }

    #[test]
    fn parse_channels_accepts_supported_channel() {
        let channels = vec![
            "events".to_owned(),
            "frames".to_owned(),
            "spectrum".to_owned(),
            "canvas".to_owned(),
            "screen_canvas".to_owned(),
            "metrics".to_owned(),
        ];
        let parsed = parse_channels(&channels).expect("events should parse");
        assert_eq!(
            parsed,
            vec![
                WsChannel::Events,
                WsChannel::Frames,
                WsChannel::Spectrum,
                WsChannel::Canvas,
                WsChannel::ScreenCanvas,
                WsChannel::Metrics
            ]
        );
    }

    #[test]
    fn parse_channels_rejects_unknown_channel() {
        let channels = vec!["unknown".to_owned()];
        let error = parse_channels(&channels).expect_err("unknown channel should fail");
        assert_eq!(error.code, "invalid_request");
    }

    #[test]
    fn channel_config_apply_patch_supports_all_channels() {
        let mut config = ChannelConfig::default();
        let patch: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
            "frames": {"fps": 30, "format": "binary"},
            "spectrum": {"fps": 20, "bins": 32},
            "canvas": {"fps": 60, "format": "rgba"},
            "screen_canvas": {"fps": 24, "format": "rgb"},
            "metrics": {"interval_ms": 500}
        }))
        .expect("valid json patch");

        config
            .apply_patch(patch)
            .expect("full channel config patch should be accepted");

        let json = serde_json::to_value(config).expect("config serializes");
        assert_eq!(json["canvas"]["fps"], 60);
        assert_eq!(json["canvas"]["format"], "rgba");
        assert_eq!(json["screen_canvas"]["fps"], 24);
        assert_eq!(json["screen_canvas"]["format"], "rgb");
        assert_eq!(json["metrics"]["interval_ms"], 500);
    }

    #[test]
    fn channel_config_defaults_are_stable() {
        let config = ChannelConfig::default();
        let json = serde_json::to_value(config).expect("config serializes");

        assert_eq!(json["frames"]["fps"], 30);
        assert_eq!(json["frames"]["format"], "binary");
        assert_eq!(json["spectrum"]["bins"], 64);
        assert_eq!(json["canvas"]["fps"], 15);
        assert_eq!(json["screen_canvas"]["fps"], 15);
        assert_eq!(json["metrics"]["interval_ms"], 1000);
    }

    #[test]
    fn unique_channel_names_are_sorted() {
        let names =
            unique_sorted_channel_names(&[WsChannel::Events, WsChannel::Events, WsChannel::Events]);
        assert_eq!(names, vec!["events"]);
    }

    #[test]
    fn snake_case_conversion_handles_camel_case() {
        assert_eq!(to_snake_case("DeviceDiscovered"), "device_discovered");
        assert_eq!(to_snake_case("Paused"), "paused");
    }

    #[test]
    fn event_message_parts_unwraps_payload() {
        let event = HypercolorEvent::DeviceDiscoveryStarted {
            backends: vec!["wled".to_owned()],
        };

        let (event_name, event_data) = event_message_parts(&event);
        assert_eq!(event_name, "device_discovery_started");
        assert_eq!(event_data["backends"], serde_json::json!(["wled"]));
        assert!(event_data.get("type").is_none());
    }

    #[test]
    fn event_message_parts_defaults_to_empty_object_for_unit_events() {
        let (event_name, event_data) = event_message_parts(&HypercolorEvent::Resumed);
        assert_eq!(event_name, "resumed");
        assert_eq!(event_data, serde_json::json!({}));
    }

    #[test]
    fn frame_rendered_events_are_suppressed_when_metrics_are_subscribed() {
        let channels = HashSet::from([WsChannel::Events, WsChannel::Metrics]);
        let event = HypercolorEvent::FrameRendered {
            frame_number: 7,
            timing: FrameTiming {
                render_us: 0,
                sample_us: 0,
                push_us: 0,
                total_us: 0,
                budget_us: 16_666,
            },
        };

        assert!(!should_relay_event(&event, &channels));
    }

    #[test]
    fn frame_rendered_events_pass_through_for_event_only_clients() {
        let channels = HashSet::from([WsChannel::Events]);
        let event = HypercolorEvent::FrameRendered {
            frame_number: 7,
            timing: FrameTiming {
                render_us: 0,
                sample_us: 0,
                push_us: 0,
                total_us: 0,
                budget_us: 16_666,
            },
        };

        assert!(should_relay_event(&event, &channels));
    }

    #[test]
    fn ws_capabilities_include_commands() {
        let capabilities = ws_capabilities();
        assert!(capabilities.contains(&"events".to_owned()));
        assert!(capabilities.contains(&"frames".to_owned()));
        assert!(capabilities.contains(&"spectrum".to_owned()));
        assert!(capabilities.contains(&"canvas".to_owned()));
        assert!(capabilities.contains(&"screen_canvas".to_owned()));
        assert!(capabilities.contains(&"metrics".to_owned()));
        assert!(capabilities.contains(&"commands".to_owned()));
    }

    #[tokio::test]
    async fn try_enqueue_json_drops_when_queue_is_full() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1);

        assert!(try_enqueue_json(&tx, "first".to_owned(), "test"));
        assert!(!try_enqueue_json(&tx, "second".to_owned(), "test"));

        assert_eq!(rx.recv().await.as_deref(), Some("first"));
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn parse_command_method_rejects_invalid_values() {
        let error = parse_command_method("BREW").expect_err("BREW should be rejected");
        assert_eq!(error.code, "invalid_request");
    }

    #[test]
    fn normalize_command_path_adds_api_prefix() {
        assert_eq!(
            normalize_command_path("/status").expect("path should normalize"),
            "/api/v1/status"
        );
        assert_eq!(
            normalize_command_path("/api/v1/status").expect("path should stay stable"),
            "/api/v1/status"
        );
    }

    #[test]
    fn normalize_command_path_rejects_relative_paths() {
        let error = normalize_command_path("status").expect_err("relative path must fail");
        assert_eq!(error.code, "invalid_request");
    }

    #[tokio::test]
    async fn command_response_from_http_unwraps_data_envelope() {
        let response = (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({
                "data": {"ok": true}
            })),
        )
            .into_response();
        let message = command_response_from_http("cmd_test".to_owned(), response).await;
        match message {
            ServerMessage::Response {
                id,
                status,
                data,
                error,
            } => {
                assert_eq!(id, "cmd_test");
                assert_eq!(status, 200);
                assert_eq!(data, Some(serde_json::json!({"ok": true})));
                assert!(error.is_none());
            }
            _ => panic!("expected response variant"),
        }
    }

    #[tokio::test]
    async fn command_response_from_http_unwraps_error_envelope() {
        let response = (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": {"code": "not_found", "message": "missing resource"}
            })),
        )
            .into_response();
        let message = command_response_from_http("cmd_missing".to_owned(), response).await;
        match message {
            ServerMessage::Response {
                id,
                status,
                data,
                error,
            } => {
                assert_eq!(id, "cmd_missing");
                assert_eq!(status, 404);
                assert!(data.is_none());
                assert_eq!(
                    error,
                    Some(serde_json::json!({
                        "code": "not_found",
                        "message": "missing resource"
                    }))
                );
            }
            _ => panic!("expected response variant"),
        }
    }

    #[tokio::test]
    async fn dispatch_command_routes_to_status() {
        let state = Arc::new(AppState::new());
        let message = dispatch_command(
            &state,
            RequestAuthContext::unsecured(),
            "cmd_status".to_owned(),
            "GET".to_owned(),
            "/status".to_owned(),
            None,
        )
        .await;

        match message {
            ServerMessage::Response {
                id,
                status,
                data,
                error,
            } => {
                assert_eq!(id, "cmd_status");
                assert_eq!(status, 200);
                let payload = data.expect("status command should return payload");
                assert!(payload.get("running").is_some());
                assert!(error.is_none());
            }
            _ => panic!("expected command response"),
        }
    }

    #[tokio::test]
    async fn dispatch_command_rejects_invalid_method() {
        let state = Arc::new(AppState::new());
        let message = dispatch_command(
            &state,
            RequestAuthContext::unsecured(),
            "cmd_bad_method".to_owned(),
            "BREW".to_owned(),
            "/status".to_owned(),
            None,
        )
        .await;

        match message {
            ServerMessage::Response {
                id,
                status,
                data,
                error,
            } => {
                assert_eq!(id, "cmd_bad_method");
                assert_eq!(status, 400);
                assert!(data.is_none());
                assert_eq!(
                    error.and_then(|value| value.get("code").cloned()),
                    Some(serde_json::json!("invalid_request"))
                );
            }
            _ => panic!("expected command response"),
        }
    }

    #[tokio::test]
    async fn dispatch_command_preserves_secured_ws_auth_context() {
        let state = secured_state();
        let message = dispatch_command(
            &state,
            RequestAuthContext::read_only(),
            "cmd_status".to_owned(),
            "GET".to_owned(),
            "/status".to_owned(),
            None,
        )
        .await;

        match message {
            ServerMessage::Response {
                id,
                status,
                data,
                error,
            } => {
                assert_eq!(id, "cmd_status");
                assert_eq!(status, 200);
                assert!(data.is_some());
                assert!(error.is_none());
            }
            _ => panic!("expected command response"),
        }
    }

    #[tokio::test]
    async fn dispatch_command_requires_auth_context_when_security_is_enabled() {
        let state = secured_state();
        let message = dispatch_command(
            &state,
            RequestAuthContext::unsecured(),
            "cmd_status".to_owned(),
            "GET".to_owned(),
            "/status".to_owned(),
            None,
        )
        .await;

        match message {
            ServerMessage::Response {
                status,
                data,
                error,
                ..
            } => {
                assert_eq!(status, 401);
                assert!(data.is_none());
                assert_eq!(
                    error.and_then(|value| value.get("code").cloned()),
                    Some(serde_json::json!("unauthorized"))
                );
            }
            _ => panic!("expected command response"),
        }
    }

    #[test]
    fn frame_binary_encoder_writes_header_and_payload() {
        let frame = FrameData {
            frame_number: 42,
            timestamp_ms: 1234,
            zones: vec![ZoneColors {
                zone_id: "zone_a".to_owned(),
                colors: vec![[255, 0, 0], [0, 255, 0]],
            }],
        };

        let encoded = encode_frame_binary(&frame);
        assert_eq!(encoded[0], 0x01);
        assert_eq!(
            u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
            42
        );
        assert_eq!(
            u32::from_le_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]),
            1234
        );
        assert_eq!(encoded[9], 1);
    }

    #[test]
    fn spectrum_binary_encoder_uses_requested_bin_count() {
        let spectrum = SpectrumData {
            timestamp_ms: 77,
            level: 0.5,
            bass: 0.4,
            mid: 0.3,
            treble: 0.2,
            beat: true,
            beat_confidence: 0.9,
            bpm: None,
            bins: vec![0.0; 64],
        };

        let encoded = encode_spectrum_binary(&spectrum, 16);
        assert_eq!(encoded[0], 0x02);
        assert_eq!(encoded[5], 16);
        assert_eq!(encoded[22], 1);
    }

    #[test]
    fn filter_frame_zones_respects_named_subset() {
        let zones = vec![
            ZoneColors {
                zone_id: "left".to_owned(),
                colors: vec![[255, 0, 0]],
            },
            ZoneColors {
                zone_id: "right".to_owned(),
                colors: vec![[0, 0, 255]],
            },
        ];

        let filtered = filter_frame_zones(&zones, &["right".to_owned()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].zone_id, "right");

        let all = filter_frame_zones(&zones, &["all".to_owned()]);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn canvas_binary_encoder_writes_spec_header_and_rgb_payload() {
        let mut canvas = Canvas::new(2, 1);
        canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
        canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
        let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

        let encoded = encode_canvas_binary(&frame, super::CanvasFormat::Rgb);
        assert_eq!(encoded[0], super::WS_CANVAS_HEADER);
        assert_eq!(
            u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
            7
        );
        assert_eq!(
            u32::from_le_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]),
            99
        );
        assert_eq!(u16::from_le_bytes([encoded[9], encoded[10]]), 2);
        assert_eq!(u16::from_le_bytes([encoded[11], encoded[12]]), 1);
        assert_eq!(encoded[13], 0);
        assert_eq!(&encoded[14..20], &[10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn canvas_binary_encoder_writes_rgba_payload_without_repacking() {
        let mut canvas = Canvas::new(2, 1);
        canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
        canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 200));
        let frame = CanvasFrame::from_canvas(&canvas, 7, 99);

        let encoded = encode_canvas_binary(&frame, super::CanvasFormat::Rgba);
        assert_eq!(encoded[13], 1);
        assert_eq!(&encoded[14..22], &[10, 20, 30, 255, 40, 50, 60, 200]);
    }

    #[test]
    fn screen_canvas_binary_encoder_uses_distinct_header() {
        let mut canvas = Canvas::new(1, 1);
        canvas.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));
        let frame = CanvasFrame::from_canvas(&canvas, 5, 44);

        let encoded = super::encode_canvas_binary_with_header(
            &frame,
            super::CanvasFormat::Rgb,
            super::WS_SCREEN_CANVAS_HEADER,
        );
        assert_eq!(encoded[0], super::WS_SCREEN_CANVAS_HEADER);
        assert_eq!(&encoded[14..17], &[90, 80, 70]);
    }
}
