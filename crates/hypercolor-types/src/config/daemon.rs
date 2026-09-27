use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Daemon ──────────────────────────────────────────────────────────────────

/// Core daemon settings: networking, render loop, lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
