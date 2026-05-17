//! Data bridge — connects to the daemon and streams real-time data as Actions.
//!
//! Spawns a background task that:
//! 1. Fetches initial state via REST (effects, devices, favorites, status)
//! 2. Opens a WebSocket for live canvas frames, spectrum, and events
//! 3. Converts all incoming data into Actions pushed to the main loop

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::action::Action;
use crate::client::rest::DaemonClient;
use crate::client::ws::{self, WsMessage};
use crate::state::DaemonState;
use hypercolor_types::controls::ControlSurfaceScope;

/// Spawn the data bridge as a background task.
///
/// Fetches initial state via REST, then streams live data via WebSocket.
/// Pushes `Action` variants to the provided sender. Shuts down when the
/// cancellation token fires.
///
/// Connection errors are reported once per disconnection event — the bridge
/// does NOT spam repeated `DaemonDisconnected` actions during reconnection.
pub async fn spawn_data_bridge(
    host: String,
    port: u16,
    api_key: Option<String>,
    action_tx: mpsc::UnboundedSender<Action>,
    cancel: CancellationToken,
) {
    let client = DaemonClient::new(&host, port, api_key.as_deref());

    // Track whether we've already notified the UI about the current
    // disconnection. Prevents spamming DaemonDisconnected on every
    // reconnection attempt.
    let mut notified_disconnect = false;
    let mut latest_daemon_state = None::<DaemonState>;

    let _ = action_tx.send(Action::DaemonReconnecting);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Phase 1: REST bootstrap — fetch effects, devices, favorites, status
        let rest_ok = match bootstrap_rest(&client, &action_tx).await {
            Ok(state) => {
                latest_daemon_state = Some(state);
                notified_disconnect = false;
                true
            }
            Err(e) => {
                tracing::debug!("REST bootstrap failed: {e}");
                if !notified_disconnect {
                    let _ = action_tx.send(Action::DaemonDisconnected(format!("{e}")));
                    notified_disconnect = true;
                }
                false
            }
        };

        // Phase 2: WebSocket streaming (only if REST succeeded)
        if rest_ok {
            let (ws_tx, mut ws_rx) = mpsc::unbounded_channel();

            let ws_host = host.clone();
            let ws_api_key = api_key.clone();
            let ws_cancel = cancel.clone();
            let ws_handle = tokio::spawn(async move {
                tokio::select! {
                    result = ws::connect(&ws_host, port, ws_api_key.as_deref(), ws_tx) => {
                        if let Err(e) = result {
                            tracing::debug!("WebSocket connection error: {e}");
                        }
                    }
                    () = ws_cancel.cancelled() => {}
                }
            });

            // Forward WebSocket messages as Actions
            loop {
                tokio::select! {
                    () = cancel.cancelled() => {
                        ws_handle.abort();
                        return;
                    }
                    msg = ws_rx.recv() => {
                        match msg {
                            Some(WsMessage::Hello(state)) => {
                                if let Some(daemon_state) = parse_hello_state(&state) {
                                    latest_daemon_state = Some(daemon_state.clone());
                                    let _ = action_tx.send(Action::DaemonConnected(Box::new(daemon_state)));
                                }
                            }
                            Some(WsMessage::Canvas(frame)) => {
                                let _ = action_tx.send(Action::CanvasFrameReceived(Arc::new(frame)));
                            }
                            Some(WsMessage::Spectrum(snapshot)) => {
                                let _ = action_tx.send(Action::SpectrumUpdated(Arc::new(snapshot)));
                            }
                            Some(WsMessage::Event(event)) => {
                                if let Err(error) = refresh_for_event(&client, &action_tx, &event, &mut latest_daemon_state).await {
                                    tracing::debug!(%error, "Failed to refresh TUI state after daemon event");
                                }
                            }
                            Some(WsMessage::Metrics(metrics)) => {
                                if let Some(next_state) =
                                    merge_metrics_into_daemon_state(latest_daemon_state.as_ref(), &metrics)
                                {
                                    latest_daemon_state = Some(next_state.clone());
                                    let _ = action_tx.send(Action::DaemonStateUpdated(Box::new(next_state)));
                                }
                            }
                            Some(WsMessage::Closed) | None => {
                                if !notified_disconnect {
                                    let _ = action_tx.send(Action::DaemonDisconnected("Connection lost".into()));
                                    notified_disconnect = true;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        if cancel.is_cancelled() {
            return;
        }

        // Reconnect with backoff
        let _ = action_tx.send(Action::DaemonReconnecting);
        tracing::debug!("Reconnecting to daemon in 2s...");
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
}

/// Fetch initial state from the REST API.
async fn bootstrap_rest(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
) -> anyhow::Result<DaemonState> {
    let status = client.get_status().await?;
    let _ = action_tx.send(Action::DaemonConnected(Box::new(status.clone())));

    refresh_effects(client, action_tx).await;
    refresh_devices(client, action_tx).await;
    refresh_favorites(client, action_tx).await;

    Ok(status)
}

async fn refresh_for_event(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
    event: &serde_json::Value,
    latest_daemon_state: &mut Option<DaemonState>,
) -> anyhow::Result<()> {
    match event_name(event).unwrap_or_default() {
        name if name.starts_with("device_") => {
            *latest_daemon_state = Some(refresh_status(client, action_tx).await?);
            refresh_devices(client, action_tx).await;
        }
        name if name.starts_with("effect_") => {
            *latest_daemon_state = Some(refresh_status(client, action_tx).await?);
            refresh_effects(client, action_tx).await;
        }
        "active_scene_changed" => {
            if let Some(next_state) =
                merge_active_scene_into_daemon_state(latest_daemon_state.as_ref(), event)
            {
                *latest_daemon_state = Some(next_state.clone());
                let _ = action_tx.send(Action::DaemonStateUpdated(Box::new(next_state)));
            } else {
                *latest_daemon_state = Some(refresh_status(client, action_tx).await?);
            }
        }
        name if name.starts_with("profile_") || name == "session_changed" => {
            *latest_daemon_state = Some(refresh_status(client, action_tx).await?);
        }
        "control_surface_changed" => {
            refresh_control_surface(client, action_tx, event).await;
        }
        _ => {}
    }

    Ok(())
}

async fn refresh_status(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
) -> anyhow::Result<DaemonState> {
    let status = client.get_status().await?;
    let _ = action_tx.send(Action::DaemonStateUpdated(Box::new(status.clone())));
    Ok(status)
}

async fn refresh_effects(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_effects().await {
        Ok(effects) => {
            let _ = action_tx.send(Action::EffectsUpdated(Arc::new(effects)));
        }
        Err(error) => {
            tracing::debug!(%error, "Failed to refresh effect list");
        }
    }
}

async fn refresh_devices(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_devices().await {
        Ok(devices) => {
            let _ = action_tx.send(Action::DevicesUpdated(Arc::new(devices)));
        }
        Err(error) => {
            tracing::debug!(%error, "Failed to refresh device list");
        }
    }
}

async fn refresh_favorites(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_favorites().await {
        Ok(favorites) => {
            let _ = action_tx.send(Action::FavoritesUpdated(Arc::new(favorites)));
        }
        Err(error) => {
            tracing::debug!(%error, "Failed to refresh favorites");
        }
    }
}

async fn refresh_control_surface(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
    event: &serde_json::Value,
) {
    let Some(surface_id) = event_data(event)
        .get("surface_id")
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };

    match client.get_control_surface(surface_id).await {
        Ok(surface) => {
            if let ControlSurfaceScope::Device { device_id, .. } = &surface.scope {
                let _ = action_tx.send(Action::DeviceControlSurfaceRefreshed {
                    device_id: device_id.to_string(),
                    surface: Arc::new(surface),
                });
            }
        }
        Err(error) => {
            tracing::debug!(%surface_id, %error, "Failed to refresh dynamic control surface");
        }
    }
}

fn event_name(event: &serde_json::Value) -> Option<&str> {
    event
        .get("event")
        .and_then(serde_json::Value::as_str)
        .or_else(|| event.get("event_type").and_then(serde_json::Value::as_str))
}

fn event_data(event: &serde_json::Value) -> &serde_json::Value {
    event.get("data").unwrap_or(event)
}

fn merge_metrics_into_daemon_state(
    current: Option<&DaemonState>,
    metrics: &serde_json::Value,
) -> Option<DaemonState> {
    let data = metrics.get("data").unwrap_or(metrics);
    let fps = data.get("fps")?;
    let mut next = current.cloned().unwrap_or(DaemonState {
        running: true,
        brightness: 100,
        fps_target: 0.0,
        fps_actual: 0.0,
        effect_name: None,
        effect_id: None,
        scene_name: None,
        scene_snapshot_locked: false,
        profile_name: None,
        device_count: 0,
        total_leds: 0,
    });

    next.fps_target = fps
        .get("target")
        .and_then(json_f32)
        .unwrap_or(next.fps_target);
    next.fps_actual = fps
        .get("actual")
        .and_then(json_f32)
        .unwrap_or(next.fps_actual);
    next.device_count = data
        .get("devices")
        .and_then(|devices| devices.get("connected"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(next.device_count);
    next.total_leds = data
        .get("devices")
        .and_then(|devices| devices.get("total_leds"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(next.total_leds);

    Some(next)
}

fn merge_active_scene_into_daemon_state(
    current: Option<&DaemonState>,
    event: &serde_json::Value,
) -> Option<DaemonState> {
    let data = event.get("data").unwrap_or(event);
    let scene_name = data
        .get("current_name")
        .or_else(|| data.get("scene_name"))
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let snapshot_locked = data
        .get("current_snapshot_locked")
        .or_else(|| data.get("snapshot_locked"))
        .and_then(serde_json::Value::as_bool)?;

    let mut next = current.cloned().unwrap_or(DaemonState {
        running: true,
        brightness: 100,
        fps_target: 0.0,
        fps_actual: 0.0,
        effect_name: None,
        effect_id: None,
        scene_name: None,
        scene_snapshot_locked: false,
        profile_name: None,
        device_count: 0,
        total_leds: 0,
    });
    next.scene_name = Some(scene_name);
    next.scene_snapshot_locked = snapshot_locked;

    Some(next)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "serde_json exposes numeric metrics as f64; values are range-checked before narrowing for TUI display state"
)]
fn json_f32(value: &serde_json::Value) -> Option<f32> {
    let value = value.as_f64()?;
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return None;
    }

    Some(value as f32)
}

/// Parse the daemon state from the WebSocket hello message.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn parse_hello_state(hello: &serde_json::Value) -> Option<DaemonState> {
    let state = hello.get("state")?;
    Some(DaemonState {
        running: state.get("running")?.as_bool().unwrap_or(true),
        brightness: state
            .get("brightness")
            .and_then(serde_json::Value::as_u64)
            .map_or(100, |v| v.min(255) as u8),
        fps_target: state
            .get("fps")
            .and_then(|f| f.get("target"))
            .and_then(serde_json::Value::as_f64)
            .map_or(60.0, |v| v as f32),
        fps_actual: state
            .get("fps")
            .and_then(|f| f.get("actual"))
            .and_then(serde_json::Value::as_f64)
            .map_or(0.0, |v| v as f32),
        effect_name: state
            .get("effect")
            .and_then(|e| e.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        effect_id: state
            .get("effect")
            .and_then(|e| e.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        scene_name: state
            .get("scene")
            .and_then(|s| s.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        scene_snapshot_locked: state
            .get("scene")
            .and_then(|s| s.get("snapshot_locked"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        profile_name: state
            .get("profile")
            .and_then(|p| p.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        device_count: state
            .get("device_count")
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |v| v as u32),
        total_leds: state
            .get("total_leds")
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |v| v as u32),
    })
}
