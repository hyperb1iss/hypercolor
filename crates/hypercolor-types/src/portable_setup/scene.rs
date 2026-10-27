use crate::identity::LayoutId;
use crate::layer::{BlendMode, SceneLayer};
use crate::scene::{
    SceneKind, SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, ZoneId,
    ZoneRole,
};
use crate::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Orientation, OutputComponent, SamplingMode,
    ZoneShape,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Authored scene fields, excluding runtime revisions and run epochs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableScene {
    pub name: String,
    pub description: Option<String>,
    pub zones: Vec<PortableZone>,
    pub transition: TransitionSpec,
    pub priority: ScenePriority,
    pub enabled: bool,
    pub metadata: HashMap<String, String>,
    pub unassigned_behavior: UnassignedBehavior,
    pub layout_id: Option<LayoutId>,
    pub activation_brightness: Option<f32>,
    pub kind: SceneKind,
    pub mutation_mode: SceneMutationMode,
}

/// Scene zone with authored layers and a resolution-free spatial layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableZone {
    pub id: ZoneId,
    pub name: String,
    pub description: Option<String>,
    pub layers: Vec<SceneLayer>,
    pub layout: PortableLayout,
    pub brightness: f32,
    pub enabled: bool,
    pub color: Option<String>,
    pub display_target: Option<PortableDisplayTarget>,
    pub role: ZoneRole,
}

/// Display destination references the physical device in the setup manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDisplayTarget {
    pub device: Uuid,
    pub blend_mode: BlendMode,
    pub opacity: f32,
}

/// Normalized spatial authoring, with dimensions supplied by the destination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableLayout {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub zones: Vec<PortableOutput>,
    pub default_sampling_mode: SamplingMode,
    pub default_edge_behavior: EdgeBehavior,
    pub version: u32,
}

/// Spatial output bound to a setup device or logical segment UUID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableOutput {
    pub id: String,
    pub name: String,
    pub device: Uuid,
    pub zone_name: Option<String>,
    pub position: NormalizedPosition,
    pub size: NormalizedPosition,
    pub rotation: f32,
    pub scale: f32,
    pub display_order: i32,
    pub orientation: Option<Orientation>,
    pub topology: LedTopology,
    pub led_mapping: Option<Vec<u32>>,
    pub sampling_mode: Option<SamplingMode>,
    pub edge_behavior: Option<EdgeBehavior>,
    pub shape: Option<ZoneShape>,
    pub shape_preset: Option<String>,
    pub attachment: Option<OutputComponent>,
    pub brightness: Option<f32>,
}
