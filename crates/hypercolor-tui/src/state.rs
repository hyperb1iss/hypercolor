//! TUI-side state types — lightweight projections of daemon data.

use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::screen::ScreenId;

// ── Connection ──────────────────────────────────────────────────────

/// Connection status with the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

// ── App State ───────────────────────────────────────────────────────

/// Top-level shared state accessible by all components.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub show_donate: bool,
    pub daemon: Option<DaemonState>,
    pub effects: Vec<EffectSummary>,
    pub devices: Vec<DeviceSummary>,
    pub favorites: Vec<String>,
    pub spectrum: Option<Arc<SpectrumSnapshot>>,
    pub active_screen: ScreenId,
    pub connection_status: ConnectionStatus,
    pub disconnect_reason: Option<String>,
}

// ── Daemon State ────────────────────────────────────────────────────

/// Snapshot of the daemon's overall state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub running: bool,
    pub brightness: u8,
    pub fps_target: f32,
    pub fps_actual: f32,
    pub effect_name: Option<String>,
    pub effect_id: Option<String>,
    pub profile_name: Option<String>,
    pub device_count: u32,
    pub total_leds: u32,
}

// ── Effects ─────────────────────────────────────────────────────────

/// Summary of an available effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub audio_reactive: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub presets: Vec<PresetTemplate>,
}

/// Definition of a user-adjustable control parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlDefinition {
    pub id: String,
    pub name: String,
    pub control_type: String,
    pub default_value: ControlValue,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub group: Option<String>,
    pub tooltip: Option<String>,
}

/// An effect-defined preset snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetTemplate {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub controls: std::collections::HashMap<String, ControlValue>,
}

/// A control parameter value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Float(f32),
    Integer(i32),
    Boolean(bool),
    Color([f32; 4]),
    Text(String),
}

impl ControlValue {
    /// Extract as f32, if numeric.
    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float(v) => Some(*v),
            #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
            Self::Integer(v) => Some(*v as f32),
            _ => None,
        }
    }

    /// Extract as bool, if boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(v) => Some(*v),
            _ => None,
        }
    }
}

// ── Devices ─────────────────────────────────────────────────────────

/// Summary of a connected device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub family: String,
    pub led_count: u32,
    pub state: String,
    pub fps: Option<f32>,
}

/// Summary of a daemon-managed virtual display simulator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimulatedDisplaySummary {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub circular: bool,
    #[serde(default = "simulator_enabled_default")]
    pub enabled: bool,
}

const fn simulator_enabled_default() -> bool {
    true
}

/// Selected source for the TUI preview surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    #[default]
    Canvas,
    Simulator(String),
}

impl PreviewSource {
    /// Return the selected simulator id, when the preview is simulator-backed.
    #[must_use]
    pub fn simulator_id(&self) -> Option<&str> {
        match self {
            Self::Canvas => None,
            Self::Simulator(id) => Some(id.as_str()),
        }
    }
}

// ── Canvas & Audio ──────────────────────────────────────────────────

/// A decoded canvas frame from the WebSocket binary stream.
#[derive(Debug, Clone)]
pub struct CanvasFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u16,
    pub height: u16,
    /// RGB pixel data, 3 bytes per pixel, row-major.
    pub pixels: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasPreviewState {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u16,
    pub height: u16,
}

impl From<&CanvasFrame> for CanvasPreviewState {
    fn from(frame: &CanvasFrame) -> Self {
        Self {
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
        }
    }
}

/// A decoded audio spectrum snapshot from the WebSocket binary stream.
#[derive(Debug, Clone)]
pub struct SpectrumSnapshot {
    pub timestamp_ms: u32,
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat: bool,
    pub beat_confidence: f32,
    pub bpm: Option<f32>,
    pub bins: Vec<f32>,
}

// ── Notifications ───────────────────────────────────────────────────

/// Notification severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A transient notification message.
#[derive(Debug, Clone)]
pub struct Notification {
    pub message: String,
    pub level: NotificationLevel,
}
