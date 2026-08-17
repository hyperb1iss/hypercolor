//! Display-face API contracts — `/api/v1/displays/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::display::DisplayDescriptor;
use crate::effect::{ControlValue, EffectMetadata};
use crate::scene::{DisplayFaceBlendMode, Zone};

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

impl DisplayFaceScope {
    /// The wire spelling, matching the serde representation and the
    /// `?scope=` query form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Scene => "scene",
        }
    }
}

/// Summary row from `GET /api/v1/displays`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplaySummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub family: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    /// Full surface description (shape, safe area, fps, pixel format) —
    /// the same descriptor injected into face pages.
    pub descriptor: DisplayDescriptor,
}

/// Response from `GET /api/v1/displays/{id}/face` and every face mutation
/// route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayFaceResponse {
    pub device_id: String,
    pub scene_id: String,
    pub effect: EffectMetadata,
    pub zone: Zone,
    /// Which layer the returned assignment lives on.
    #[serde(default)]
    pub live_scope: DisplayFaceScope,
    /// Whether the active scene has its own face assignment for this display.
    #[serde(default)]
    pub scene_assigned: bool,
    /// Whether a persisted default face exists for this display.
    #[serde(default)]
    pub default_assigned: bool,
}

/// Request body for `PUT /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetDisplayFaceRequest {
    pub effect_id: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
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
