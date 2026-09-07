//! TUI-side state types — lightweight projections of daemon data.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use hypercolor_types::api::effects::EffectSourceKind;
pub use hypercolor_types::api::scene::{SceneDocument, ZoneResource};
pub use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::EffectCategory;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::ZoneRole;
use serde::{Deserialize, Deserializer, Serialize};

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
    pub scenes: Vec<SceneSummary>,
    pub active_scene: Option<Arc<SceneDocument>>,
    /// Zone targeted by apply/control edits. `None` = the primary zone.
    pub focused_zone: Option<String>,
    pub spectrum: Option<Arc<SpectrumSnapshot>>,
    pub active_screen: ScreenId,
    pub connection_status: ConnectionStatus,
    pub disconnect_reason: Option<String>,
}

impl AppState {
    /// The zone that apply/control edits currently target.
    ///
    /// Falls back to the primary zone when no explicit focus is set or the
    /// focused zone no longer exists in the active scene.
    #[must_use]
    pub fn target_zone(&self) -> Option<&ZoneResource> {
        let scene = self.active_scene.as_deref()?;
        self.focused_zone
            .as_deref()
            .and_then(|id| scene_zone(scene, id))
            .or_else(|| primary_zone(scene))
    }
}

// ── Scenes & Zones ──────────────────────────────────────────────────

/// One saved scene, as listed by `GET /scenes` (shared wire contract).
pub use hypercolor_types::api::scenes::SceneSummary;

/// Whether the live scene contains more than one authored zone.
#[must_use]
pub fn scene_is_multi_zone(scene: &SceneDocument) -> bool {
    scene.zones.len() > 1
}

/// Look up a zone by its canonical wire id.
#[must_use]
pub fn scene_zone<'a>(scene: &'a SceneDocument, id: &str) -> Option<&'a ZoneResource> {
    scene.zones.iter().find(|zone| zone.id.to_string() == id)
}

/// Select the primary zone, falling back to the first authored zone.
#[must_use]
pub fn primary_zone(scene: &SceneDocument) -> Option<&ZoneResource> {
    scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .or_else(|| scene.zones.first())
}

/// Select the topmost effect layer from a canonical zone resource.
#[must_use]
pub fn zone_effect_layer(zone: &ZoneResource) -> Option<&SceneLayer> {
    zone.layers
        .iter()
        .rev()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
}

/// Select the topmost effect id from a canonical zone resource.
#[must_use]
pub fn zone_effect_id(zone: &ZoneResource) -> Option<String> {
    let LayerSource::Effect { effect_id, .. } = &zone_effect_layer(zone)?.source else {
        return None;
    };
    Some(effect_id.to_string())
}

/// Materialize the selected effect layer's controls for TUI widgets.
#[must_use]
pub fn zone_effect_controls(zone: &ZoneResource) -> HashMap<String, ControlValue> {
    let Some(layer) = zone_effect_layer(zone) else {
        return HashMap::new();
    };
    let LayerSource::Effect { controls, .. } = &layer.source else {
        return HashMap::new();
    };
    controls
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Update one selected effect control in a canonical zone resource.
pub fn set_zone_effect_control(zone: &mut ZoneResource, id: &str, value: &ControlValue) {
    let Some(layer) = zone
        .layers
        .iter_mut()
        .rev()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
    else {
        return;
    };
    let LayerSource::Effect { controls, .. } = &mut layer.source else {
        return;
    };
    controls.insert(id.to_owned(), value.clone());
}

// ── Daemon State ────────────────────────────────────────────────────

/// Snapshot of the daemon's overall state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub running: bool,
    pub brightness: u8,
    pub fps_target: f32,
    pub fps_actual: f32,
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
    pub category: EffectCategory,
    pub source: EffectSourceKind,
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
#[derive(Debug, Clone, Serialize)]
pub struct PresetTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub controls: std::collections::HashMap<String, ControlValue>,
}

#[derive(Deserialize)]
struct PresetTemplateWire {
    id: Option<String>,
    name: String,
    description: Option<String>,
    #[serde(default)]
    controls: HashMap<String, ControlValue>,
}

impl<'de> Deserialize<'de> for PresetTemplate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PresetTemplateWire::deserialize(deserializer)?;
        let id = wire
            .id
            .filter(|id| !PresetId::normalize_key(id).is_empty())
            .unwrap_or_else(|| PresetId::stable(&wire.name).to_string());

        Ok(Self {
            id,
            name: wire.name,
            description: wire.description,
            controls: wire.controls,
        })
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
pub type SimulatedDisplaySummary = hypercolor_types::api::simulators::SimulatedDisplay;

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
    pub width: u32,
    pub height: u32,
    /// RGB pixel data, 3 bytes per pixel, row-major.
    pub pixels: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasPreviewState {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
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
