//! Display and face endpoints — `/api/v1/displays/*`.
//!
//! Covers display discovery, face assignment, face control updates, and the
//! preview JPEG URL.

use hypercolor_types::effect::{ControlDefinition, ControlValue, PresetTemplate};
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::client;

/// Summary row from `GET /api/v1/displays`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplaySummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub family: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

/// Effect metadata carried inside a display-face assignment response.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DisplayFaceEffect {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub presets: Vec<PresetTemplate>,
}

/// Render-group details carried inside a display-face assignment response.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DisplayFaceGroup {
    pub id: String,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub display_target: Option<DisplayFaceTarget>,
}

/// Response from `GET /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DisplayFaceResponse {
    pub device_id: String,
    pub scene_id: String,
    pub effect: DisplayFaceEffect,
    pub group: DisplayFaceGroup,
}

/// Request body for `PUT /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SetDisplayFaceRequest {
    pub effect_id: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/composition`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateDisplayFaceCompositionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}

/// `GET /api/v1/displays` — list display-capable devices.
pub async fn fetch_displays() -> Result<Vec<DisplaySummary>, String> {
    client::fetch_json::<Vec<DisplaySummary>>("/api/v1/displays")
        .await
        .map_err(Into::into)
}

/// `GET /api/v1/displays/{id}/face` — fetch the current face assignment.
pub async fn fetch_display_face(display_id: &str) -> Result<Option<DisplayFaceResponse>, String> {
    let url = format!("/api/v1/displays/{display_id}/face");
    client::fetch_json::<Option<DisplayFaceResponse>>(&url)
        .await
        .map_err(|error| error.to_string())
}

/// `PUT /api/v1/displays/{id}/face` — assign a display face in the active scene.
pub async fn set_display_face(
    display_id: &str,
    effect_id: &str,
) -> Result<DisplayFaceResponse, String> {
    let url = format!("/api/v1/displays/{display_id}/face");
    let body = SetDisplayFaceRequest {
        effect_id: effect_id.to_owned(),
        controls: HashMap::new(),
        blend_mode: Some(DisplayFaceBlendMode::Replace),
        opacity: Some(1.0),
    };
    client::put_json::<SetDisplayFaceRequest, DisplayFaceResponse>(&url, &body)
        .await
        .map_err(Into::into)
}

/// `DELETE /api/v1/displays/{id}/face` — clear the face assignment.
pub async fn delete_display_face(display_id: &str) -> Result<(), String> {
    let url = format!("/api/v1/displays/{display_id}/face");
    client::delete_empty(&url).await.map_err(Into::into)
}

/// `PATCH /api/v1/displays/{id}/face/controls` — merge control overrides.
pub async fn update_display_face_controls(
    display_id: &str,
    controls: &serde_json::Value,
) -> Result<DisplayFaceResponse, String> {
    let url = format!("/api/v1/displays/{display_id}/face/controls");
    let body = serde_json::json!({ "controls": controls });
    client::patch_json::<serde_json::Value, DisplayFaceResponse>(&url, &body)
        .await
        .map_err(Into::into)
}

/// `PATCH /api/v1/displays/{id}/face/composition` — update face/effect composition.
pub async fn update_display_face_composition(
    display_id: &str,
    blend_mode: Option<DisplayFaceBlendMode>,
    opacity: Option<f32>,
) -> Result<DisplayFaceResponse, String> {
    let url = format!("/api/v1/displays/{display_id}/face/composition");
    let body = UpdateDisplayFaceCompositionRequest {
        blend_mode,
        opacity,
    };
    client::patch_json::<UpdateDisplayFaceCompositionRequest, DisplayFaceResponse>(&url, &body)
        .await
        .map_err(Into::into)
}

/// URL of the latest composited preview JPEG for a display.
#[must_use]
pub fn display_preview_url(display_id: &str, cache_buster: Option<u64>) -> String {
    cache_buster.map_or_else(
        || format!("/api/v1/displays/{display_id}/preview.jpg"),
        |cb| format!("/api/v1/displays/{display_id}/preview.jpg?ts={cb}"),
    )
}
