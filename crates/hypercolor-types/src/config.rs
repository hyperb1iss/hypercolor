//! Configuration types -- daemon, audio, web, TUI, discovery, and feature flag settings.
//!
//! All config structs derive `Serialize`/`Deserialize` with `#[serde(default)]` on
//! every optional section for forward/backward compatibility. A fresh install with
//! zero config files boots the daemon entirely from compile-time defaults.

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
    pub fn wled_dedup_threshold() -> u8 {
        2
    }
    pub fn nanoleaf_transition() -> u16 {
        1
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
    pub fn render_acceleration_mode() -> RenderAccelerationMode {
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

    #[serde(default)]
    pub wled: WledConfig,

    #[serde(default)]
    pub hue: HueConfig,

    #[serde(default)]
    pub nanoleaf: NanoleafConfig,

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
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

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
            wled: WledConfig::default(),
            hue: HueConfig::default(),
            nanoleaf: NanoleafConfig::default(),
            dbus: DbusConfig::default(),
            tui: TuiConfig::default(),
            session: SessionConfig::default(),
            features: FeatureFlags::default(),
        }
    }
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

    #[serde(default = "defaults::render_acceleration_mode")]
    pub render_acceleration_mode: RenderAccelerationMode,

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
            render_acceleration_mode: defaults::render_acceleration_mode(),
            extra_effect_dirs: Vec::new(),
            watch_effects: defaults::bool_true(),
            watch_config: defaults::bool_true(),
        }
    }
}

/// Preferred render-surface acceleration path.
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

    #[serde(default = "defaults::bool_true")]
    pub wled_scan: bool,

    #[serde(default = "defaults::bool_true")]
    pub hue_scan: bool,

    /// Enable Nanoleaf device scanning (mDNS + manual IP probe).
    #[serde(default = "defaults::bool_true")]
    pub nanoleaf_scan: bool,

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
            wled_scan: defaults::bool_true(),
            hue_scan: defaults::bool_true(),
            nanoleaf_scan: defaults::bool_true(),
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

// ─── WLED ───────────────────────────────────────────────────────────────────

/// Default protocol for WLED realtime streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WledProtocolConfig {
    /// Distributed Display Protocol (preferred).
    #[default]
    Ddp,
    /// E1.31 / sACN output.
    E131,
}

/// Global WLED backend settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WledConfig {
    /// IPs that are always probed during WLED discovery.
    #[serde(default)]
    pub known_ips: Vec<IpAddr>,

    /// Default realtime transport for newly connected WLED devices.
    #[serde(default)]
    pub default_protocol: WledProtocolConfig,

    /// Whether startup/shutdown should toggle WLED realtime mode over HTTP.
    #[serde(default = "defaults::bool_true")]
    pub realtime_http_enabled: bool,

    /// Fuzzy frame dedup threshold (0 disables deduplication).
    #[serde(default = "defaults::wled_dedup_threshold")]
    pub dedup_threshold: u8,
}

impl Default for WledConfig {
    fn default() -> Self {
        Self {
            known_ips: Vec::new(),
            default_protocol: WledProtocolConfig::default(),
            realtime_http_enabled: defaults::bool_true(),
            dedup_threshold: defaults::wled_dedup_threshold(),
        }
    }
}

/// Philips Hue backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HueConfig {
    /// Preferred entertainment configuration name or ID.
    #[serde(default)]
    pub entertainment_config: Option<String>,

    /// Manual bridge IPs for networks where mDNS discovery is unavailable.
    #[serde(default)]
    pub bridge_ips: Vec<IpAddr>,

    /// Use CIE xy color conversion when streaming to Hue.
    #[serde(default = "defaults::bool_true")]
    pub use_cie_xy: bool,
}

impl Default for HueConfig {
    fn default() -> Self {
        Self {
            entertainment_config: None,
            bridge_ips: Vec::new(),
            use_cie_xy: defaults::bool_true(),
        }
    }
}

/// Nanoleaf backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NanoleafConfig {
    /// Manual device IPs for networks where mDNS discovery is unavailable.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Transition time per frame in deciseconds (100ms units).
    #[serde(default = "defaults::nanoleaf_transition")]
    pub transition_time: u16,
}

impl Default for NanoleafConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            transition_time: defaults::nanoleaf_transition(),
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
