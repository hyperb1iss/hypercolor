//! Scene API client — `/api/v1/scenes/*` routes.

use super::client;

// Wire contracts are shared with the daemon (hypercolor-types::api::scenes).
pub use hypercolor_types::api::scenes::{
    ActiveSceneResponse, CreateSceneRequest, SceneListResponse, SceneSummary, UpdateSceneRequest,
};

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

/// List every user-facing scene (the daemon omits the ephemeral default).
pub async fn list_scenes() -> Result<Vec<SceneSummary>, String> {
    client::fetch_json::<SceneListResponse>("/api/v1/scenes")
        .await
        .map(|response| response.items)
        .map_err(Into::into)
}

/// Create a scene. The daemon seeds it with a Default zone (§5.2).
pub async fn create_scene(name: &str) -> Result<SceneSummary, String> {
    let request = CreateSceneRequest {
        name: name.to_owned(),
        ..CreateSceneRequest::default()
    };
    client::post_json("/api/v1/scenes", &request)
        .await
        .map_err(Into::into)
}

/// Rename a scene. `description` is sent back verbatim because the
/// daemon's PUT replaces it wholesale — omitting it would clear the field.
pub async fn rename_scene(
    scene_id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<(), String> {
    let request = UpdateSceneRequest {
        name: name.to_owned(),
        description: description.map(ToOwned::to_owned),
        ..UpdateSceneRequest::default()
    };
    client::put_json::<_, serde_json::Value>(&format!("/api/v1/scenes/{scene_id}"), &request)
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
