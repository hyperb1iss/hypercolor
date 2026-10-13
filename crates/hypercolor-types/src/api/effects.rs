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
    /// Effect console cursor taken just before the install. Pass it as
    /// `after` to `GET /api/v1/effects/console` to read only what the new
    /// page prints.
    #[serde(default)]
    pub console_after: u64,
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

/// Severity floor for `GET /api/v1/effects/console`.
///
/// Raw console levels fold onto four floors: `trace` and `debug` are
/// `debug`; `log`, `info`, and `dir` are `log`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
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
pub enum EffectConsoleLevel {
    /// Everything, including debug and trace output.
    #[default]
    Debug,
    /// `console.log`, `info`, and `dir` output and above.
    Log,
    /// Warnings and errors.
    Warn,
    /// Errors only, including uncaught script exceptions.
    Error,
}

impl EffectConsoleLevel {
    /// The floor a raw console level label belongs to.
    #[must_use]
    pub fn of_label(label: &str) -> Self {
        match label {
            "warn" => Self::Warn,
            "error" => Self::Error,
            "debug" | "trace" => Self::Debug,
            _ => Self::Log,
        }
    }
}

/// One message an effect page printed, as `GET /api/v1/effects/console`
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EffectConsoleEntry {
    /// Journal sequence number; strictly increasing within one daemon run.
    pub seq: u64,
    /// Wall-clock time the message was recorded, in Unix milliseconds.
    pub timestamp_ms: u64,
    /// Raw console level: `log`, `info`, `warn`, `error`, `debug`, `trace`,
    /// or `dir`.
    pub level: String,
    /// Message text. File locations in the effect's own page point at the
    /// author's source file and line, not the engine's runtime copy.
    pub message: String,
    /// Display name of the effect that printed it.
    pub effect_name: String,
    /// Catalog id of that effect, when it is still installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<String>,
    /// File name of the effect's HTML source.
    pub source_file: String,
    /// Line in that file the message points at, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Response for `GET /api/v1/effects/console`.
///
/// A cursor-based window over the effect console journal rather than a
/// paged list: pass `next_seq` back as `after` to read only newer output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EffectConsoleResponse {
    /// Matching messages after the cursor, oldest first.
    pub entries: Vec<EffectConsoleEntry>,
    /// Cursor for the next read.
    pub next_seq: u64,
    /// Whether output after the requested cursor was evicted before this
    /// read, so some messages were missed.
    pub truncated: bool,
}
