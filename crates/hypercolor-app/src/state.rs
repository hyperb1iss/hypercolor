//! Application state management for the unified desktop app.
//!
//! Defines the shared state synchronized from the daemon via WebSocket,
//! along with message types for cross-thread communication between the
//! async daemon client and native app UI surfaces.

use hypercolor_types::server::{DiscoveredServer, ServerIdentity};
use serde::Deserialize;

/// Applet state synchronized from the daemon via WebSocket.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AppState {
    /// Whether the tray is connected to the daemon.
    pub connected: bool,
    /// Whether the daemon is running.
    pub running: bool,
    /// Whether rendering is paused.
    pub paused: bool,
    /// Global brightness percentage (0-100).
    pub brightness: u8,
    /// Currently active effect, if any.
    pub current_effect: Option<EffectInfo>,
    /// Currently active scene name, if known.
    pub active_scene_name: Option<String>,
    /// Whether the active scene blocks live mutation.
    pub scene_snapshot_locked: bool,
    /// Number of connected devices.
    pub device_count: usize,
    /// All available effects from the daemon registry.
    pub effects: Vec<EffectInfo>,
    /// All available reusable scenes.
    pub scenes: Vec<SceneInfo>,
    /// Connected server identity, when known.
    pub server_identity: Option<ServerIdentity>,
    /// Discovered Hypercolor servers on the local network.
    pub servers: Vec<ServerEntry>,
    /// Selected server index within `servers`.
    pub active_server: Option<usize>,
}

impl AppState {
    /// Create a new disconnected state with all fields zeroed/empty.
    #[must_use]
    pub fn disconnected() -> Self {
        Self {
            connected: false,
            running: false,
            paused: false,
            brightness: 0,
            current_effect: None,
            active_scene_name: None,
            scene_snapshot_locked: false,
            device_count: 0,
            effects: Vec::new(),
            scenes: Vec::new(),
            server_identity: None,
            servers: Vec::new(),
            active_server: None,
        }
    }

    /// Apply a daemon client message to this state snapshot.
    pub fn apply_daemon_message(&mut self, message: DaemonMessage) {
        match message {
            DaemonMessage::Connected(next_state) => *self = next_state,
            DaemonMessage::Disconnected => {
                self.connected = false;
                self.running = false;
                self.paused = false;
                self.current_effect = None;
                self.active_scene_name = None;
                self.scene_snapshot_locked = false;
                self.device_count = 0;
                self.server_identity = None;
                self.active_server = None;
            }
            DaemonMessage::ServersUpdated(servers) => {
                self.servers = servers;
            }
            DaemonMessage::StateUpdate(update) => self.apply_state_update(update),
        }
    }

    fn apply_state_update(&mut self, update: StateUpdate) {
        match update {
            StateUpdate::EffectChanged { id, name } => {
                self.current_effect = Some(EffectInfo { id, name });
                self.paused = false;
            }
            StateUpdate::EffectStopped => {
                self.current_effect = None;
                self.paused = false;
            }
            StateUpdate::SceneChanged {
                name,
                snapshot_locked,
            } => {
                self.active_scene_name = name;
                self.scene_snapshot_locked = snapshot_locked;
            }
            StateUpdate::BrightnessChanged(brightness) => {
                self.brightness = brightness;
            }
            StateUpdate::Paused => {
                self.paused = true;
            }
            StateUpdate::Resumed => {
                self.paused = false;
            }
            StateUpdate::DeviceCountChanged(device_count) => {
                self.device_count = device_count;
            }
            StateUpdate::EffectsRefreshed(effects) => {
                self.effects = effects;
            }
            StateUpdate::Snapshot {
                running,
                paused,
                brightness,
                device_count,
            } => {
                // The handshake says nothing about content: the live
                // tree is multi-zone, so what is rendering arrives from
                // the effect lifecycle events instead (Spec 78 §7.1).
                self.running = running;
                self.paused = paused;
                self.brightness = brightness;
                self.device_count = device_count;
            }
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::disconnected()
    }
}

/// Lightweight effect information for display in the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectInfo {
    pub id: String,
    pub name: String,
}

/// Lightweight scene information for display in the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneInfo {
    pub id: String,
    pub name: String,
}

/// A discovered server plus local credential availability.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    pub server: DiscoveredServer,
    pub has_api_key: bool,
}

/// Messages from the async daemon client to the tray UI thread.
#[derive(Debug, Clone)]
pub enum DaemonMessage {
    /// Initial connection established; full state snapshot.
    Connected(AppState),
    /// Connection to the daemon was lost.
    Disconnected,
    /// The set of discoverable servers changed.
    ServersUpdated(Vec<ServerEntry>),
    /// Incremental state update from a WebSocket event.
    StateUpdate(StateUpdate),
}

/// Incremental state updates parsed from daemon WebSocket events.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StateUpdate {
    /// The active effect changed.
    EffectChanged { id: String, name: String },
    /// The active effect was stopped.
    EffectStopped,
    /// The active scene changed.
    SceneChanged {
        name: Option<String>,
        snapshot_locked: bool,
    },
    /// Global brightness changed.
    BrightnessChanged(u8),
    /// Rendering was paused.
    Paused,
    /// Rendering was resumed.
    Resumed,
    /// Device count changed (connected or disconnected).
    DeviceCountChanged(usize),
    /// Effect list was updated (rescan).
    EffectsRefreshed(Vec<EffectInfo>),
    /// Authoritative state snapshot from the WebSocket hello message.
    Snapshot {
        running: bool,
        paused: bool,
        brightness: u8,
        device_count: usize,
    },
}

/// Commands from the tray UI thread to the async daemon client.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TrayCommand {
    /// Apply the given effect by ID.
    ApplyEffect(String),
    /// Activate the given scene by ID.
    ActivateScene(String),
    /// Stop the currently active effect.
    StopEffect,
    /// Set global brightness (0-100).
    SetBrightness(u8),
    /// Set the reversible global output pause state.
    SetPaused(bool),
    /// Open the web UI in the default browser.
    OpenWebUi,
    /// Switch the active daemon connection.
    SwitchServer(usize),
    /// Refresh the list of discoverable daemons.
    RefreshServers,
    /// Quit the tray applet.
    Quit,
}

// ── WebSocket response types (deserialization only) ────────────────────

/// WebSocket hello message from the daemon.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WsHello {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub server: Option<ServerIdentity>,
    pub state: Option<WsHelloState>,
}

/// State snapshot included in the WebSocket hello message.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WsHelloState {
    pub running: bool,
    pub paused: bool,
    pub brightness: u8,
    pub device_count: usize,
}

/// A generic WebSocket event message from the daemon.
#[derive(Debug, Deserialize)]
pub struct WsEventMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl WsEventMessage {
    #[must_use]
    pub fn requires_full_resync(&self) -> bool {
        self.msg_type == "event" && self.event == "resync_required"
    }

    #[must_use]
    pub fn is_destructive_effect_stop(&self) -> bool {
        self.msg_type == "event"
            && self.event == "effect_stopped"
            && self.data.get("reason").and_then(serde_json::Value::as_str) == Some("stopped")
    }

    #[must_use]
    pub fn targets_zone(&self, zone_id: &hypercolor_types::scene::ZoneId) -> bool {
        self.data
            .get("zone_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == zone_id.to_string())
    }
}
