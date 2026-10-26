use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Daemon ──────────────────────────────────────────────────────────────────

/// Default `[daemon] target_fps`, the top render tier.
///
/// The value is a ceiling, not a fixed rate: the render loop's FPS
/// controller never upshifts past it, and it downshifts on its own after
/// consecutive budget misses. A lower default would only strand headroom
/// on machines that can hold the full tier, while slower machines are
/// already protected by the downshift.
pub const DEFAULT_TARGET_FPS: u32 = 60;

/// Core daemon settings: networking, render loop, lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "defaults::listen_address")]
    pub listen_address: String,

    #[serde(default = "defaults::port")]
    pub port: u16,

    #[serde(default = "defaults::bool_true")]
    pub unix_socket: bool,

    /// Highest render tier the FPS controller may run at.
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

    #[serde(default = "defaults::start_scene")]
    pub start_scene: String,

    #[serde(default = "defaults::shutdown_behavior")]
    pub shutdown_behavior: ShutdownBehavior,

    #[serde(default = "defaults::shutdown_color")]
    pub shutdown_color: String,

    /// Previous versions of each durable state file the daemon keeps, from
    /// newest to oldest. `0` keeps none.
    #[serde(default = "defaults::state_history_generations")]
    pub state_history_generations: u32,

    /// Seconds a state file's content must stay current before a later
    /// write keeps it as a previous version. Shorter-lived content is
    /// transient (a slider drag, a burst of automation) and is not kept.
    #[serde(default = "defaults::state_history_min_interval_secs")]
    pub state_history_min_interval_secs: u64,
}

impl DaemonConfig {
    /// The render resolution must describe a non-empty RGBA surface that
    /// fits in addressable memory. It is the only size every LED layout
    /// renders at, so an unusable one is refused before it is saved.
    ///
    /// # Errors
    ///
    /// Returns the surface error for a zero or oversized canvas.
    pub fn validate_canvas(&self) -> Result<(), crate::canvas::SurfaceResourceError> {
        crate::canvas::SurfaceDescriptor::rgba8888(self.canvas_width, self.canvas_height)
            .try_non_empty_byte_len()
            .map(|_| ())
    }
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
            start_scene: defaults::start_scene(),
            shutdown_behavior: defaults::shutdown_behavior(),
            shutdown_color: defaults::shutdown_color(),
            state_history_generations: defaults::state_history_generations(),
            state_history_min_interval_secs: defaults::state_history_min_interval_secs(),
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
