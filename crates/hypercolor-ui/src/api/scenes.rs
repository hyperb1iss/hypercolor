use serde::Deserialize;

use hypercolor_types::scene::{RenderGroup, SceneKind, SceneMutationMode, UnassignedBehavior};

use super::client;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActiveSceneResponse {
    pub id: String,
    pub name: String,
    pub kind: SceneKind,
    pub mutation_mode: SceneMutationMode,
    #[serde(default)]
    pub groups: Vec<RenderGroup>,
    /// Monotonic render-group structure counter. Carried as the
    /// `If-Match` precondition for every zone mutation (Spec 64).
    #[serde(default)]
    pub groups_revision: u64,
    /// Scene-level policy for device outputs claimed by no zone (§9.4).
    #[serde(default)]
    pub unassigned_behavior: UnassignedBehavior,
}

pub async fn fetch_active_scene() -> Result<Option<ActiveSceneResponse>, String> {
    client::fetch_json_optional("/api/v1/scenes/active")
        .await
        .map_err(Into::into)
}

pub async fn deactivate_scene() -> Result<(), String> {
    client::post_empty("/api/v1/scenes/deactivate")
        .await
        .map_err(Into::into)
}
