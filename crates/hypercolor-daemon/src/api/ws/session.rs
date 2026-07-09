//! Connection upgrade, handshake, heartbeat, and client message dispatch.
//!
//! Owns the per-connection state machine: sends the initial hello, spawns
//! relay tasks, runs the select loop that muxes incoming client frames,
//! outbound JSON/binary queues, and the ping/pong heartbeat.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use axum::extract::{Extension, State, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_leptos_ext::axum::upgrade_handler;
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;
use tracing::{debug, warn};
use uuid::Uuid;

use hypercolor_types::scene::{Scene, SceneId, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

use super::cache::{WS_BUFFER_SIZE, WsClientGuard, track_ws_bytes_sent};
use super::command::dispatch_command;
use super::protocol::{
    ClientMessage, HelloFps, HelloState, NameRef, SceneRef, ServerMessage, SubscriptionState,
    WsChannel, WsProtocolError, parse_channels, sorted_channel_names, unique_sorted_channel_names,
    ws_capabilities,
};
use super::relays::{
    publish_subscriptions, relay_canvas, relay_device_metrics, relay_display_preview, relay_events,
    relay_frames, relay_metrics, relay_screen_canvas, relay_screen_zones, relay_sensors,
    relay_spectrum, relay_web_viewport_canvas, relay_zone_preview,
};
use crate::api::AppState;
use crate::api::effects::active_effect_metadata;
use crate::api::layouts::validate_layout_sampling_radii;
use crate::api::scenes;
use crate::api::security::RequestAuthContext;

const WS_PROTOCOL_VERSION: &str = "1.0";
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
const WS_PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// `GET /api/v1/ws` — Upgrade to WebSocket.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> Response {
    if !ws_origin_allowed(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let auth_context =
        auth_context.map_or_else(RequestAuthContext::unsecured, |Extension(context)| context);
    upgrade_handler(ws, move |socket| handle_socket(socket, state, auth_context))
}

fn ws_origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };

    if is_loopback_origin(origin) {
        return true;
    }

    if !state.security_state.security_enabled() {
        return false;
    }

    state
        .config_manager
        .as_ref()
        .map(|config| config.get().web.cors_origins.clone())
        .unwrap_or_default()
        .into_iter()
        .any(|allowed| header_value_eq_origin(origin, &allowed))
}

fn header_value_eq_origin(origin: &HeaderValue, allowed: &str) -> bool {
    origin
        .to_str()
        .is_ok_and(|value| value.eq_ignore_ascii_case(allowed.trim()))
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return false;
    }

    let Some(host) = uri.host() else {
        return false;
    };
    // `Uri::host()` keeps the brackets on IPv6 literals (`[::1]`), which
    // `IpAddr` will not parse — strip them before classifying the address.
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
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
            subscriptions: sorted_channel_names(subscriptions.channels),
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
    let screen_zones_relay_handle = tokio::spawn(relay_screen_zones(
        Arc::clone(&state.preview_runtime),
        subscriptions_rx.clone(),
        binary_tx.clone(),
    ));
    let web_viewport_canvas_relay_handle = tokio::spawn(relay_web_viewport_canvas(
        Arc::clone(&state.preview_runtime),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let zone_preview_relay_handle = tokio::spawn(relay_zone_preview(
        Arc::clone(&state.preview_runtime),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let display_preview_relay_handle = tokio::spawn(relay_display_preview(
        Arc::clone(&state),
        Arc::clone(&state.display_frames),
        json_tx.clone(),
        binary_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let metrics_relay_handle = tokio::spawn(relay_metrics(
        Arc::clone(&state),
        json_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let device_metrics_relay_handle = tokio::spawn(relay_device_metrics(
        Arc::clone(&state),
        json_tx.clone(),
        subscriptions_rx.clone(),
    ));
    let sensors_relay_handle = tokio::spawn(relay_sensors(
        Arc::clone(&state),
        json_tx.clone(),
        subscriptions_rx.clone(),
    ));

    let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut awaiting_pong = false;
    let mut ping_sent_at = Instant::now();
    let mut zone_layout_preview_keys = HashSet::<(SceneId, ZoneId)>::new();

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
                        if socket.send(Message::Binary(bytes)).await.is_err() {
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
                            &mut zone_layout_preview_keys,
                            &mut socket,
                        )
                        .await;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        awaiting_pong = false;
                    }
                    Some(Ok(Message::Ping(payload)))
                        if socket
                            .send(Message::Pong(payload.clone()))
                            .await
                            .is_err() =>
                    {
                        break;
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
    screen_zones_relay_handle.abort();
    web_viewport_canvas_relay_handle.abort();
    display_preview_relay_handle.abort();
    zone_preview_relay_handle.abort();
    metrics_relay_handle.abort();
    device_metrics_relay_handle.abort();
    sensors_relay_handle.abort();
    state
        .zone_layout_previews
        .clear_many(zone_layout_preview_keys)
        .await;
    debug!("WebSocket client disconnected");
}

pub(super) fn authorize_subscription_channels(
    auth_context: RequestAuthContext,
    channels: &[WsChannel],
) -> Result<(), WsProtocolError> {
    if auth_context.can_control() {
        return Ok(());
    }

    let restricted_channels: Vec<&'static str> = channels
        .iter()
        .copied()
        .filter(|channel| channel.requires_control_subscription())
        .map(WsChannel::as_str)
        .collect();

    if restricted_channels.is_empty() {
        Ok(())
    } else {
        Err(WsProtocolError::forbidden(
            "Screen capture preview subscriptions require a control-tier API key",
            json!({"channels": restricted_channels, "required_tier": "control"}),
        ))
    }
}

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    subscriptions: &mut SubscriptionState,
    subscriptions_tx: &watch::Sender<SubscriptionState>,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
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

            if let Err(error) = authorize_subscription_channels(auth_context, &parsed_channels) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

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
                config: subscriptions.config.filtered_json(subscriptions.channels),
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
            let remaining = sorted_channel_names(subscriptions.channels);

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
        ClientMessage::ZoneLayoutPreview {
            scene_id,
            zone_id,
            layout,
        } => {
            if let Err(error) = ensure_control_tier(auth_context) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            if let Err(error) = handle_zone_layout_preview(
                state,
                zone_layout_preview_keys,
                scene_id,
                zone_id,
                layout,
            )
            .await
            {
                let _ = send_json(socket, &error.into_message()).await;
            }
        }
        ClientMessage::ZoneLayoutPreviewClear { scene_id, zone_id } => {
            if let Err(error) = ensure_control_tier(auth_context) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            if let Err(error) = handle_zone_layout_preview_clear(
                state,
                zone_layout_preview_keys,
                &scene_id,
                &zone_id,
            )
            .await
            {
                let _ = send_json(socket, &error.into_message()).await;
            }
        }
    }
}

fn ensure_control_tier(auth_context: RequestAuthContext) -> Result<(), WsProtocolError> {
    if auth_context.can_control() {
        Ok(())
    } else {
        Err(WsProtocolError::forbidden(
            "Read-only API key cannot perform write operations",
            serde_json::json!({"required_tier": "control"}),
        ))
    }
}

async fn handle_zone_layout_preview(
    state: &Arc<AppState>,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
    scene_id_raw: String,
    zone_id_raw: String,
    layout: SpatialLayout,
) -> Result<(), WsProtocolError> {
    let zone_id = parse_zone_preview_id(&zone_id_raw)?;
    let (scene_id, layout) = {
        let manager = state.scene_manager.read().await;
        let scene_id = scenes::resolve_scene_id(&manager, &scene_id_raw).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Scene not found: {scene_id_raw}"))
        })?;
        let scene = manager.get(&scene_id).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Scene not found: {scene_id_raw}"))
        })?;
        let layout = validated_zone_layout_preview(scene, zone_id, layout)?;
        (scene_id, layout)
    };

    state
        .zone_layout_previews
        .set(scene_id, zone_id, layout)
        .await;
    zone_layout_preview_keys.insert((scene_id, zone_id));
    Ok(())
}

async fn handle_zone_layout_preview_clear(
    state: &Arc<AppState>,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
    scene_id_raw: &str,
    zone_id_raw: &str,
) -> Result<(), WsProtocolError> {
    let scene_id = parse_scene_preview_id(scene_id_raw)?;
    let zone_id = parse_zone_preview_id(zone_id_raw)?;
    state.zone_layout_previews.clear(scene_id, zone_id).await;
    zone_layout_preview_keys.remove(&(scene_id, zone_id));
    Ok(())
}

pub(super) fn validated_zone_layout_preview(
    scene: &Scene,
    zone_id: ZoneId,
    layout: SpatialLayout,
) -> Result<SpatialLayout, WsProtocolError> {
    validate_layout_sampling_radii(&layout).map_err(WsProtocolError::invalid_request)?;

    if scene.blocks_runtime_mutation() {
        return Err(WsProtocolError::invalid_request(format!(
            "Scene '{}' is snapshot locked",
            scene.name
        )));
    }

    let Some(group) = scene.groups.iter().find(|group| group.id == zone_id) else {
        return Err(WsProtocolError::invalid_request(format!(
            "Zone not found: {zone_id}"
        )));
    };

    let stored_ids = group
        .layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    let request_ids = layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    if request_ids.len() != layout.zones.len() || stored_ids != request_ids {
        return Err(WsProtocolError::invalid_request(
            "zone layout preview must contain exactly the selected zone outputs",
        ));
    }

    let mut stored = group
        .layout
        .zones
        .iter()
        .cloned()
        .map(|zone| (zone.id.clone(), zone))
        .collect::<HashMap<_, _>>();
    let mut preview = group.layout.clone();
    preview.zones = layout
        .zones
        .into_iter()
        .filter_map(|incoming| {
            let mut merged = stored.remove(&incoming.id)?;
            merged.name = incoming.name;
            merged.position = incoming.position;
            merged.size = incoming.size;
            merged.rotation = incoming.rotation;
            merged.scale = incoming.scale;
            merged.display_order = incoming.display_order;
            merged.orientation = incoming.orientation;
            merged.shape = incoming.shape;
            merged.shape_preset = incoming.shape_preset;
            merged.sampling_mode = incoming.sampling_mode;
            merged.edge_behavior = incoming.edge_behavior;
            merged.brightness = incoming.brightness;
            Some(merged)
        })
        .collect();
    preview.canvas_width = layout.canvas_width;
    preview.canvas_height = layout.canvas_height;
    preview.default_sampling_mode = layout.default_sampling_mode;
    preview.default_edge_behavior = layout.default_edge_behavior;

    Ok(preview)
}

fn parse_scene_preview_id(raw: &str) -> Result<SceneId, WsProtocolError> {
    match raw {
        "default" => Ok(SceneId::DEFAULT),
        _ => Uuid::parse_str(raw).map(SceneId).map_err(|_| {
            WsProtocolError::invalid_request("scene_id must be a valid UUID or 'default'")
        }),
    }
}

fn parse_zone_preview_id(raw: &str) -> Result<ZoneId, WsProtocolError> {
    Uuid::parse_str(raw)
        .map(ZoneId)
        .map_err(|_| WsProtocolError::invalid_request("zone_id must be a valid UUID"))
}

async fn build_hello_state(state: &AppState) -> HelloState {
    let render_snapshot = state.render_loop.read().await.stats();
    let target_fps = render_snapshot.tier.fps();
    let actual_fps = paced_fps(render_snapshot.avg_frame_time.as_secs_f64(), target_fps);

    let active_effect = active_effect_metadata(state).await.map(|meta| NameRef {
        id: meta.id.to_string(),
        name: meta.name.clone(),
    });
    let active_scene = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager.active_scene().map(|scene| SceneRef {
            id: scene.id.to_string(),
            name: scene.name.clone(),
            snapshot_locked: scene.blocks_runtime_mutation(),
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
        scene: active_scene,
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

#[cfg(test)]
mod security_tests {
    use super::ensure_control_tier;
    use crate::api::security::RequestAuthContext;

    #[test]
    fn read_only_auth_cannot_mutate_zone_layout_previews() {
        let error = ensure_control_tier(RequestAuthContext::read_only())
            .expect_err("read-only auth context should be rejected for mutating preview messages");
        assert_eq!(error.code, "forbidden");
        assert_eq!(
            error.message,
            "Read-only API key cannot perform write operations"
        );
        assert_eq!(
            error.details,
            Some(serde_json::json!({"required_tier": "control"}))
        );
    }

    #[test]
    fn unsecured_auth_can_mutate_zone_layout_previews() {
        ensure_control_tier(RequestAuthContext::unsecured())
            .expect("unsecured context should continue to allow local mutations");
    }
}

#[cfg(test)]
mod origin_tests {
    use super::{header_value_eq_origin, is_loopback_origin, ws_origin_allowed};
    use crate::api::AppState;
    use axum::http::{HeaderMap, HeaderValue, header};

    #[test]
    fn loopback_origin_is_allowed() {
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "http://localhost:9430"
        )));
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "https://127.0.0.1:9430"
        )));
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "http://[::1]:9430"
        )));
    }

    #[test]
    fn non_loopback_origin_is_rejected() {
        assert!(!is_loopback_origin(&HeaderValue::from_static(
            "https://evil.example"
        )));
    }

    #[test]
    fn origin_comparison_is_case_insensitive() {
        let origin = HeaderValue::from_static("https://studio.example");
        assert!(header_value_eq_origin(&origin, "HTTPS://STUDIO.EXAMPLE"));
    }

    #[test]
    fn unsecured_daemon_rejects_non_loopback_browser_origins() {
        let state = AppState::new();

        // No Origin header: native and CLI clients are always allowed.
        assert!(ws_origin_allowed(&state, &HeaderMap::new()));

        let mut loopback = HeaderMap::new();
        loopback.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:9430"),
        );
        assert!(ws_origin_allowed(&state, &loopback));

        // A non-loopback browser origin is rejected on the default
        // unsecured daemon, where no cors_origins allowlist applies.
        let mut remote = HeaderMap::new();
        remote.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!ws_origin_allowed(&state, &remote));
    }
}
