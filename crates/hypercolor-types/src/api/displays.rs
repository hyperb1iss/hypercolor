//! Display-face API contracts — `/api/v1/displays/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::effect::ControlValue;
use crate::scene::DisplayFaceBlendMode;

/// Which assignment layer a face operation targets (spec 69 §3.6).
///
/// `default` persists across scenes (the display's own face); `scene`
/// writes into the active scene's display zone, which always wins while
/// that scene is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFaceScope {
    #[default]
    Default,
    Scene,
}

/// Request body for `PUT /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetDisplayFaceRequest {
    pub effect_id: String,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub scope: DisplayFaceScope,
}

/// Query parameters for `DELETE /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayFaceScopeQuery {
    #[serde(default)]
    pub scope: DisplayFaceScope,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/controls`.
///
/// The payload carries only the overrides the caller wants to change;
/// existing control values on the zone are preserved unless their
/// key appears in this map. `controls` is typed as raw JSON (rather than
/// `HashMap<String, ControlValue>`) so callers can send natural shapes
/// like `{"accent": 0.5}` instead of `{"accent": {"float": 0.5}}`, which
/// mirrors the effects controls patch endpoint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateDisplayFaceControlsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<serde_json::Value>,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/composition`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateDisplayFaceCompositionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<DisplayFaceBlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}
