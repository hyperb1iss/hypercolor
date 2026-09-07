//! HTTP + WebSocket client for daemon communication.
//!
//! Manages the connection to Hypercolor daemons, including mDNS discovery,
//! server switching, REST bootstrap, and WebSocket subscriptions.

use std::net::IpAddr;
use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hypercolor_core::config::{paths, servers};
use hypercolor_core::device::discover_servers;
use hypercolor_types::api::ApiResponse;
use hypercolor_types::api::effects::{EffectListResponse, EffectSummary};
use hypercolor_types::api::output::{OutputPowerMode, OutputResource};
use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scenes::{SceneListResponse, SceneSummary};
use hypercolor_types::api::system::{SystemResource, SystemStatus};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_types::server::{DiscoveredServer, ServerIdentity};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::state::{
    AppState, DaemonMessage, EffectInfo, SceneInfo, ServerEntry, StateUpdate, TrayCommand,
    WsEventMessage, WsHello,
};

/// Interval between reconnection attempts when the daemon is unreachable.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: u16 = 9420;

/// Manages communication with the Hypercolor daemon.
pub struct DaemonClient {
    base_url: String,
    ws_url: String,
    active_server_id: Option<String>,
    active_api_key: Option<String>,
    known_servers: Vec<ServerEntry>,
    stored_api_keys: Vec<StoredServerApiKey>,
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
            active_api_key: None,
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
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "topics": [{ "topic": "events" }]
        });
        ws_write
            .send(Message::Text(subscribe_msg.to_string().into()))
            .await?;
        wait_for_subscription_ack(&mut ws_read, SUBSCRIPTION_TIMEOUT).await?;

        let state = self.fetch_initial_state().await?;
        if let (Some(expected_id), Some(server)) = (&self.active_server_id, &state.server_identity)
            && expected_id != &server.instance_id
        {
            anyhow::bail!(
                "discovered server identity changed from {expected_id} to {}",
                server.instance_id
            );
        }
        if self.active_server_id.is_none() {
            self.active_server_id = state
                .server_identity
                .as_ref()
                .map(|server| server.instance_id.clone());
        }
        let _ = self.tx.send(DaemonMessage::Connected(state));

        info!("Connected to daemon WebSocket");

        loop {
            tokio::select! {
                ws_msg = ws_read.next() => {
                    match ws_msg {
                        Some(Ok(Message::Text(text))) => {
                            self.handle_ws_message(&text).await?;
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
        let system = self.fetch_system().await?;
        let server = system.identity;
        let status = system
            .status
            .ok_or_else(|| anyhow::anyhow!("System status requires daemon read access"))?;
        let power = self.fetch_output().await?;

        let effects_url = format!("{}/api/v1/effects", self.base_url);
        let effects_resp: ApiResponse<EffectListResponse> = self
            .auth_request(self.http.get(&effects_url))
            .send()
            .await?
            .json()
            .await?;
        let effects: Vec<EffectInfo> = effects_resp
            .data
            .items
            .into_iter()
            .map(|item: EffectSummary| EffectInfo {
                id: item.id,
                name: item.name,
            })
            .collect();

        let scenes_url = format!("{}/api/v1/scenes", self.base_url);
        let scenes: Vec<SceneInfo> =
            match self.auth_request(self.http.get(&scenes_url)).send().await {
                Ok(response) => {
                    let scene_resp: Result<ApiResponse<SceneListResponse>, _> =
                        response.json().await;
                    scene_resp
                        .ok()
                        .map(|list| {
                            list.data
                                .items
                                .into_iter()
                                .map(|item: SceneSummary| SceneInfo {
                                    id: item.id,
                                    name: item.name,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
                Err(error) => {
                    debug!("Failed to fetch scenes: {error}");
                    Vec::new()
                }
            };

        let active_effect = status.active_effect.and_then(|name| {
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
            paused: power.power == OutputPowerMode::Paused,
            brightness: status.global_brightness,
            active_effect,
            active_scene_name: status.active_scene,
            scene_snapshot_locked: status.active_scene_snapshot_locked,
            device_count: status.device_count,
            effects,
            scenes,
            server_identity: Some(server_identity.clone()),
            servers: self.known_servers.clone(),
            active_server: self.find_server_index(&server_identity.instance_id),
        })
    }

    async fn fetch_status(&self) -> anyhow::Result<SystemStatus> {
        self.fetch_system()
            .await?
            .status
            .ok_or_else(|| anyhow::anyhow!("System status requires daemon read access"))
    }

    async fn fetch_system(&self) -> anyhow::Result<SystemResource> {
        let url = format!("{}/api/v1/system", self.base_url);
        let response: ApiResponse<SystemResource> = self
            .auth_request(self.http.get(&url))
            .send()
            .await?
            .json()
            .await?;
        Ok(response.data)
    }

    async fn fetch_output(&self) -> anyhow::Result<OutputResource> {
        let url = format!("{}/api/v1/output", self.base_url);
        let response: ApiResponse<OutputResource> = self
            .auth_request(self.http.get(&url))
            .send()
            .await?
            .json()
            .await?;
        Ok(response.data)
    }

    async fn fetch_primary_zone_id(&self) -> anyhow::Result<Option<ZoneId>> {
        let url = format!("{}/api/v1/scene", self.base_url);
        let response: ApiResponse<SceneDocument> = self
            .auth_request(self.http.get(&url))
            .send()
            .await?
            .json()
            .await?;
        let scene = response.data;
        Ok(scene
            .zones
            .into_iter()
            .find(|zone| zone.role == ZoneRole::Primary)
            .map(|zone| zone.id))
    }

    async fn event_targets_primary_zone(&self, message: &WsEventMessage) -> bool {
        match self.fetch_primary_zone_id().await {
            Ok(Some(zone_id)) => message.targets_zone(&zone_id),
            Ok(None) => false,
            Err(error) => {
                debug!("Failed to resolve primary zone for lifecycle event: {error}");
                false
            }
        }
    }

    /// Parse a WebSocket text message and send a state update if relevant.
    async fn handle_ws_message(&self, text: &str) -> anyhow::Result<()> {
        let Ok(msg) = serde_json::from_str::<WsEventMessage>(text) else {
            debug!("Ignoring unparseable WS message");
            return Ok(());
        };

        if msg.msg_type == "hello"
            && let Ok(hello) = serde_json::from_str::<WsHello>(text)
            && let Some(state) = hello.state
        {
            let _ = self
                .tx
                .send(DaemonMessage::StateUpdate(StateUpdate::Snapshot {
                    running: state.running,
                    paused: state.paused,
                    brightness: state.brightness,
                    device_count: state.device_count,
                }));
            return Ok(());
        }

        if msg.msg_type != "event" {
            return Ok(());
        }

        if msg.requires_full_resync() {
            let state = self.fetch_initial_state().await?;
            let _ = self.tx.send(DaemonMessage::Connected(state));
            return Ok(());
        }

        let update = match msg.event.as_str() {
            "active_scene_changed" => {
                let scene_name = msg
                    .data
                    .get("current_name")
                    .or_else(|| msg.data.get("scene_name"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                let snapshot_locked = msg
                    .data
                    .get("current_snapshot_locked")
                    .or_else(|| msg.data.get("snapshot_locked"))
                    .and_then(serde_json::Value::as_bool);

                match (scene_name, snapshot_locked) {
                    (Some(name), Some(snapshot_locked)) => Some(StateUpdate::SceneChanged {
                        name: Some(name),
                        snapshot_locked,
                    }),
                    _ => match self.fetch_status().await {
                        Ok(status) => Some(StateUpdate::SceneChanged {
                            name: status.active_scene,
                            snapshot_locked: status.active_scene_snapshot_locked,
                        }),
                        Err(error) => {
                            debug!("Failed to refresh tray scene state: {error}");
                            None
                        }
                    },
                }
            }
            "effect_started" if self.event_targets_primary_zone(&msg).await => {
                let effect_data = &msg.data["effect"];
                let id = effect_data["id"].as_str().unwrap_or_default().to_owned();
                let name = effect_data["name"].as_str().unwrap_or_default().to_owned();
                if id.is_empty() && name.is_empty() {
                    return Ok(());
                }
                Some(StateUpdate::EffectChanged { id, name })
            }
            "effect_started" => None,
            "effect_stopped"
                if msg.is_destructive_effect_stop()
                    && self.event_targets_primary_zone(&msg).await =>
            {
                Some(StateUpdate::EffectStopped)
            }
            "effect_stopped" => None,
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
        Ok(())
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
            TrayCommand::ActivateScene(id) => {
                let url = format!("{}/api/v1/scenes/{}/activate", self.base_url, id);
                if let Err(error) = self
                    .send_command(self.auth_request(self.http.post(&url)), "activate scene")
                    .await
                {
                    error!("Failed to activate scene {id}: {error}");
                }
                false
            }
            TrayCommand::StopEffect => {
                let url = format!("{}/api/v1/scene/clear", self.base_url);
                if let Err(error) = self
                    .send_command(self.auth_request(self.http.post(&url)), "clear scene")
                    .await
                {
                    error!("Failed to clear scene: {error}");
                }
                false
            }
            TrayCommand::SetBrightness(value) => {
                let url = format!("{}/api/v1/output", self.base_url);
                let body = serde_json::json!({ "brightness": f32::from(value) / 100.0 });
                if let Err(error) = self
                    .send_command(
                        self.auth_request(self.http.patch(&url)).json(&body),
                        "set brightness",
                    )
                    .await
                {
                    error!("Failed to set brightness: {error}");
                }
                false
            }
            TrayCommand::SetPaused(paused) => {
                let url = format!("{}/api/v1/output", self.base_url);
                let state = if paused { "paused" } else { "running" };
                let body = serde_json::json!({ "power": state });
                if let Err(error) = self
                    .send_command(
                        self.auth_request(self.http.patch(&url)).json(&body),
                        "set output power",
                    )
                    .await
                {
                    error!("Failed to set output power to {state}: {error}");
                }
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
                    .map(|server| {
                        let has_api_key = self.api_key_for_server(&server).is_some();
                        ServerEntry {
                            server,
                            has_api_key,
                        }
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
        let api_key = self.api_key_for_server(&entry.server).map(str::to_owned);
        let next_base = build_base_url(&host, entry.server.port);
        let next_ws = build_ws_url(&host, entry.server.port, api_key.as_deref());
        let changed = self.base_url != next_base
            || self.ws_url != next_ws
            || self.active_server_id.as_deref() != Some(entry.server.identity.instance_id.as_str());

        self.base_url = next_base;
        self.ws_url = next_ws;
        self.active_server_id = Some(entry.server.identity.instance_id.clone());
        self.active_api_key = api_key;
        changed
    }

    fn auth_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.active_api_key {
            return request.bearer_auth(api_key);
        }

        request
    }

    fn api_key_for_server(&self, server: &DiscoveredServer) -> Option<&str> {
        api_key_for_server(&self.stored_api_keys, server)
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

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SubscriptionAdmission {
    Subscribed,
    Error {
        message: Option<String>,
    },
    #[serde(other)]
    Other,
}

async fn wait_for_subscription_ack<S>(read: &mut S, timeout: Duration) -> anyhow::Result<()>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(timeout, async {
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let Ok(message) = serde_json::from_str::<SubscriptionAdmission>(&text) else {
                        continue;
                    };
                    match message {
                        SubscriptionAdmission::Subscribed => return Ok(()),
                        SubscriptionAdmission::Error { message } => {
                            let detail = message.as_deref().unwrap_or("subscription rejected");
                            anyhow::bail!("daemon rejected event subscription: {detail}");
                        }
                        SubscriptionAdmission::Other => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    anyhow::bail!("WebSocket closed before subscription acknowledgment");
                }
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("event subscription acknowledgment timed out"))?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredServerApiKey {
    pub instance_id: String,
    pub host: IpAddr,
    pub port: u16,
    pub api_key: String,
}

impl StoredServerApiKey {
    fn from_credential(credential: servers::StoredServerCredential) -> Option<Self> {
        let Some((host, port)) = credential.endpoint() else {
            warn!(
                instance_id = credential.instance_id(),
                "Ignoring servers.toml entry without a host/port binding; re-authenticate this daemon"
            );
            return None;
        };
        Some(Self {
            instance_id: credential.instance_id().to_owned(),
            host,
            port,
            api_key: credential.api_key().to_owned(),
        })
    }

    fn matches_server(&self, server: &DiscoveredServer) -> bool {
        self.instance_id == server.identity.instance_id
            && self.host == server.host
            && self.port == server.port
    }
}

fn load_server_api_keys() -> Vec<StoredServerApiKey> {
    let path = paths::config_dir().join("servers.toml");
    match servers::load_server_credentials(&path) {
        Ok(credentials) => credentials
            .into_iter()
            .filter_map(StoredServerApiKey::from_credential)
            .collect(),
        Err(error) => {
            debug!(path = %path.display(), %error, "Failed to load stored server credentials");
            Vec::new()
        }
    }
}

pub fn api_key_for_server<'a>(
    stored_api_keys: &'a [StoredServerApiKey],
    server: &DiscoveredServer,
) -> Option<&'a str> {
    stored_api_keys
        .iter()
        .rfind(|credential| credential.matches_server(server))
        .map(|credential| credential.api_key.as_str())
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

#[cfg(test)]
mod subscription_tests {
    use std::time::Duration;

    use futures_util::stream;
    use tokio_tungstenite::tungstenite::{Error, Message};

    use super::wait_for_subscription_ack;

    #[tokio::test]
    async fn subscription_rejection_fails_connection_admission() {
        let mut messages = stream::iter([Ok::<_, Error>(Message::Text(
            r#"{"type":"error","message":"forbidden"}"#.into(),
        ))]);

        let error = wait_for_subscription_ack(&mut messages, Duration::from_secs(1))
            .await
            .expect_err("subscription rejection must fail admission");

        assert!(error.to_string().contains("forbidden"));
    }

    #[tokio::test]
    async fn subscribed_ack_admits_authoritative_rest_reconciliation() {
        let mut messages = stream::iter([Ok::<_, Error>(Message::Text(
            r#"{"type":"subscribed","topics":[{"topic":"events"}]}"#.into(),
        ))]);

        wait_for_subscription_ack(&mut messages, Duration::from_secs(1))
            .await
            .expect("typed acknowledgment should admit REST reconciliation");
    }

    #[tokio::test]
    async fn subscription_timeout_fails_connection_admission() {
        let mut messages = stream::pending::<Result<Message, Error>>();

        let error = wait_for_subscription_ack(&mut messages, Duration::ZERO)
            .await
            .expect_err("missing acknowledgment must fail admission");

        assert!(error.to_string().contains("timed out"));
    }
}
