//! Display and face endpoints — `/api/v1/displays/*`.
//!
//! Covers display discovery, face assignment, face control updates, and the
//! preview JPEG URL.

use hypercolor_types::layer::{BlendMode, LayerSource};
use std::collections::{BTreeMap, HashMap};

use super::{ApiResult, client};

pub use hypercolor_types::api::displays::{
    DisplayFaceResponse, DisplayFaceScope, DisplaySummary, SetDisplayFaceRequest,
    UpdateDisplayFaceCompositionRequest,
};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::control::ControlValue;

/// `GET /api/v1/displays` — list display-capable devices.
pub async fn fetch_displays() -> ApiResult<Vec<DisplaySummary>> {
    client::fetch_json::<Vec<DisplaySummary>>("/api/v1/displays").await
}

/// `GET /api/v1/displays/{id}/face` — fetch the current face assignment.
pub async fn fetch_display_face(display_id: &str) -> ApiResult<Option<DisplayFaceResponse>> {
    let url = format!("/api/v1/displays/{display_id}/face");
    client::fetch_json::<Option<DisplayFaceResponse>>(&url).await
}

/// `PUT /api/v1/displays/{id}/face` — assign a display face on the chosen layer.
pub async fn set_display_face(
    display_id: &str,
    effect_id: &str,
    scope: DisplayFaceScope,
) -> ApiResult<DisplayFaceResponse> {
    let url = format!("/api/v1/displays/{display_id}/face");
    let body = SetDisplayFaceRequest {
        effect_id: effect_id.to_owned(),
        controls: HashMap::new(),
        // Default to a blended overlay so a freshly-assigned face layers
        // over the live effect — transparent regions reveal it — instead of
        // blacking the effect out. Replace stays available in the
        // composition panel for face-only looks.
        blend_mode: Some(BlendMode::Alpha),
        opacity: Some(1.0),
        // The screen's mounting rotation is a display property; assigning
        // a face keeps whatever the display already has.
        rotation: None,
        scope,
    };
    client::put_json::<SetDisplayFaceRequest, DisplayFaceResponse>(&url, &body).await
}

/// Copy a display's live default face into the active scene as that
/// screen's own layer, carrying the default's controls and composition so
/// the screen looks the same the moment it becomes scene-editable.
pub async fn promote_default_face(
    display_id: &str,
    face: &DisplayFaceResponse,
) -> ApiResult<DisplayFaceResponse> {
    let url = format!("/api/v1/displays/{display_id}/face");
    let controls = face
        .zone
        .layers
        .iter()
        .find_map(|layer| match &layer.source {
            LayerSource::Effect { controls, .. } => Some(controls.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let target = face.zone.display_target.as_ref();
    let body = SetDisplayFaceRequest {
        effect_id: face.effect.id.to_string(),
        controls,
        blend_mode: Some(target.map_or(BlendMode::Alpha, |target| target.blend_mode)),
        opacity: Some(target.map_or(1.0, |target| target.opacity)),
        scope: DisplayFaceScope::Scene,
    };
    client::put_json::<SetDisplayFaceRequest, DisplayFaceResponse>(&url, &body).await
}

/// `DELETE /api/v1/displays/{id}/face?scope=...` — clear one layer's assignment.
pub async fn delete_display_face(display_id: &str, scope: DisplayFaceScope) -> ApiResult<()> {
    let url = format!(
        "/api/v1/displays/{display_id}/face?scope={}",
        scope.as_str()
    );
    client::delete_empty(&url).await
}

/// `PATCH /api/v1/displays/{id}/face/controls` — merge control overrides.
pub async fn update_display_face_controls(
    display_id: &str,
    values: BTreeMap<String, ControlValue>,
) -> ApiResult<DisplayFaceResponse> {
    let url = format!("/api/v1/displays/{display_id}/face/controls");
    let body = PatchControlsRequest {
        values,
        clear_bindings: Vec::new(),
    };
    client::patch_json::<PatchControlsRequest, DisplayFaceResponse>(&url, &body).await
}

/// `PATCH /api/v1/displays/{id}/face/composition` — update face/effect composition.
pub async fn update_display_face_composition(
    display_id: &str,
    blend_mode: Option<BlendMode>,
    opacity: Option<f32>,
) -> ApiResult<DisplayFaceResponse> {
    let url = format!("/api/v1/displays/{display_id}/face/composition");
    let body = UpdateDisplayFaceCompositionRequest {
        blend_mode,
        opacity,
        rotation: None,
    };
    client::patch_json::<UpdateDisplayFaceCompositionRequest, DisplayFaceResponse>(&url, &body)
        .await
}

/// URL of the latest composited preview JPEG for a display.
#[must_use]
pub fn display_preview_url(display_id: &str, cache_buster: Option<u64>) -> String {
    client::daemon_url(&cache_buster.map_or_else(
        || format!("/api/v1/displays/{display_id}/frame"),
        |cb| format!("/api/v1/displays/{display_id}/frame?ts={cb}"),
    ))
    .unwrap_or_default()
}
