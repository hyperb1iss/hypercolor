//! HTTP + WebSocket client for daemon communication.
//!
//! Manages the connection to Hypercolor daemons, including mDNS discovery,
//! server switching, REST bootstrap, and WebSocket subscriptions.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hypercolor_core::config::paths;
use hypercolor_core::device::discover_servers;
use hypercolor_types::server::ServerIdentity;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::state::{
    ApiEnvelope, AppState, DaemonMessage, EffectInfo, EffectListResponse, EffectSummary,
    ProfileInfo, ProfileListResponse, ProfileSummary, ServerEntry, ServerResponse, StateUpdate,
    StatusResponse, TrayCommand, WsEventMessage, WsHello,
};

/// Interval between reconnection attempts when the daemon is unreachable.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: u16 = 9420;

/// Manages communication with the Hypercolor daemon.
pub struct DaemonClient {
    base_url: String,
    ws_url: String,
    active_server_id: Option<String>,
    known_servers: Vec<ServerEntry>,
    stored_api_keys: HashMap<String, String>,
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
            base_url: build_base_url(DEFAULT_HOST, DEFAULT_PORT),
            ws_url: build_ws_url(DEFAULT_HOST, DEFAULT_PORT, None),
            active_server_id: None,
            known_servers: Vec::new(),
            stored_api_keys: load_server_api_keys(),
            tx,
            cmd_rx,
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("failed to build reqwest client"),
        }
    }

    /// Run the client forever, reconnecting as needed.
    pub async fn run(&mut self) {
        let _ = self.refresh_servers(true).await;

        loop {
            match self.connect_and_watch().await {
                Ok(should_quit) => {
                    if should_quit {
                        info!("Daemon client shutting down");
                        return;
                    }
                    warn!("Daemon connection closed; reconnecting in 5s");
                }
                Err(error) => {
                    debug!("Daemon connection failed: {error}; retrying in 5s");
                }
            }

            let _ = self.tx.send(DaemonMessage::Disconnected);
            tokio::time::sleep(RECONNECT_INTERVAL).await;
        }
    }

    /// Attempt to connect to the daemon and watch for events.
    async fn connect_and_watch(&mut self) -> anyhow::Result<bool> {
        let state = self.fetch_initial_state().await?;
        self.active_server_id = state
            .server_identity
            .as_ref()
            .map(|server| server.instance_id.clone());
        let _ = self.tx.send(DaemonMessage::Connected(state));

        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "channels": ["events"]
        });
        ws_write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;

        info!("Connected to daemon WebSocket at {}", self.ws_url);

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
                        Some(Err(error)) => {
                            warn!("WebSocket error: {error}");
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(TrayCommand::Quit) | None => return Ok(true),
                        Some(command) => {
                            if self.handle_command(command).await {
                                return Ok(false);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Fetch initial state from the daemon REST API.
    async fn fetch_initial_state(&self) -> anyhow::Result<AppState> {
        let server_url = format!("{}/api/v1/server", self.base_url);
        let server_resp: ApiEnvelope<ServerResponse> = self
            .auth_request(self.http.get(&server_url))
            .send()
            .await?
            .json()
            .await?;
        let server = server_resp
            .data
            .ok_or_else(|| anyhow::anyhow!("Missing data in server response"))?;

        let status_url = format!("{}/api/v1/status", self.base_url);
        let status_resp: ApiEnvelope<StatusResponse> = self
            .auth_request(self.http.get(&status_url))
            .send()
            .await?
            .json()
            .await?;
        let status = status_resp
            .data
            .ok_or_else(|| anyhow::anyhow!("Missing data in status response"))?;

        let effects_url = format!("{}/api/v1/effects", self.base_url);
        let effects_resp: ApiEnvelope<EffectListResponse> = self
            .auth_request(self.http.get(&effects_url))
            .send()
            .await?
            .json()
            .await?;
        let effects: Vec<EffectInfo> = effects_resp
            .data
            .map(|list| {
                list.items
                    .into_iter()
                    .map(|item: EffectSummary| EffectInfo {
                        id: item.id,
                        name: item.name,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let profiles_url = format!("{}/api/v1/profiles", self.base_url);
        let profiles: Vec<ProfileInfo> =
            match self.auth_request(self.http.get(&profiles_url)).send().await {
                Ok(response) => {
                    let profile_resp: Result<ApiEnvelope<ProfileListResponse>, _> =
                        response.json().await;
                    profile_resp
                        .ok()
                        .and_then(|envelope| envelope.data)
                        .map(|list| {
                            list.items
                                .into_iter()
                                .map(|item: ProfileSummary| ProfileInfo {
                                    id: item.id,
                                    name: item.name,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
                Err(error) => {
                    debug!("Failed to fetch profiles: {error}");
                    Vec::new()
                }
            };

        let current_effect = status.active_effect.and_then(|name| {
            effects
                .iter()
                .find(|effect| effect.name == name)
                .cloned()
                .or_else(|| {
                    Some(EffectInfo {
                        id: String::new(),
                        name,
                    })
                })
        });

        let server_identity = ServerIdentity {
            instance_id: server.instance_id,
            instance_name: server.instance_name,
            version: server.version,
        };

        Ok(AppState {
            connected: true,
            running: status.running,
            paused: false,
            brightness: status.global_brightness,
            current_effect,
            device_count: status.device_count,
            effects,
            profiles,
            server_identity: Some(server_identity.clone()),
            servers: self.known_servers.clone(),
            active_server: self.find_server_index(&server_identity.instance_id),
        })
    }

    /// Parse a WebSocket text message and send a state update if relevant.
    fn handle_ws_message(&self, text: &str) {
        let Ok(msg) = serde_json::from_str::<WsEventMessage>(text) else {
            debug!("Ignoring unparseable WS message");
            return;
        };

        if msg.msg_type == "hello"
            && let Ok(hello) = serde_json::from_str::<WsHello>(text)
            && let Some(state) = hello.state
        {
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
            _ => None,
        };

        if let Some(update) = update {
            let _ = self.tx.send(DaemonMessage::StateUpdate(update));
        }
    }

    /// Handle a command from the tray UI thread.
    ///
    /// Returns `true` when the current connection should be torn down so the
    /// outer loop can reconnect with updated target settings.
    async fn handle_command(&mut self, command: TrayCommand) -> bool {
        match command {
            TrayCommand::ApplyEffect(id) => {
                let url = format!("{}/api/v1/effects/{}/apply", self.base_url, id);
                if let Err(error) = self
                    .send_command(self.auth_request(self.http.post(&url)), "apply effect")
                    .await
                {
                    error!("Failed to apply effect {id}: {error}");
                }
                false
            }
            TrayCommand::ApplyProfile(id) => {
                let url = format!("{}/api/v1/profiles/{}/apply", self.base_url, id);
                if let Err(error) = self
                    .send_command(self.auth_request(self.http.post(&url)), "apply profile")
                    .await
                {
                    error!("Failed to apply profile {id}: {error}");
                }
                false
            }
            TrayCommand::StopEffect => {
                let url = format!("{}/api/v1/effects/stop", self.base_url);
                if let Err(error) = self
                    .send_command(self.auth_request(self.http.post(&url)), "stop effect")
                    .await
                {
                    error!("Failed to stop effect: {error}");
                }
                false
            }
            TrayCommand::SetBrightness(value) => {
                let url = format!("{}/api/v1/settings/brightness", self.base_url);
                let body = serde_json::json!({ "brightness": value });
                if let Err(error) = self
                    .send_command(
                        self.auth_request(self.http.put(&url)).json(&body),
                        "set brightness",
                    )
                    .await
                {
                    error!("Failed to set brightness: {error}");
                }
                false
            }
            TrayCommand::TogglePause => {
                warn!("Pause/resume toggle not yet implemented in daemon API");
                false
            }
            TrayCommand::OpenWebUi => {
                let url = self.base_url.clone();
                tokio::task::spawn_blocking(move || open_web_ui(&url));
                false
            }
            TrayCommand::SwitchServer(index) => self.switch_server(index),
            TrayCommand::RefreshServers => self.refresh_servers(false).await,
            TrayCommand::Quit => false,
        }
    }

    async fn refresh_servers(&mut self, allow_auto_switch: bool) -> bool {
        match discover_servers(DISCOVERY_TIMEOUT).await {
            Ok(servers) => {
                self.known_servers = servers
                    .into_iter()
                    .map(|server| ServerEntry {
                        has_api_key: self
                            .stored_api_keys
                            .contains_key(&server.identity.instance_id),
                        server,
                    })
                    .collect();

                let mut reconnect = false;

                if let Some(active_id) = self.active_server_id.clone()
                    && let Some(index) = self.find_server_index(&active_id)
                {
                    reconnect = self.switch_server(index);
                } else if allow_auto_switch && self.known_servers.len() == 1 {
                    reconnect = self.switch_server(0);
                }

                self.send_servers_updated();
                reconnect
            }
            Err(error) => {
                debug!("Failed to refresh Hypercolor servers: {error}");
                false
            }
        }
    }

    fn send_servers_updated(&self) {
        let _ = self
            .tx
            .send(DaemonMessage::ServersUpdated(self.known_servers.clone()));
    }

    fn find_server_index(&self, instance_id: &str) -> Option<usize> {
        self.known_servers
            .iter()
            .position(|entry| entry.server.identity.instance_id == instance_id)
    }

    fn switch_server(&mut self, index: usize) -> bool {
        let Some(entry) = self.known_servers.get(index) else {
            return false;
        };

        let host = entry.server.host.to_string();
        let api_key = self
            .stored_api_keys
            .get(&entry.server.identity.instance_id)
            .map(String::as_str);
        let next_base = build_base_url(&host, entry.server.port);
        let next_ws = build_ws_url(&host, entry.server.port, api_key);
        let changed = self.base_url != next_base
            || self.ws_url != next_ws
            || self.active_server_id.as_deref() != Some(entry.server.identity.instance_id.as_str());

        self.base_url = next_base;
        self.ws_url = next_ws;
        self.active_server_id = Some(entry.server.identity.instance_id.clone());
        changed
    }

    fn auth_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(active_id) = &self.active_server_id
            && let Some(api_key) = self.stored_api_keys.get(active_id)
        {
            return request.bearer_auth(api_key);
        }

        request
    }

    async fn send_command(
        &self,
        request: reqwest::RequestBuilder,
        action: &str,
    ) -> anyhow::Result<()> {
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let body = response
            .text()
            .await
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        match body {
            Some(body) => Err(anyhow::anyhow!("{action} returned HTTP {status}: {body}")),
            None => Err(anyhow::anyhow!("{action} returned HTTP {status}")),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct StoredServersFile {
    #[serde(default)]
    servers: Vec<StoredServerConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct StoredServerConfig {
    instance_id: String,
    api_key: String,
}

fn load_server_api_keys() -> HashMap<String, String> {
    let path = paths::config_dir().join("servers.toml");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };

    match toml::from_str::<StoredServersFile>(&contents) {
        Ok(file) => file
            .servers
            .into_iter()
            .filter_map(|entry| {
                let instance_id = entry.instance_id.trim();
                let api_key = entry.api_key.trim();
                if instance_id.is_empty() || api_key.is_empty() {
                    None
                } else {
                    Some((instance_id.to_owned(), api_key.to_owned()))
                }
            })
            .collect(),
        Err(error) => {
            debug!(path = %path.display(), %error, "Failed to parse tray server config");
            HashMap::new()
        }
    }
}

fn build_base_url(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

fn build_ws_url(host: &str, port: u16, api_key: Option<&str>) -> String {
    let base = format!("ws://{host}:{port}/api/v1/ws");
    api_key.map_or(base.clone(), |key| {
        format!("{base}?token={}", percent_encode(key))
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

/// Open the Hypercolor web UI in the default browser.
fn open_web_ui(base_url: &str) {
    if let Err(error) = open::that(base_url) {
        error!("Failed to open web UI: {error}");
    }
}
