//! Effect API contracts — `/api/v1/effects/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::common::Pagination;
use crate::effect::{ControlDefinition, ControlValue, PresetTemplate};

/// Origin of a preset in an effect's unified preset stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectPresetOrigin {
    Bundled,
    Saved,
}

/// One bundled or saved preset projected through an effect-scoped API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectPresetSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub effect_id: String,
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub origin: EffectPresetOrigin,
    pub editable: bool,
}

/// Response for `GET /api/v1/effects/{id}/presets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectPresetListResponse {
    pub items: Vec<EffectPresetSummary>,
    pub pagination: Pagination,
}

/// Response for `GET /api/v1/effects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
    pub pagination: Pagination,
}

/// One effect in the list response.
///
/// `controls` and `presets` are expansions: they are absent unless the
/// request asked for them via `include=controls,presets`, so the default
/// list shape is unchanged and a client that ignores the parameter sees
/// exactly the payload it saw before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    #[serde(default)]
    pub audio_reactive: bool,
    #[serde(default)]
    pub input_reactive: bool,
    #[serde(default)]
    pub capabilities: EffectCapabilitySet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<Vec<ControlDefinition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<PresetTemplate>>,
}

/// Typed source requirements declared by an effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EffectCapabilitySet {
    #[serde(default)]
    pub audio_reactive: bool,
    #[serde(default)]
    pub screen_reactive: bool,
    #[serde(default)]
    pub input_reactive: bool,
}

/// Response for `GET /api/v1/effects/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<PresetTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

/// Response for `POST /api/v1/effects/install`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InstalledEffectResponse {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: String,
    pub controls: usize,
    pub presets: usize,
}

/// Response for `POST /api/v1/effects/rescan`.
///
/// Counts describe what the rescan changed in the registry, so an
/// all-zero response means the effect directories were already current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescanResponse {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
}
