//! Configuration types -- daemon, audio, web, TUI, discovery, and feature flag settings.
//!
//! All config structs derive `Serialize`/`Deserialize` with `#[serde(default)]` on
//! every optional section for forward/backward compatibility. A fresh install with
//! zero config files boots the daemon entirely from compile-time defaults.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::session::SessionConfig;

// ─── Default Value Functions ─────────────────────────────────────────────────
// Referenced by `#[serde(default = "defaults::...")]` throughout this module.

mod defaults {
    use super::LogLevel;
    use super::RenderAccelerationMode;
    use super::ShutdownBehavior;

    // Daemon
    pub fn listen_address() -> String {
        "127.0.0.1".into()
    }
    pub fn port() -> u16 {
        9420
    }
    pub fn target_fps() -> u32 {
        30
    }
    pub fn canvas_width() -> u32 {
        640
    }
    pub fn canvas_height() -> u32 {
        480
    }
    pub fn max_devices() -> u32 {
        32
    }
    pub fn log_level() -> LogLevel {
        LogLevel::Info
    }
    pub fn start_profile() -> String {
        "last".into()
    }
    pub fn shutdown_behavior() -> ShutdownBehavior {
        ShutdownBehavior::HardwareDefault
    }
    pub fn shutdown_color() -> String {
        "#1a1a2e".into()
    }

    // Web
    pub fn websocket_fps() -> u32 {
        30
    }

    // MCP
    pub fn mcp_base_path() -> String {
        "/mcp".into()
    }
    pub fn sse_keep_alive_secs() -> u64 {
        15
    }

    // Audio
    pub fn audio_device() -> String {
        "default".into()
    }
    pub fn fft_size() -> u32 {
        1024
    }
    pub fn smoothing() -> f32 {
        0.8
    }
    pub fn noise_gate() -> f32 {
        0.02
    }
    pub fn beat_sensitivity() -> f32 {
        0.6
    }

    // Capture
    pub fn capture_source() -> String {
        "auto".into()
    }
    pub fn capture_fps() -> u32 {
        30
    }

    // Discovery
    pub fn scan_interval() -> u64 {
        300
    }
    pub fn govee_lan_state_fps() -> u32 {
        10
    }
    pub fn govee_razer_fps() -> u32 {
        25
    }
    // Network
    pub fn remote_access() -> bool {
        false
    }

    // D-Bus
    pub fn bus_name() -> String {
        "tech.hyperbliss.hypercolor1".into()
    }

    // TUI
    pub fn tui_theme() -> String {
        "silkcircuit".into()
    }
    pub fn preview_fps() -> u32 {
        15
    }
    pub fn keybindings() -> String {
        "default".into()
    }

    // Effect engine
    pub fn auto_string() -> String {
        "auto".into()
    }
    pub fn compositor_acceleration_mode() -> RenderAccelerationMode {
        RenderAccelerationMode::Cpu
    }

    // Shared
    pub fn bool_true() -> bool {
        true
    }
    pub fn bool_false() -> bool {
        false
    }
}

// ─── Top-Level Config ────────────────────────────────────────────────────────

/// Root configuration loaded from `hypercolor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypercolorConfig {
    /// Schema version for migration tracking.
    pub schema_version: u32,

    /// Additional TOML files to merge (relative paths).
    #[serde(default)]
    pub include: Vec<String>,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub web: WebConfig,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub effect_engine: EffectEngineConfig,

    #[serde(default)]
    pub audio: AudioConfig,

    #[serde(default)]
    pub capture: CaptureConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    #[serde(default)]
    pub network: NetworkConfig,

    #[serde(default = "default_driver_configs")]
    pub drivers: DriverConfigs,

    #[serde(default)]
    pub dbus: DbusConfig,

    #[serde(default)]
    pub tui: TuiConfig,

    #[serde(default)]
    pub session: SessionConfig,

    #[serde(default)]
    pub features: FeatureFlags,
}

/// Current schema version for newly created configurations.
pub const CURRENT_SCHEMA_VERSION: u32 = 4;

impl Default for HypercolorConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            include: Vec::new(),
            daemon: DaemonConfig::default(),
            web: WebConfig::default(),
            mcp: McpConfig::default(),
            effect_engine: EffectEngineConfig::default(),
            audio: AudioConfig::default(),
            capture: CaptureConfig::default(),
            discovery: DiscoveryConfig::default(),
            network: NetworkConfig::default(),
            drivers: default_driver_configs(),
            dbus: DbusConfig::default(),
            tui: TuiConfig::default(),
            session: SessionConfig::default(),
            features: FeatureFlags::default(),
        }
    }
}

// ─── Driver Registry ────────────────────────────────────────────────────────

/// Stable config map for all driver-owned settings.
pub type DriverConfigs = BTreeMap<String, DriverConfigEntry>;

/// Host-owned wrapper around one driver's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverConfigEntry {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(flatten)]
    pub settings: BTreeMap<String, serde_json::Value>,
}

impl DriverConfigEntry {
    #[must_use]
    pub fn enabled(settings: BTreeMap<String, serde_json::Value>) -> Self {
        Self {
            enabled: true,
            settings,
        }
    }

    #[must_use]
    pub fn disabled(settings: BTreeMap<String, serde_json::Value>) -> Self {
        Self {
            enabled: false,
            settings,
        }
    }
}

impl Default for DriverConfigEntry {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            settings: BTreeMap::new(),
        }
    }
}

#[must_use]
pub fn default_driver_configs() -> DriverConfigs {
    let mut drivers = DriverConfigs::new();
    drivers.insert(
        "wled".to_owned(),
        DriverConfigEntry::enabled(BTreeMap::new()),
    );
    drivers.insert(
        "hue".to_owned(),
        DriverConfigEntry::enabled(BTreeMap::new()),
    );
    drivers.insert(
        "nanoleaf".to_owned(),
        DriverConfigEntry::enabled(BTreeMap::new()),
    );
    drivers.insert(
        "govee".to_owned(),
        DriverConfigEntry::enabled(BTreeMap::new()),
    );
    drivers
}

// ─── Daemon ──────────────────────────────────────────────────────────────────

/// Core daemon settings: networking, render loop, lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "defaults::listen_address")]
    pub listen_address: String,

    #[serde(default = "defaults::port")]
    pub port: u16,

    #[serde(default = "defaults::bool_true")]
    pub unix_socket: bool,

    #[serde(default = "defaults::target_fps")]
    pub target_fps: u32,

    #[serde(default = "defaults::canvas_width")]
    pub canvas_width: u32,

    #[serde(default = "defaults::canvas_height")]
    pub canvas_height: u32,

    #[serde(default = "defaults::max_devices")]
    pub max_devices: u32,

    #[serde(default = "defaults::log_level")]
    pub log_level: LogLevel,

    #[serde(default)]
    pub log_file: String,

    #[serde(default = "defaults::start_profile")]
    pub start_profile: String,

    #[serde(default = "defaults::shutdown_behavior")]
    pub shutdown_behavior: ShutdownBehavior,

    #[serde(default = "defaults::shutdown_color")]
    pub shutdown_color: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            listen_address: defaults::listen_address(),
            port: defaults::port(),
            unix_socket: defaults::bool_true(),
            target_fps: defaults::target_fps(),
            canvas_width: defaults::canvas_width(),
            canvas_height: defaults::canvas_height(),
            max_devices: defaults::max_devices(),
            log_level: defaults::log_level(),
            log_file: String::new(),
            start_profile: defaults::start_profile(),
            shutdown_behavior: defaults::shutdown_behavior(),
            shutdown_color: defaults::shutdown_color(),
        }
    }
}

/// Log verbosity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// What happens to LEDs when the daemon shuts down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownBehavior {
    /// Let hardware controllers decide (most just hold last frame).
    HardwareDefault,
    /// Turn all LEDs off.
    Off,
    /// Set a static color (see `DaemonConfig::shutdown_color`).
    Static,
}

// ─── Web UI ──────────────────────────────────────────────────────────────────

/// Embedded web UI and WebSocket preview server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default)]
    pub open_browser: bool,

    #[serde(default)]
    pub cors_origins: Vec<String>,

    #[serde(default = "defaults::websocket_fps")]
    pub websocket_fps: u32,

    #[serde(default)]
    pub auth_enabled: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            open_browser: false,
            cors_origins: Vec::new(),
            websocket_fps: defaults::websocket_fps(),
            auth_enabled: false,
        }
    }
}

// ─── MCP ─────────────────────────────────────────────────────────────────────

/// Model Context Protocol server settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default = "defaults::bool_false")]
    pub enabled: bool,

    #[serde(default = "defaults::mcp_base_path")]
    pub base_path: String,

    #[serde(default = "defaults::bool_true")]
    pub stateful_mode: bool,

    #[serde(default = "defaults::bool_false")]
    pub json_response: bool,

    #[serde(default = "defaults::sse_keep_alive_secs")]
    pub sse_keep_alive_secs: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_false(),
            base_path: defaults::mcp_base_path(),
            stateful_mode: defaults::bool_true(),
            json_response: defaults::bool_false(),
            sse_keep_alive_secs: defaults::sse_keep_alive_secs(),
        }
    }
}

// ─── Effect Engine ───────────────────────────────────────────────────────────

/// Renderer selection, hot-reload, and effect directory config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectEngineConfig {
    #[serde(default = "defaults::auto_string")]
    pub preferred_renderer: String,

    #[serde(default = "defaults::bool_true")]
    pub servo_enabled: bool,

    #[serde(default = "defaults::auto_string")]
    pub wgpu_backend: String,

    #[serde(
        default = "defaults::compositor_acceleration_mode",
        alias = "render_acceleration_mode"
    )]
    pub compositor_acceleration_mode: RenderAccelerationMode,

    #[serde(default)]
    pub effect_error_fallback: EffectErrorFallbackPolicy,

    #[serde(default)]
    pub extra_effect_dirs: Vec<PathBuf>,

    #[serde(default = "defaults::bool_true")]
    pub watch_effects: bool,

    #[serde(default = "defaults::bool_true")]
    pub watch_config: bool,
}

impl Default for EffectEngineConfig {
    fn default() -> Self {
        Self {
            preferred_renderer: defaults::auto_string(),
            servo_enabled: defaults::bool_true(),
            wgpu_backend: defaults::auto_string(),
            compositor_acceleration_mode: defaults::compositor_acceleration_mode(),
            effect_error_fallback: EffectErrorFallbackPolicy::default(),
            extra_effect_dirs: Vec::new(),
            watch_effects: defaults::bool_true(),
            watch_config: defaults::bool_true(),
        }
    }
}

/// Preferred scene compositor acceleration path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderAccelerationMode {
    /// Always use the CPU path.
    Cpu,
    /// Prefer GPU acceleration when available, otherwise fall back safely.
    Auto,
    /// Require the GPU acceleration lane.
    Gpu,
}

/// Daemon response when a live effect emits an
/// [`crate::event::HypercolorEvent::EffectError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectErrorFallbackPolicy {
    /// Leave the failing assignment in place and surface the error only.
    #[default]
    None,
    /// Clear every active render-group assignment using the failing effect.
    ClearGroups,
}

impl EffectErrorFallbackPolicy {
    #[must_use]
    pub const fn event_label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ClearGroups => Some("clear_groups"),
        }
    }
}

// ─── Audio ───────────────────────────────────────────────────────────────────

/// Audio capture and FFT analysis settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::audio_device")]
    pub device: String,

    #[serde(default = "defaults::fft_size")]
    pub fft_size: u32,

    #[serde(default = "defaults::smoothing")]
    pub smoothing: f32,

    #[serde(default = "defaults::noise_gate")]
    pub noise_gate: f32,

    #[serde(default = "defaults::beat_sensitivity")]
    pub beat_sensitivity: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            device: defaults::audio_device(),
            fft_size: defaults::fft_size(),
            smoothing: defaults::smoothing(),
            noise_gate: defaults::noise_gate(),
            beat_sensitivity: defaults::beat_sensitivity(),
        }
    }
}

// ─── Screen Capture ──────────────────────────────────────────────────────────

/// Screen capture settings for ambient lighting effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "defaults::capture_source")]
    pub source: String,

    #[serde(default = "defaults::capture_fps")]
    pub capture_fps: u32,

    #[serde(default)]
    pub monitor: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: defaults::capture_source(),
            capture_fps: defaults::capture_fps(),
            monitor: 0,
        }
    }
}

// ─── Discovery ───────────────────────────────────────────────────────────────

/// Network device discovery: mDNS, WLED, Hue, and blocksd.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct DiscoveryConfig {
    #[serde(default = "defaults::bool_true")]
    pub mdns_enabled: bool,

    #[serde(default = "defaults::scan_interval")]
    pub scan_interval_secs: u64,

    /// Enable ROLI Blocks discovery via blocksd bridge.
    #[serde(default = "defaults::bool_true")]
    pub blocks_scan: bool,

    /// Custom socket path for blocksd (empty = auto-detect).
    #[serde(default)]
    pub blocks_socket_path: Option<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mdns_enabled: defaults::bool_true(),
            scan_interval_secs: defaults::scan_interval(),
            blocks_scan: defaults::bool_true(),
            blocks_socket_path: None,
        }
    }
}

// ─── Network ────────────────────────────────────────────────────────────────

/// Network discovery and remote access settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "defaults::bool_true")]
    pub mdns_publish: bool,

    #[serde(default = "defaults::remote_access")]
    pub remote_access: bool,

    #[serde(default)]
    pub instance_name: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mdns_publish: defaults::bool_true(),
            remote_access: defaults::remote_access(),
            instance_name: None,
        }
    }
}

// ─── Govee ──────────────────────────────────────────────────────────────────

/// Global Govee backend settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoveeConfig {
    /// IPs that are always probed during Govee LAN discovery.
    #[serde(default)]
    pub known_ips: Vec<IpAddr>,

    /// Device-level power-off on backend disconnect.
    #[serde(default)]
    pub power_off_on_disconnect: bool,

    /// Maximum whole-device LAN state command rate.
    #[serde(default = "defaults::govee_lan_state_fps")]
    pub lan_state_fps: u32,

    /// Maximum validated Razer/Desktop streaming frame rate.
    #[serde(default = "defaults::govee_razer_fps")]
    pub razer_fps: u32,
}

impl Default for GoveeConfig {
    fn default() -> Self {
        Self {
            known_ips: Vec::new(),
            power_off_on_disconnect: false,
            lan_state_fps: defaults::govee_lan_state_fps(),
            razer_fps: defaults::govee_razer_fps(),
        }
    }
}

// ─── D-Bus ───────────────────────────────────────────────────────────────────

/// D-Bus integration settings (Linux desktop integration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbusConfig {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(default = "defaults::bus_name")]
    pub bus_name: String,
}

impl Default for DbusConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            bus_name: defaults::bus_name(),
        }
    }
}

// ─── TUI ─────────────────────────────────────────────────────────────────────

/// Terminal UI preferences: theme, frame rate, keybindings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "defaults::tui_theme")]
    pub theme: String,

    #[serde(default = "defaults::preview_fps")]
    pub preview_fps: u32,

    #[serde(default = "defaults::keybindings")]
    pub keybindings: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: defaults::tui_theme(),
            preview_fps: defaults::preview_fps(),
            keybindings: defaults::keybindings(),
        }
    }
}

// ─── Feature Flags ───────────────────────────────────────────────────────────

/// Opt-in experimental features (all default to `false`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureFlags {
    #[serde(default)]
    pub wasm_plugins: bool,

    #[serde(default)]
    pub hue_entertainment: bool,

    #[serde(default)]
    pub midi_input: bool,
}
