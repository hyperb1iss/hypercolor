//! Scene API contracts — `/api/v1/scenes/*`.

use serde::{Deserialize, Serialize};

use crate::api::common::Pagination;
use crate::scene::{SceneKind, SceneMutationMode, UnassignedBehavior, Zone};

/// Response for `GET /api/v1/scenes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneListResponse {
    pub items: Vec<SceneSummary>,
    pub pagination: Pagination,
}

/// One saved scene as listed by `GET /api/v1/scenes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the scene participates in activation. Defaults true for
    /// daemons that predate the field.
    #[serde(default = "default_scene_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u8,
    /// Live vs snapshot-locked. Lets scene pickers mark locked scenes
    /// without joining `/scenes/active`.
    #[serde(default)]
    pub mutation_mode: SceneMutationMode,
}

/// Response for `GET /api/v1/scenes/active` — the active scene with its
/// full render-group (zone) set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveSceneResponse {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_scene_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub kind: SceneKind,
    #[serde(default)]
    pub mutation_mode: SceneMutationMode,
    #[serde(default)]
    pub groups: Vec<Zone>,
    /// Monotonic render-group structure counter. Carried as the
    /// `If-Match` precondition for every zone mutation (Spec 64).
    #[serde(default)]
    pub groups_revision: u64,
    /// Scene-level policy for device outputs claimed by no zone (§9.4).
    #[serde(default)]
    pub unassigned_behavior: UnassignedBehavior,
}

/// Request body for `POST /api/v1/scenes`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreateSceneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_mode: Option<SceneMutationMode>,
}

/// Request body for `PUT /api/v1/scenes/{id}`.
///
/// The daemon replaces `name` and `description` wholesale — clients
/// renaming a scene must echo the existing description back or it is
/// cleared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateSceneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_mode: Option<SceneMutationMode>,
}

const fn default_scene_enabled() -> bool {
    true
}
