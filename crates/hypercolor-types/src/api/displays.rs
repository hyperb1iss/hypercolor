//! Display-face API contracts — `/api/v1/displays/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::control::ControlValue;
use crate::display::DisplayDescriptor;
use crate::effect::EffectMetadata;
use crate::layer::BlendMode;
use crate::scene::Zone;

/// Which assignment layer a face operation targets (spec 69 §3.6).
///
/// `default` persists across scenes (the display's own face); `scene`
/// writes into the active scene's display zone, which always wins while
/// that scene is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DisplayFaceResponse {
    pub device_id: String,
    pub scene_id: String,
    pub effect: EffectMetadata,
    #[schema(value_type = Object)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SetDisplayFaceRequest {
    pub effect_id: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<BlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub scope: DisplayFaceScope,
}

/// Query parameters for `DELETE /api/v1/displays/{id}/face`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
pub struct DisplayFaceScopeQuery {
    #[serde(default)]
    #[param(required = false)]
    pub scope: DisplayFaceScope,
}

/// Response from `DELETE /api/v1/displays/{id}/face`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeleteDisplayFaceResponse {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_id: Option<String>,
    pub scope: DisplayFaceScope,
    pub deleted: bool,
}

/// Request body for `PATCH /api/v1/displays/{id}/face/composition`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateDisplayFaceCompositionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<BlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}
