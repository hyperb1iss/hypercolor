//! WebSocket handler — `/api/v1/ws`.
//!
//! Real-time event stream, binary frame data, and bidirectional commands.
//! Each connected client gets its own broadcast subscription with configurable
//! channel filtering. Backpressure is handled by bounded channels — slow
//! consumers get dropped frames rather than unbounded memory growth.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, warn};

use crate::api::AppState;

/// Maximum number of events that can be buffered per WebSocket client.
const WS_BUFFER_SIZE: usize = 64;
const WS_PROTOCOL_VERSION: &str = "1.0";

// ── Subscription Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WsChannel {
    Frames,
    Spectrum,
    Events,
    Canvas,
    Metrics,
}

impl WsChannel {
    const SUPPORTED: [Self; 1] = [Self::Events];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Spectrum => "spectrum",
            Self::Events => "events",
            Self::Canvas => "canvas",
            Self::Metrics => "metrics",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "frames" => Some(Self::Frames),
            "spectrum" => Some(Self::Spectrum),
            "events" => Some(Self::Events),
            "canvas" => Some(Self::Canvas),
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
                validate_range(fps, 1, 30, "config.canvas.fps", "expected 1..=30")?;
                self.canvas.fps = fps;
            }
            if let Some(format) = canvas.format {
                self.canvas.format = format;
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
    /// Protocol-level request error.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
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
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.protocols(["hypercolor-v1"])
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// Process a single WebSocket connection.
async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // Default subscriptions: events only.
    // Wrapped in Arc<RwLock<_>> so the relay task sees subscription changes.
    let subscriptions = Arc::new(RwLock::new(SubscriptionState::default()));

    // Send hello message.
    let hello = {
        let subs = subscriptions.read().await;
        ServerMessage::Hello {
            version: WS_PROTOCOL_VERSION.to_owned(),
            state: build_hello_state(&state).await,
            capabilities: WsChannel::SUPPORTED
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
            subscriptions: sorted_channel_names(&subs.channels),
        }
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // Subscribe to the event bus.
    let event_rx = state.event_bus.subscribe_all();

    // Bounded relay channel for backpressure.
    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<String>(WS_BUFFER_SIZE);

    // Spawn event relay task — shares the subscription set via Arc<RwLock<_>>.
    let relay_subs = Arc::clone(&subscriptions);
    let relay_handle = tokio::spawn(relay_events(event_rx, relay_tx, relay_subs));

    // Main loop: multiplex between incoming client messages and outbound events.
    loop {
        tokio::select! {
            // Outbound: relay events to the client.
            relay_msg = relay_rx.recv() => {
                match relay_msg {
                    Some(msg) => {
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break, // Relay channel closed.
                }
            }

            // Inbound: process client messages.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(&text, &subscriptions, &mut socket).await;
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
    debug!("WebSocket client disconnected");
}

/// Relay events from the broadcast bus to a bounded mpsc channel.
/// Drops events when the consumer is slow (backpressure).
async fn relay_events(
    mut event_rx: broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
    relay_tx: tokio::sync::mpsc::Sender<String>,
    subscriptions: Arc<RwLock<SubscriptionState>>,
) {
    loop {
        match event_rx.recv().await {
            Ok(timestamped) => {
                let has_events = subscriptions
                    .read()
                    .await
                    .channels
                    .contains(&WsChannel::Events);
                if !has_events {
                    continue;
                }

                let event_name = event_identifier(&timestamped.event);
                let msg = ServerMessage::Event {
                    event: event_name,
                    timestamp: timestamped.timestamp.clone(),
                    data: serde_json::to_value(&timestamped.event).unwrap_or_default(),
                };
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };

                // If the channel is full, drop the event (backpressure).
                if relay_tx.try_send(json).is_err() {
                    debug!("Dropping event for slow WebSocket consumer");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket consumer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
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

            if let Some(config_patch) = config {
                if let Err(error) = validate_patch_supported(&config_patch)
                    .and_then(|()| subs.config.apply_patch(config_patch))
                {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
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
    }
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

fn validate_patch_supported(patch: &ChannelConfigPatch) -> Result<(), WsProtocolError> {
    if patch.frames.is_some() {
        return Err(WsProtocolError::unsupported_channel("frames"));
    }
    if patch.spectrum.is_some() {
        return Err(WsProtocolError::unsupported_channel("spectrum"));
    }
    if patch.canvas.is_some() {
        return Err(WsProtocolError::unsupported_channel("canvas"));
    }
    if patch.metrics.is_some() {
        return Err(WsProtocolError::unsupported_channel("metrics"));
    }
    Ok(())
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

fn event_identifier(event: &hypercolor_types::event::HypercolorEvent) -> String {
    let serialized = serde_json::to_value(event).ok();
    let event_type = serialized
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str);

    if let Some(event_type) = event_type {
        return to_snake_case(event_type);
    }

    format!("{:?}", event.category()).to_lowercase()
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

async fn build_hello_state(state: &AppState) -> HelloState {
    let render_snapshot = state.render_loop.read().await.stats();
    let target_fps = render_snapshot.tier.fps();
    let actual_fps = if render_snapshot.avg_frame_time.is_zero() {
        f64::from(target_fps)
    } else {
        (1.0 / render_snapshot.avg_frame_time.as_secs_f64()).max(0.0)
    };

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
        ChannelConfig, ChannelConfigPatch, WsChannel, parse_channels, to_snake_case,
        unique_sorted_channel_names, validate_patch_supported,
    };

    #[test]
    fn parse_channels_accepts_supported_channel() {
        let channels = vec!["events".to_owned()];
        let parsed = parse_channels(&channels).expect("events should parse");
        assert_eq!(parsed, vec![WsChannel::Events]);
    }

    #[test]
    fn parse_channels_rejects_unknown_channel() {
        let channels = vec!["unknown".to_owned()];
        let error = parse_channels(&channels).expect_err("unknown channel should fail");
        assert_eq!(error.code, "invalid_request");
    }

    #[test]
    fn parse_channels_rejects_unsupported_channel() {
        let channels = vec!["frames".to_owned()];
        let error = parse_channels(&channels).expect_err("unsupported channel should fail");
        assert_eq!(error.code, "unsupported_channel");
    }

    #[test]
    fn validate_patch_rejects_unsupported_channel_config() {
        let patch: ChannelConfigPatch = serde_json::from_value(serde_json::json!({
            "frames": {"fps": 30}
        }))
        .expect("valid json patch");

        let error = validate_patch_supported(&patch).expect_err("frames config not supported");
        assert_eq!(error.code, "unsupported_channel");
    }

    #[test]
    fn channel_config_defaults_are_stable() {
        let config = ChannelConfig::default();
        let json = serde_json::to_value(config).expect("config serializes");

        assert_eq!(json["frames"]["fps"], 30);
        assert_eq!(json["frames"]["format"], "binary");
        assert_eq!(json["spectrum"]["bins"], 64);
        assert_eq!(json["canvas"]["fps"], 15);
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
}
