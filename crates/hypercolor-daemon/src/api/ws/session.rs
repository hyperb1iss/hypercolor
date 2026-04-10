//! Connection upgrade, handshake, heartbeat, and client message dispatch.
//!
//! Owns the per-connection state machine: sends the initial hello, spawns
//! relay tasks, runs the select loop that muxes incoming client frames,
//! outbound JSON/binary queues, and the ping/pong heartbeat.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use axum::extract::{Extension, State, WebSocketUpgrade};
use axum::response::Response;
use serde::Serialize;
use tokio::sync::watch;
use tracing::{debug, warn};

use super::cache::{WS_BUFFER_SIZE, WsClientGuard, track_ws_bytes_sent};
use super::command::dispatch_command;
use super::protocol::{
    ClientMessage, HelloFps, HelloState, NameRef, ServerMessage, SubscriptionState,
    WsProtocolError, parse_channels, sorted_channel_names, unique_sorted_channel_names,
    ws_capabilities,
};
use super::relays::{
    publish_subscriptions, relay_canvas, relay_events, relay_frames, relay_metrics,
    relay_screen_canvas, relay_spectrum,
};
use crate::api::AppState;
use crate::api::security::RequestAuthContext;

const WS_PROTOCOL_VERSION: &str = "1.0";
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
const WS_PONG_TIMEOUT: Duration = Duration::from_secs(10);

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

    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let mut subscriptions = initial_subscriptions;

    // Send hello message.
    let hello = {
        ServerMessage::Hello {
            version: WS_PROTOCOL_VERSION.to_owned(),
            server: state.server_identity.clone(),
            state: build_hello_state(&state).await,
            capabilities: ws_capabilities(),
            subscriptions: sorted_channel_names(&subscriptions.channels),
        }
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // Subscribe to the event bus and watch channels.
    let event_rx = state.event_bus.subscribe_all();
    // Split outbound traffic: both queues are bounded so slow clients cannot
    // grow daemon memory without limit.
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(WS_BUFFER_SIZE);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(WS_BUFFER_SIZE);

    // Spawn event relay tasks — each watches immutable subscription snapshots.
    let relay_handle = tokio::spawn(relay_events(
        event_rx,
        json_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let frame_relay_handle = tokio::spawn(relay_frames(
        Arc::clone(&state),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let spectrum_relay_handle = tokio::spawn(relay_spectrum(
        Arc::clone(&state),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let canvas_power_rx = state.power_state.subscribe();
    let canvas_relay_handle = tokio::spawn(relay_canvas(
        Arc::clone(&state.preview_runtime),
        canvas_power_rx,
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let screen_canvas_relay_handle = tokio::spawn(relay_screen_canvas(
        Arc::clone(&state.preview_runtime),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let metrics_relay_handle =
        tokio::spawn(relay_metrics(Arc::clone(&state), json_tx, subscriptions_rx));

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
                        if socket.send(Message::Text(msg)).await.is_err() {
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
                            &mut subscriptions,
                            &subscriptions_tx,
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

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    subscriptions: &mut SubscriptionState,
    subscriptions_tx: &watch::Sender<SubscriptionState>,
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

            if let Some(config_patch) = config
                && let Err(error) = subscriptions.config.apply_patch(config_patch)
            {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            for channel in &parsed_channels {
                subscriptions.channels.insert(*channel);
            }

            let ack = ServerMessage::Subscribed {
                channels: unique_sorted_channel_names(&parsed_channels),
                config: subscriptions.config.filtered_json(&subscriptions.channels),
            };
            publish_subscriptions(subscriptions_tx, subscriptions);
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

            for channel in &parsed_channels {
                subscriptions.channels.remove(*channel);
            }
            let remaining = sorted_channel_names(&subscriptions.channels);

            let ack = ServerMessage::Unsubscribed {
                channels: unique_sorted_channel_names(&parsed_channels),
                remaining,
            };
            publish_subscriptions(subscriptions_tx, subscriptions);
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

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

/// Serialize and send a JSON message over the WebSocket.
async fn send_json(socket: &mut WebSocket, msg: &impl Serialize) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).unwrap_or_default();
    socket.send(Message::Text(json.into())).await.map_err(|e| {
        debug!("WebSocket send error: {e}");
        e
    })
}
