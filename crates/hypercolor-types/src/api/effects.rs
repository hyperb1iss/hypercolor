//! Effect API contracts — `/api/v1/effects/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, VariantNames};

use crate::api::envelope::ListResponse;
use crate::control::ControlValue;
pub use crate::effect::EffectCategory;
use crate::effect::{ControlDefinition, EffectSource, PresetTemplate};

/// Rendering implementation used by an effect.
///
/// The catalog publishes the implementation kind without leaking the source
/// file path carried by the engine's internal [`EffectSource`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    EnumString,
    Display,
    VariantNames,
)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum EffectSourceKind {
    Native,
    Html,
    Shader,
}

impl EffectSourceKind {
    /// Canonical wire spelling for this source kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Html => "html",
            Self::Shader => "shader",
        }
    }
}

impl From<&EffectSource> for EffectSourceKind {
    fn from(source: &EffectSource) -> Self {
        match source {
            EffectSource::Native { .. } => Self::Native,
            EffectSource::Html { .. } => Self::Html,
            EffectSource::Shader { .. } => Self::Shader,
        }
    }
}

/// Origin of a preset in an effect's unified preset stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EffectPresetOrigin {
    Bundled,
    Saved,
}

/// One bundled or saved preset projected through an effect-scoped API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
pub type EffectPresetListResponse = ListResponse<EffectPresetSummary>;

/// Response for `GET /api/v1/effects`.
pub type EffectListResponse = ListResponse<EffectSummary>;

/// One effect in the list response.
///
/// `controls` and `presets` are expansions: they are absent unless the
/// request asked for them via `include=controls,presets`, so the default
/// list shape is unchanged and a client that ignores the parameter sees
/// exactly the payload it saw before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: EffectCategory,
    pub source: EffectSourceKind,
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EffectCapabilitySet {
    #[serde(default)]
    pub audio_reactive: bool,
    #[serde(default)]
    pub screen_reactive: bool,
    #[serde(default)]
    pub input_reactive: bool,
}

/// Response for `GET /api/v1/effects/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: EffectCategory,
    pub source: EffectSourceKind,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct InstalledEffectResponse {
    pub id: String,
    pub name: String,
    pub path: String,
    pub controls: usize,
    pub presets: usize,
}

/// Response for `POST /api/v1/effects/rescan`.
///
/// Counts describe what the rescan changed in the registry, so an
/// all-zero response means the effect directories were already current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RescanResponse {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
}
