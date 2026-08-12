//! Configuration types -- daemon, audio, web, TUI, discovery, and feature flag settings.
//!
//! All config structs derive `Serialize`/`Deserialize` with `#[serde(default)]` on
//! every optional section for forward/backward compatibility. A fresh install with
//! zero config files boots the daemon entirely from compile-time defaults.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::session::SessionConfig;

// ─── Default Value Functions ─────────────────────────────────────────────────
// Referenced by `#[serde(default = "defaults::...")]` throughout this module.

mod defaults {
    use super::InteractionRoutePolicy;
    use super::LogLevel;
    use super::RenderAccelerationMode;
    use super::ServoGpuImportMode;
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
    pub const fn interactive_preview_resource_bytes() -> u64 {
        1024 * 1024 * 1024
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
    /// Whether screen capture is allowed without the user opting in.
    ///
    /// Windows Desktop Duplication needs no permission grant, shows no
    /// source picker, and draws no capture indicator, so there is nothing
    /// for a user to consent to and an ambient effect can simply work. The
    /// XDG portal on Linux opens a picker the user must answer, and macOS
    /// gates capture behind a TCC prompt; forcing either at daemon start
    /// would be an ambush, so those stay opt-in.
    ///
    /// Enabling this only grants permission. Capture opens on demand and
    /// stays closed until a screen-reactive effect actually asks for it.
    pub fn capture_enabled() -> bool {
        cfg!(target_os = "windows")
    }
    pub fn capture_fps() -> u32 {
        30
    }
    pub fn capture_grid_cols() -> u32 {
        8
    }
    pub fn capture_grid_rows() -> u32 {
        6
    }
    pub fn capture_smoothing() -> f32 {
        0.3
    }
    pub fn capture_scene_cut_threshold() -> f32 {
        100.0
    }
    pub fn capture_letterbox_threshold() -> f32 {
        0.02
    }
    pub fn capture_target_led_white_x() -> f32 {
        0.3127
    }
    pub fn capture_target_led_white_y() -> f32 {
        0.3290
    }
    pub fn capture_target_led_reference_white_nits() -> f32 {
        203.0
    }
    pub fn capture_target_led_peak_nits() -> f32 {
        406.0
    }
    pub fn capture_exposure_ev() -> f32 {
        0.0
    }
    pub fn unit_scale() -> f32 {
        1.0
    }

    // Display
    pub fn face_fps_cap() -> u32 {
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
    pub fn network_access_mode() -> super::NetworkAccessMode {
        super::NetworkAccessMode::LocalOnly
    }
    pub fn network_client_scope() -> super::NetworkClientScope {
        super::NetworkClientScope::LocalSubnets
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
        RenderAccelerationMode::Auto
    }
    pub const fn servo_gpu_import_mode() -> ServoGpuImportMode {
        ServoGpuImportMode::Auto
    }
    pub const fn max_video_producers() -> u8 {
        2
    }
    pub const fn max_livestream_producers() -> u8 {
        1
    }

    // Shared
    pub fn bool_true() -> bool {
        true
    }
    pub fn bool_false() -> bool {
        false
    }
    pub const fn daemon_interaction_route() -> InteractionRoutePolicy {
        InteractionRoutePolicy::Host
    }
    pub const fn preview_interaction_route() -> InteractionRoutePolicy {
        InteractionRoutePolicy::Browser
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
    pub rendering: RenderingConfig,

    #[serde(default)]
    pub media: MediaConfig,

    #[serde(default)]
    pub audio: AudioConfig,

    #[serde(default)]
    pub capture: CaptureConfig,

    #[serde(default)]
    pub input: InputConfig,

    #[serde(default)]
    pub display: DisplayConfig,

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

    /// Top-level sections this build does not model, preserved verbatim.
    ///
    /// The daemon persists config as a whole-file rewrite, and extension
    /// crates (the official cloud daemon's `[cloud]` section, for one)
    /// share this file. Without a catch-all, every save silently deletes
    /// their configuration.
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
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
            rendering: RenderingConfig::default(),
            media: MediaConfig::default(),
            audio: AudioConfig::default(),
            capture: CaptureConfig::default(),
            input: InputConfig::default(),
            display: DisplayConfig::default(),
            discovery: DiscoveryConfig::default(),
            network: NetworkConfig::default(),
            drivers: default_driver_configs(),
            dbus: DbusConfig::default(),
            tui: TuiConfig::default(),
            session: SessionConfig::default(),
            features: FeatureFlags::default(),
            extensions: BTreeMap::new(),
        }
    }
}

// ─── Driver Registry ────────────────────────────────────────────────────────

/// Stable config map for all driver-owned settings.
pub type DriverConfigs = BTreeMap<String, DriverConfigEntry>;

/// Host-owned wrapper around one driver's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
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
    DriverConfigs::new()
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

    #[serde(default = "defaults::interactive_preview_resource_bytes")]
    pub interactive_preview_resource_bytes: u64,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::bool_true(),
            open_browser: false,
            cors_origins: Vec::new(),
            websocket_fps: defaults::websocket_fps(),
            interactive_preview_resource_bytes: defaults::interactive_preview_resource_bytes(),
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

// ─── Rendering ───────────────────────────────────────────────────────────────

/// Rendering-path feature switches and import policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingConfig {
    pub servo_gpu_import: ServoGpuImportConfig,
}

/// Linux Servo GPU import policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServoGpuImportConfig {
    #[serde(default = "defaults::servo_gpu_import_mode")]
    pub mode: ServoGpuImportMode,
}

// ─── Media ──────────────────────────────────────────────────────────────────

/// User media decoder policy and resource caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaConfig {
    #[serde(default = "defaults::max_video_producers")]
    pub max_video_producers: u8,

    #[serde(default = "defaults::max_livestream_producers")]
    pub max_livestream_producers: u8,

    #[serde(default)]
    pub stream_private_network_allowlist: Vec<String>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_video_producers: defaults::max_video_producers(),
            max_livestream_producers: defaults::max_livestream_producers(),
            stream_private_network_allowlist: Vec::new(),
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

/// Servo framebuffer import policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServoGpuImportMode {
    /// Never attempt Servo GPU framebuffer import.
    Off,
    /// Attempt import when startup capabilities indicate it can work.
    #[default]
    Auto,
    /// Require import and report frame errors instead of silent CPU fallback.
    On,
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
///
/// The capture source is chosen interactively through the desktop portal
/// picker; `restore_token` persists that choice across daemon restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    #[serde(default = "defaults::capture_enabled")]
    pub enabled: bool,

    #[serde(default = "defaults::capture_source")]
    pub source: String,

    #[serde(default = "defaults::capture_fps")]
    pub capture_fps: u32,

    /// Sector grid columns for ambilight zone sampling.
    #[serde(default = "defaults::capture_grid_cols")]
    pub grid_cols: u32,

    /// Sector grid rows for ambilight zone sampling.
    #[serde(default = "defaults::capture_grid_rows")]
    pub grid_rows: u32,

    /// Process-memory byte budget shared by analysis and screen publications.
    ///
    /// When omitted, the daemon snapshots currently available host memory
    /// during startup. The analyzer reserves its peak first and publication
    /// plans consume the remainder. Dimensions remain unconstrained; checked
    /// memory and compute admission determine whether a configuration fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_memory_bytes: Option<u64>,

    /// Temporal smoothing factor (0.0 = frozen, 1.0 = raw).
    #[serde(default = "defaults::capture_smoothing")]
    pub smoothing: f32,

    /// Frame-difference threshold that bypasses smoothing on scene cuts.
    #[serde(default = "defaults::capture_scene_cut_threshold")]
    pub scene_cut_threshold: f32,

    /// Auto-detect and crop black letterbox/pillarbox bars.
    ///
    /// Off by default: ambient lighting almost always mirrors a desktop, not
    /// a letterboxed film, and dark desktop content trips the detector into
    /// cropping real picture away. Turn it on when mirroring video that
    /// genuinely has bars.
    #[serde(default)]
    pub letterbox: bool,

    /// Luminance threshold for letterbox detection (0.0 - 1.0).
    #[serde(default = "defaults::capture_letterbox_threshold")]
    pub letterbox_threshold: f32,

    /// Saturation boost applied to zone colors (1.0 = neutral).
    #[serde(default = "defaults::unit_scale")]
    pub saturation: f32,

    /// Brightness multiplier applied to zone colors (1.0 = neutral).
    #[serde(default = "defaults::unit_scale")]
    pub brightness: f32,

    /// Gamma shaping applied to zone colors (1.0 = neutral, >1 darkens mids).
    #[serde(default = "defaults::unit_scale")]
    pub gamma: f32,

    /// Target LED white-point x coordinate in CIE xy chromaticity space.
    #[serde(default = "defaults::capture_target_led_white_x")]
    pub target_led_white_x: f32,

    /// Target LED white-point y coordinate in CIE xy chromaticity space.
    #[serde(default = "defaults::capture_target_led_white_y")]
    pub target_led_white_y: f32,

    /// Target LED reference white in nits for HDR tone mapping.
    #[serde(default = "defaults::capture_target_led_reference_white_nits")]
    pub target_led_reference_white_nits: f32,

    /// Calibrated target LED peak in nits for HDR tone mapping.
    #[serde(default = "defaults::capture_target_led_peak_nits")]
    pub target_led_peak_nits: f32,

    /// User exposure adjustment in exposure-value stops.
    #[serde(default = "defaults::capture_exposure_ev")]
    pub exposure_ev: f32,

    /// XDG portal restore token so the picked source survives restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_token: Option<String>,
}

/// Native capture implementation selected by the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePlatform {
    /// DXGI Desktop Duplication.
    WindowsDesktopDuplication,
    /// XDG desktop portal plus PipeWire.
    LinuxPipeWire,
    /// ScreenCaptureKit with the system content picker.
    MacosScreenCaptureKit,
    /// No native screen-capture implementation is available.
    Unsupported,
}

impl CapturePlatform {
    /// Capture platform compiled into this target.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::WindowsDesktopDuplication
        }
        #[cfg(target_os = "linux")]
        {
            Self::LinuxPipeWire
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacosScreenCaptureKit
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            Self::Unsupported
        }
    }
}

/// Invalid screen-capture configuration rejected before persistence or startup.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CaptureConfigValidationError {
    /// The target has no native capture backend.
    #[error("screen capture is not supported on this platform")]
    UnsupportedPlatform,
    /// Capture cadence must be non-zero.
    #[error("capture.capture_fps must be non-zero, got {value}")]
    CaptureFps {
        /// Rejected value.
        value: u32,
    },
    /// A grid dimension is empty.
    #[error("capture.{field} must be non-zero, got {value}")]
    GridDimension {
        /// Config field name.
        field: &'static str,
        /// Rejected value.
        value: u32,
    },
    /// An explicit publication-memory budget is empty.
    #[error("capture.publication_memory_bytes must be non-zero, got {value}")]
    PublicationMemoryBudget {
        /// Rejected byte budget.
        value: u64,
    },
    /// A floating-point setting is non-finite or outside its semantic range.
    #[error("capture.{field} must be finite and in {min}..={max}, got {value}")]
    FloatRange {
        /// Config field name.
        field: &'static str,
        /// Inclusive lower bound.
        min: f32,
        /// Inclusive upper bound.
        max: f32,
        /// Rejected value.
        value: f32,
    },
    /// The target LED white point lies outside the CIE xy triangle.
    #[error(
        "capture target LED white point must be finite with x > 0, y > 0, and x + y < 1, got ({x}, {y})"
    )]
    WhitePointChromaticity {
        /// Rejected CIE xy x coordinate.
        x: f32,
        /// Rejected CIE xy y coordinate.
        y: f32,
    },
    /// Target peak does not leave any headroom above reference white.
    #[error(
        "capture.target_led_peak_nits must be greater than target_led_reference_white_nits ({reference}), got {peak}"
    )]
    PeakNotAboveReference {
        /// Configured target reference white in nits.
        reference: f32,
        /// Rejected target peak in nits.
        peak: f32,
    },
    /// The selected source cannot be represented by the native backend.
    #[error("capture.source is invalid for {platform}: {reason}")]
    Source {
        /// Backend accepting the source string.
        platform: &'static str,
        /// Specific validation failure.
        reason: &'static str,
    },
}

impl CaptureConfig {
    /// Validate every capture setting against the backend compiled for this target.
    ///
    /// # Errors
    ///
    /// Returns the first unsupported or out-of-range setting.
    pub fn validate(&self) -> Result<(), CaptureConfigValidationError> {
        self.validate_for_platform(CapturePlatform::current())
    }

    /// Validate against an explicit backend for cross-platform contract tests.
    ///
    /// # Errors
    ///
    /// Returns the first unsupported or out-of-range setting.
    pub fn validate_for_platform(
        &self,
        platform: CapturePlatform,
    ) -> Result<(), CaptureConfigValidationError> {
        if self.capture_fps == 0 {
            return Err(CaptureConfigValidationError::CaptureFps {
                value: self.capture_fps,
            });
        }
        validate_grid_dimension("grid_cols", self.grid_cols)?;
        validate_grid_dimension("grid_rows", self.grid_rows)?;
        if self.publication_memory_bytes == Some(0) {
            return Err(CaptureConfigValidationError::PublicationMemoryBudget { value: 0 });
        }
        validate_capture_float("smoothing", self.smoothing, 0.0, 1.0)?;
        validate_capture_float("scene_cut_threshold", self.scene_cut_threshold, 0.0, 765.0)?;
        validate_capture_float("letterbox_threshold", self.letterbox_threshold, 0.0, 1.0)?;
        validate_capture_float("saturation", self.saturation, 0.0, 4.0)?;
        validate_capture_float("brightness", self.brightness, 0.0, 4.0)?;
        validate_capture_float("gamma", self.gamma, 0.2, 5.0)?;
        if !self.target_led_white_x.is_finite()
            || !self.target_led_white_y.is_finite()
            || self.target_led_white_x <= 0.0
            || self.target_led_white_y <= 0.0
            || self.target_led_white_x + self.target_led_white_y >= 1.0
        {
            return Err(CaptureConfigValidationError::WhitePointChromaticity {
                x: self.target_led_white_x,
                y: self.target_led_white_y,
            });
        }
        validate_capture_float(
            "target_led_reference_white_nits",
            self.target_led_reference_white_nits,
            1.0,
            5_000.0,
        )?;
        validate_capture_float(
            "target_led_peak_nits",
            self.target_led_peak_nits,
            1.0,
            10_000.0,
        )?;
        if self.target_led_peak_nits <= self.target_led_reference_white_nits {
            return Err(CaptureConfigValidationError::PeakNotAboveReference {
                reference: self.target_led_reference_white_nits,
                peak: self.target_led_peak_nits,
            });
        }
        validate_capture_float("exposure_ev", self.exposure_ev, -8.0, 8.0)?;
        validate_capture_source(platform, &self.source, self.enabled)?;
        if matches!(platform, CapturePlatform::Unsupported) && self.enabled {
            return Err(CaptureConfigValidationError::UnsupportedPlatform);
        }
        Ok(())
    }
}

fn validate_grid_dimension(
    field: &'static str,
    value: u32,
) -> Result<(), CaptureConfigValidationError> {
    if value != 0 {
        Ok(())
    } else {
        Err(CaptureConfigValidationError::GridDimension { field, value })
    }
}

fn validate_capture_float(
    field: &'static str,
    value: f32,
    min: f32,
    max: f32,
) -> Result<(), CaptureConfigValidationError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(CaptureConfigValidationError::FloatRange {
            field,
            min,
            max,
            value,
        })
    }
}

fn validate_capture_source(
    platform: CapturePlatform,
    source: &str,
    enabled: bool,
) -> Result<(), CaptureConfigValidationError> {
    let source = source.trim();
    let platform_name = match platform {
        CapturePlatform::WindowsDesktopDuplication => "Windows Desktop Duplication",
        CapturePlatform::LinuxPipeWire => "Linux PipeWire",
        CapturePlatform::MacosScreenCaptureKit => "macOS ScreenCaptureKit",
        CapturePlatform::Unsupported => "this platform",
    };
    if source.is_empty() {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source must not be empty",
        });
    }
    if source.len() > 1024 {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source exceeds 1024 bytes",
        });
    }
    if source.chars().any(char::is_control) {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source contains control characters",
        });
    }
    if enabled
        && matches!(platform, CapturePlatform::LinuxPipeWire)
        && !source.eq_ignore_ascii_case("auto")
    {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "portal-managed capture requires source = \"auto\"",
        });
    }
    if matches!(platform, CapturePlatform::MacosScreenCaptureKit)
        && !is_valid_macos_capture_source(source)
    {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "expected auto, primary_display, session_scoped, or display:<canonical UUID>",
        });
    }
    Ok(())
}

fn is_valid_macos_capture_source(source: &str) -> bool {
    matches!(source, "auto" | "primary_display" | "session_scoped")
        || source.strip_prefix("display:").is_some_and(|value| {
            value.len() == 36
                && Uuid::parse_str(value)
                    .is_ok_and(|uuid| uuid.hyphenated().to_string().eq_ignore_ascii_case(value))
        })
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::capture_enabled(),
            source: defaults::capture_source(),
            capture_fps: defaults::capture_fps(),
            grid_cols: defaults::capture_grid_cols(),
            grid_rows: defaults::capture_grid_rows(),
            publication_memory_bytes: None,
            smoothing: defaults::capture_smoothing(),
            scene_cut_threshold: defaults::capture_scene_cut_threshold(),
            letterbox: false,
            letterbox_threshold: defaults::capture_letterbox_threshold(),
            saturation: defaults::unit_scale(),
            brightness: defaults::unit_scale(),
            gamma: defaults::unit_scale(),
            target_led_white_x: defaults::capture_target_led_white_x(),
            target_led_white_y: defaults::capture_target_led_white_y(),
            target_led_reference_white_nits: defaults::capture_target_led_reference_white_nits(),
            target_led_peak_nits: defaults::capture_target_led_peak_nits(),
            exposure_ev: defaults::capture_exposure_ev(),
            restore_token: None,
        }
    }
}

// ─── Input ───────────────────────────────────────────────────────────────────

/// Host keyboard/mouse capture for interactive effects.
///
/// Capture is consent-gated: `enabled` defaults to `false` and nothing opens
/// an input device until the user turns it on. Even when enabled, backends
/// only capture while an active effect declares input reactivity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Capture host keyboard state and events.
    #[serde(default = "defaults::bool_true")]
    pub keyboard: bool,

    /// Capture host pointer state and events.
    #[serde(default = "defaults::bool_true")]
    pub mouse: bool,

    /// Interaction sources routed into authoritative daemon effects.
    #[serde(default = "defaults::daemon_interaction_route")]
    pub daemon_route: InteractionRoutePolicy,

    /// Interaction sources routed into connection-scoped interactive previews.
    #[serde(default = "defaults::preview_interaction_route")]
    pub preview_route: InteractionRoutePolicy,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            keyboard: defaults::bool_true(),
            mouse: defaults::bool_true(),
            daemon_route: defaults::daemon_interaction_route(),
            preview_route: defaults::preview_interaction_route(),
        }
    }
}

/// Which interaction sources one effect consumer receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRoutePolicy {
    /// Host keyboard and pointer sources only.
    Host,
    /// The consumer's explicitly addressed browser source only.
    Browser,
    /// Host sources plus the consumer's explicitly addressed browser source.
    Merge,
}

// ─── Display ─────────────────────────────────────────────────────────────────

/// Bounds for [`DisplayConfig::face_fps_cap`].
pub const FACE_FPS_CAP_MIN: u32 = 15;
pub const FACE_FPS_CAP_MAX: u32 = 60;

/// Device display (LCD face) output settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Upper bound for HTML face rendering on the group-direct path.
    /// The device transport limit still wins below this cap.
    #[serde(default = "defaults::face_fps_cap")]
    pub face_fps_cap: u32,
}

impl DisplayConfig {
    /// The configured cap clamped to the supported range.
    #[must_use]
    pub fn effective_face_fps_cap(&self) -> u32 {
        self.face_fps_cap.clamp(FACE_FPS_CAP_MIN, FACE_FPS_CAP_MAX)
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            face_fps_cap: defaults::face_fps_cap(),
        }
    }
}

// ─── Discovery ───────────────────────────────────────────────────────────────

/// Network device discovery: mDNS, WLED, Hue, and blocksd.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct DiscoveryConfig {
    /// Run startup, hotplug, and periodic background discovery.
    ///
    /// Manual discovery requests remain available when this is disabled.
    #[serde(default = "defaults::bool_true")]
    pub background_enabled: bool,

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
            background_enabled: defaults::bool_true(),
            mdns_enabled: defaults::bool_true(),
            scan_interval_secs: defaults::scan_interval(),
            blocks_scan: defaults::bool_true(),
            blocks_socket_path: None,
        }
    }
}

// ─── Network ────────────────────────────────────────────────────────────────

/// Coarse-grained daemon API exposure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccessMode {
    LocalOnly,
    LanTrusted,
    LanProtected,
    Custom,
}

impl Default for NetworkAccessMode {
    fn default() -> Self {
        defaults::network_access_mode()
    }
}

/// Built-in client-address scopes for network-reachable API listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkClientScope {
    LocalSubnets,
    PrivateRanges,
    Custom,
}

impl Default for NetworkClientScope {
    fn default() -> Self {
        defaults::network_client_scope()
    }
}

/// Network discovery and remote access settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "defaults::network_access_mode")]
    pub access_mode: NetworkAccessMode,

    #[serde(default = "defaults::network_client_scope")]
    pub client_scope: NetworkClientScope,

    #[serde(default = "defaults::bool_true")]
    pub mdns_publish: bool,

    #[serde(default = "defaults::remote_access")]
    pub remote_access: bool,

    #[serde(default)]
    pub allow_unauthenticated_remote_access: bool,

    #[serde(default)]
    pub allowed_clients: Vec<String>,

    #[serde(default)]
    pub instance_name: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            access_mode: NetworkAccessMode::default(),
            client_scope: NetworkClientScope::default(),
            mdns_publish: defaults::bool_true(),
            remote_access: defaults::remote_access(),
            allow_unauthenticated_remote_access: false,
            allowed_clients: Vec::new(),
            instance_name: None,
        }
    }
}

impl NetworkConfig {
    #[must_use]
    pub const fn remote_access_enabled(&self) -> bool {
        self.remote_access
            || matches!(
                self.access_mode,
                NetworkAccessMode::LanTrusted | NetworkAccessMode::LanProtected
            )
    }

    #[must_use]
    pub const fn unauthenticated_remote_access_allowed(&self) -> bool {
        match self.access_mode {
            NetworkAccessMode::LanTrusted => true,
            NetworkAccessMode::LanProtected => false,
            NetworkAccessMode::LocalOnly | NetworkAccessMode::Custom => {
                self.allow_unauthenticated_remote_access
            }
        }
    }

    #[must_use]
    pub const fn network_bind_requires_auth(&self) -> bool {
        match self.access_mode {
            NetworkAccessMode::LanTrusted => false,
            NetworkAccessMode::LanProtected => true,
            NetworkAccessMode::LocalOnly | NetworkAccessMode::Custom => {
                !self.unauthenticated_remote_access_allowed()
            }
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

#[cfg(test)]
mod extension_section_tests {
    use super::*;

    #[test]
    fn unknown_top_level_sections_survive_a_round_trip() {
        let source = r"
schema_version = 4

[daemon]
port = 9420

[cloud]
enabled = true
connect_on_start = true
";
        let parsed: HypercolorConfig = toml::from_str(source).expect("parses");
        assert_eq!(
            parsed
                .extensions
                .get("cloud")
                .and_then(|section| section.get("enabled"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "the [cloud] section lands in the catch-all"
        );

        let rewritten = toml::to_string_pretty(&parsed).expect("serializes");
        let reparsed: HypercolorConfig = toml::from_str(&rewritten).expect("reparses");
        assert_eq!(
            reparsed.extensions.get("cloud"),
            parsed.extensions.get("cloud"),
            "a persist rewrite must not delete extension config"
        );
    }
}
