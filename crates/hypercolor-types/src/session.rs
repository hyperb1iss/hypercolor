//! Session and power-awareness types shared across Hypercolor crates.
//!
//! This module defines the event vocabulary, configuration schema, and
//! policy action enums used by the core session watcher and daemon power
//! management controller.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Desktop or hardware session state changes observed by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The user's session was locked.
    ScreenLocked,
    /// The user's session was unlocked.
    ScreenUnlocked,
    /// The system is about to suspend.
    Suspending,
    /// The system resumed from suspend.
    Resumed,
    /// The user has been idle for at least the configured threshold.
    IdleEntered { idle_duration: Duration },
    /// The user became active again after an idle period.
    IdleExited,
    /// Laptop lid closed.
    LidClosed,
    /// Laptop lid opened.
    LidOpened,
}

/// Session-awareness configuration loaded from `[session]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub enabled: bool,
    pub idle_enabled: bool,
    pub idle_backend: IdleBackend,
    pub idle_dim_timeout_secs: u64,
    pub idle_off_timeout_secs: u64,
    pub on_screen_lock: SleepBehavior,
    pub screen_lock_brightness: f32,
    pub screen_lock_scene: String,
    pub screen_lock_fade_ms: u64,
    pub screen_unlock_fade_ms: u64,
    pub on_suspend: SleepBehavior,
    pub suspend_fade_ms: u64,
    pub resume_fade_ms: u64,
    pub on_lid_close: SleepBehavior,
    pub lid_close_brightness: f32,
    pub lid_close_scene: String,
    pub lid_close_fade_ms: u64,
    pub lid_open_fade_ms: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_enabled: true,
            idle_backend: IdleBackend::Auto,
            idle_dim_timeout_secs: 120,
            idle_off_timeout_secs: 600,
            on_screen_lock: SleepBehavior::Off,
            screen_lock_brightness: 0.0,
            screen_lock_scene: String::new(),
            screen_lock_fade_ms: 2_000,
            screen_unlock_fade_ms: 500,
            on_suspend: SleepBehavior::Off,
            suspend_fade_ms: 300,
            resume_fade_ms: 150,
            on_lid_close: SleepBehavior::Off,
            lid_close_brightness: 0.0,
            lid_close_scene: String::new(),
            lid_close_fade_ms: 500,
            lid_open_fade_ms: 300,
        }
    }
}

/// High-level behavior mapping for lock, suspend, and lid events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SleepBehavior {
    #[default]
    Off,
    Dim,
    Scene,
    Ignore,
}

/// Preferred idle-detection backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleBackend {
    #[default]
    Auto,
    Wayland,
    X11,
    Dbus,
    Disabled,
}

/// Action to apply when a sleep-related session event fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SleepAction {
    /// Ignore the event.
    Ignore,
    /// Dim output to a target multiplier.
    Dim { brightness: f32, fade_ms: u64 },
    /// Fade to black and stop sending output frames.
    Off { fade_ms: u64 },
    /// Activate a named scene instead of dimming output directly.
    Scene { scene_name: String, fade_ms: u64 },
}

/// Action to apply when the user returns from a sleep-related state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WakeAction {
    /// Restore the previously active output state.
    Restore { fade_ms: u64 },
    /// Activate a named scene on wake.
    Scene { scene_name: String, fade_ms: u64 },
}
