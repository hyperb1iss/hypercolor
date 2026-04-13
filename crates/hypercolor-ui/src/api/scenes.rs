use serde::Deserialize;

use hypercolor_types::scene::{SceneKind, SceneMutationMode};

use super::client;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActiveSceneResponse {
    pub id: String,
    pub name: String,
    pub kind: SceneKind,
    pub mutation_mode: SceneMutationMode,
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
