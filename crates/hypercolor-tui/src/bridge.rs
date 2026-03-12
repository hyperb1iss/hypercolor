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

/// Spawn the data bridge as a background task.
///
/// Fetches initial state via REST, then streams live data via WebSocket.
/// Pushes `Action` variants to the provided sender. Shuts down when the
/// cancellation token fires.
pub async fn spawn_data_bridge(
    host: String,
    port: u16,
    action_tx: mpsc::UnboundedSender<Action>,
    cancel: CancellationToken,
) {
    let _ = action_tx.send(Action::DaemonReconnecting);

    // Phase 1: REST bootstrap
    let client = DaemonClient::new(&host, port);
    if let Err(e) = bootstrap_rest(&client, &action_tx).await {
        tracing::warn!("REST bootstrap failed: {e}");
        let _ = action_tx.send(Action::DaemonDisconnected(format!("{e}")));
        // Still try WebSocket — daemon might be partially available
    }

    // Phase 2: WebSocket streaming with reconnection
    loop {
        if cancel.is_cancelled() {
            break;
        }

        let (ws_tx, mut ws_rx) = mpsc::unbounded_channel();

        let ws_host = host.clone();
        let ws_cancel = cancel.clone();
        let ws_handle = tokio::spawn(async move {
            tokio::select! {
                result = ws::connect(&ws_host, port, ws_tx) => {
                    if let Err(e) = result {
                        tracing::warn!("WebSocket connection error: {e}");
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
                            if let Err(error) = refresh_for_event(&client, &action_tx, &event).await {
                                tracing::warn!(%error, "Failed to refresh TUI state after daemon event");
                            }
                        }
                        Some(WsMessage::Metrics(_metrics)) => {
                            // TODO: parse metrics and update state
                        }
                        Some(WsMessage::Closed) | None => {
                            let _ = action_tx.send(Action::DaemonDisconnected("WebSocket closed".into()));
                            break;
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
        tracing::info!("Reconnecting to daemon in 2s...");
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
) -> anyhow::Result<()> {
    let status = client.get_status().await?;
    let _ = action_tx.send(Action::DaemonConnected(Box::new(status)));

    refresh_effects(client, action_tx).await;
    refresh_devices(client, action_tx).await;
    refresh_favorites(client, action_tx).await;

    Ok(())
}

async fn refresh_for_event(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
    event: &serde_json::Value,
) -> anyhow::Result<()> {
    match event_name(event).unwrap_or_default() {
        name if name.starts_with("device_") => {
            refresh_status(client, action_tx).await?;
            refresh_devices(client, action_tx).await;
        }
        name if name.starts_with("effect_") => {
            refresh_status(client, action_tx).await?;
            refresh_effects(client, action_tx).await;
        }
        name if name.starts_with("profile_") || name == "session_changed" => {
            refresh_status(client, action_tx).await?;
        }
        _ => {}
    }

    Ok(())
}

async fn refresh_status(
    client: &DaemonClient,
    action_tx: &mpsc::UnboundedSender<Action>,
) -> anyhow::Result<()> {
    let status = client.get_status().await?;
    let _ = action_tx.send(Action::DaemonStateUpdated(Box::new(status)));
    Ok(())
}

async fn refresh_effects(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_effects().await {
        Ok(effects) => {
            let _ = action_tx.send(Action::EffectsUpdated(Arc::new(effects)));
        }
        Err(error) => {
            tracing::warn!(%error, "Failed to refresh effect list");
        }
    }
}

async fn refresh_devices(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_devices().await {
        Ok(devices) => {
            let _ = action_tx.send(Action::DevicesUpdated(Arc::new(devices)));
        }
        Err(error) => {
            tracing::warn!(%error, "Failed to refresh device list");
        }
    }
}

async fn refresh_favorites(client: &DaemonClient, action_tx: &mpsc::UnboundedSender<Action>) {
    match client.get_favorites().await {
        Ok(favorites) => {
            let _ = action_tx.send(Action::FavoritesUpdated(Arc::new(favorites)));
        }
        Err(error) => {
            tracing::warn!(%error, "Failed to refresh favorites");
        }
    }
}

fn event_name(event: &serde_json::Value) -> Option<&str> {
    event
        .get("event")
        .and_then(serde_json::Value::as_str)
        .or_else(|| event.get("event_type").and_then(serde_json::Value::as_str))
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
