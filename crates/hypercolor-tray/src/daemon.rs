//! HTTP + WebSocket client for daemon communication.
//!
//! Manages the connection to the Hypercolor daemon at `localhost:9420`,
//! fetches initial state via REST, subscribes to real-time updates via
//! WebSocket, and sends commands back to the daemon.

use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::state::{
    ApiEnvelope, AppState, DaemonMessage, EffectInfo, EffectListResponse, EffectSummary,
    ProfileInfo, ProfileListResponse, ProfileSummary, StateUpdate, StatusResponse, TrayCommand,
    WsEventMessage, WsHello,
};

/// Interval between reconnection attempts when the daemon is unreachable.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);

/// Daemon base URL.
const DEFAULT_DAEMON_URL: &str = "http://localhost:9420";

/// WebSocket URL derived from the daemon base URL.
const DEFAULT_WS_URL: &str = "ws://localhost:9420/api/v1/ws";

/// Manages communication with the Hypercolor daemon.
pub struct DaemonClient {
    base_url: String,
    ws_url: String,
    tx: mpsc::Sender<DaemonMessage>,
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<TrayCommand>,
    http: reqwest::Client,
}

impl DaemonClient {
    /// Create a new daemon client.
    ///
    /// `tx` sends [`DaemonMessage`]s to the tray UI thread.
    /// `cmd_rx` receives [`TrayCommand`]s from the tray UI thread.
    #[must_use]
    pub fn new(
        tx: mpsc::Sender<DaemonMessage>,
        cmd_rx: tokio::sync::mpsc::UnboundedReceiver<TrayCommand>,
    ) -> Self {
        Self {
            base_url: DEFAULT_DAEMON_URL.to_owned(),
            ws_url: DEFAULT_WS_URL.to_owned(),
            tx,
            cmd_rx,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("failed to build reqwest client"),
        }
    }

    /// Run the client forever, reconnecting as needed.
    ///
    /// This function never returns under normal operation. It loops between
    /// connecting, watching for events, and reconnecting on failure.
    pub async fn run(&mut self) {
        loop {
            match self.connect_and_watch().await {
                Ok(should_quit) => {
                    if should_quit {
                        info!("Daemon client shutting down");
                        return;
                    }
                    warn!("Daemon connection closed; reconnecting in 5s");
                }
                Err(e) => {
                    debug!("Daemon connection failed: {e}; retrying in 5s");
                }
            }

            let _ = self.tx.send(DaemonMessage::Disconnected);
            tokio::time::sleep(RECONNECT_INTERVAL).await;
        }
    }

    /// Attempt to connect to the daemon and watch for events.
    ///
    /// Returns `Ok(true)` if we should quit, `Ok(false)` if the connection
    /// was lost and we should reconnect, or `Err` on connection failure.
    async fn connect_and_watch(&mut self) -> anyhow::Result<bool> {
        // Step 1: Fetch initial state via REST.
        let state = self.fetch_initial_state().await?;
        let _ = self.tx.send(DaemonMessage::Connected(state));

        // Step 2: Connect to WebSocket for real-time updates.
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Subscribe to the events channel.
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "channels": ["events"]
        });
        ws_write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        info!("Connected to daemon WebSocket at {}", self.ws_url);

        // Step 3: Event loop — process WS messages and tray commands.
        loop {
            tokio::select! {
                ws_msg = ws_read.next() => {
                    match ws_msg {
                        Some(Ok(Message::Text(text))) => {
                            self.handle_ws_message(&text);
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = ws_write.send(Message::Pong(payload)).await;
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            info!("WebSocket connection closed");
                            return Ok(false);
                        }
                        Some(Err(e)) => {
                            warn!("WebSocket error: {e}");
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(TrayCommand::Quit) => return Ok(true),
                        Some(command) => {
                            self.handle_command(command).await;
                        }
                        None => {
                            // Command channel closed — tray exited.
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    /// Fetch initial state from the daemon REST API.
    async fn fetch_initial_state(&self) -> anyhow::Result<AppState> {
        // Fetch status.
        let status_url = format!("{}/api/v1/status", self.base_url);
        let status_resp: ApiEnvelope<StatusResponse> =
            self.http.get(&status_url).send().await?.json().await?;
        let status = status_resp
            .data
            .ok_or_else(|| anyhow::anyhow!("Missing data in status response"))?;

        // Fetch effects.
        let effects_url = format!("{}/api/v1/effects", self.base_url);
        let effects_resp: ApiEnvelope<EffectListResponse> =
            self.http.get(&effects_url).send().await?.json().await?;
        let effects: Vec<EffectInfo> = effects_resp
            .data
            .map(|list| {
                list.items
                    .into_iter()
                    .map(|s: EffectSummary| EffectInfo {
                        id: s.id,
                        name: s.name,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Fetch profiles.
        let profiles_url = format!("{}/api/v1/profiles", self.base_url);
        let profiles: Vec<ProfileInfo> = match self.http.get(&profiles_url).send().await {
            Ok(resp) => {
                let profile_resp: Result<ApiEnvelope<ProfileListResponse>, _> = resp.json().await;
                profile_resp
                    .ok()
                    .and_then(|envelope| envelope.data)
                    .map(|list| {
                        list.items
                            .into_iter()
                            .map(|s: ProfileSummary| ProfileInfo {
                                id: s.id,
                                name: s.name,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Err(e) => {
                debug!("Failed to fetch profiles: {e}");
                Vec::new()
            }
        };

        // Derive the current effect from the status active_effect name.
        // The status endpoint returns just the name, not the ID. Match against
        // the effects list to find the full info.
        let current_effect = status.active_effect.and_then(|name| {
            effects
                .iter()
                .find(|e| e.name == name)
                .cloned()
                .or_else(|| {
                    Some(EffectInfo {
                        id: String::new(),
                        name,
                    })
                })
        });

        Ok(AppState {
            connected: true,
            running: status.running,
            paused: false,
            brightness: status.global_brightness,
            current_effect,
            device_count: status.device_count,
            effects,
            profiles,
        })
    }

    /// Parse a WebSocket text message and send a state update if relevant.
    fn handle_ws_message(&self, text: &str) {
        let Ok(msg) = serde_json::from_str::<WsEventMessage>(text) else {
            debug!("Ignoring unparseable WS message");
            return;
        };

        // The daemon sends hello on connect — we already have initial state
        // from REST, but we can update from the hello if present.
        if msg.msg_type == "hello"
            && let Ok(hello) = serde_json::from_str::<WsHello>(text)
            && let Some(state) = hello.state
        {
            // Update brightness and pause state from hello.
            let _ = self
                .tx
                .send(DaemonMessage::StateUpdate(StateUpdate::BrightnessChanged(
                    state.brightness,
                )));
            if state.paused {
                let _ = self
                    .tx
                    .send(DaemonMessage::StateUpdate(StateUpdate::Paused));
            }
            if let Some(effect) = state.effect {
                let _ = self
                    .tx
                    .send(DaemonMessage::StateUpdate(StateUpdate::EffectChanged {
                        id: effect.id,
                        name: effect.name,
                    }));
            }
            return;
        }
        if msg.msg_type == "hello" {
            return;
        }

        // Only process event-type messages.
        if msg.msg_type != "event" {
            return;
        }

        let update = match msg.event.as_str() {
            "effect_started" => {
                let effect_data = &msg.data["effect"];
                let id = effect_data["id"].as_str().unwrap_or_default().to_owned();
                let name = effect_data["name"].as_str().unwrap_or_default().to_owned();
                if id.is_empty() && name.is_empty() {
                    return;
                }
                Some(StateUpdate::EffectChanged { id, name })
            }
            "effect_stopped" => Some(StateUpdate::EffectStopped),
            "brightness_changed" => {
                let new_value = msg.data["new_value"].as_u64().unwrap_or(0);
                #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
                let brightness = new_value.min(100) as u8;
                Some(StateUpdate::BrightnessChanged(brightness))
            }
            "paused" => Some(StateUpdate::Paused),
            "resumed" => Some(StateUpdate::Resumed),
            // All other events: the tray re-fetches full state on reconnect.
            _ => None,
        };

        if let Some(update) = update {
            let _ = self.tx.send(DaemonMessage::StateUpdate(update));
        }
    }

    /// Handle a command from the tray UI thread.
    async fn handle_command(&self, command: TrayCommand) {
        match command {
            TrayCommand::ApplyEffect(id) => {
                let url = format!("{}/api/v1/effects/{}/apply", self.base_url, id);
                if let Err(e) = self.http.post(&url).send().await {
                    error!("Failed to apply effect {id}: {e}");
                }
            }
            TrayCommand::ApplyProfile(id) => {
                let url = format!("{}/api/v1/profiles/{}/apply", self.base_url, id);
                if let Err(e) = self.http.post(&url).send().await {
                    error!("Failed to apply profile {id}: {e}");
                }
            }
            TrayCommand::StopEffect => {
                let url = format!("{}/api/v1/effects/stop", self.base_url);
                if let Err(e) = self.http.post(&url).send().await {
                    error!("Failed to stop effect: {e}");
                }
            }
            TrayCommand::SetBrightness(value) => {
                let url = format!("{}/api/v1/settings/brightness", self.base_url);
                let body = serde_json::json!({ "brightness": value });
                if let Err(e) = self.http.put(&url).json(&body).send().await {
                    error!("Failed to set brightness: {e}");
                }
            }
            TrayCommand::TogglePause => {
                // The daemon doesn't have a dedicated pause endpoint yet;
                // the tray toggles by stopping/resuming the effect. For now,
                // log that this is not yet implemented.
                warn!("Pause/resume toggle not yet implemented in daemon API");
            }
            TrayCommand::OpenWebUi => {
                let url = self.base_url.clone();
                tokio::task::spawn_blocking(move || open_web_ui(&url));
            }
            TrayCommand::Quit => {
                // Handled in the event loop before reaching here.
            }
        }
    }
}

/// Open the Hypercolor web UI in the default browser.
fn open_web_ui(base_url: &str) {
    if let Err(e) = open::that(base_url) {
        error!("Failed to open web UI: {e}");
    }
}
