//! Scene API client — `/api/v1/scenes/*` routes.

use serde::{Deserialize, Serialize};

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

/// One scene as the scene selector lists it. `description` is held so a
/// rename can echo it back: the daemon's PUT replaces the field wholesale.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SceneSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SceneListResponse {
    items: Vec<SceneSummary>,
}

/// List every user-facing scene (the daemon omits the ephemeral default).
pub async fn list_scenes() -> Result<Vec<SceneSummary>, String> {
    client::fetch_json::<SceneListResponse>("/api/v1/scenes")
        .await
        .map(|response| response.items)
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
struct CreateSceneRequest<'a> {
    name: &'a str,
}

/// Create a scene. The daemon seeds it with a Default zone (§5.2).
pub async fn create_scene(name: &str) -> Result<SceneSummary, String> {
    client::post_json("/api/v1/scenes", &CreateSceneRequest { name })
        .await
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
struct UpdateSceneRequest<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

/// Rename a scene. `description` is sent back verbatim because the
/// daemon's PUT replaces it wholesale — omitting it would clear the field.
pub async fn rename_scene(
    scene_id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<(), String> {
    client::put_json::<_, serde_json::Value>(
        &format!("/api/v1/scenes/{scene_id}"),
        &UpdateSceneRequest { name, description },
    )
    .await
    .map(|_| ())
    .map_err(Into::into)
}

/// Delete a scene.
pub async fn delete_scene(scene_id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/scenes/{scene_id}"))
        .await
        .map_err(Into::into)
}

/// Activate a scene, making it the one the render loop composes.
pub async fn activate_scene(scene_id: &str) -> Result<(), String> {
    client::post_empty(&format!("/api/v1/scenes/{scene_id}/activate"))
        .await
        .map_err(Into::into)
}
