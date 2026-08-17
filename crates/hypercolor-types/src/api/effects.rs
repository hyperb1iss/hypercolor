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

/// Optional body for `POST /api/v1/effects/{id}/presets/{preset_id}/apply`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyEffectPresetRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}

/// Response for `GET /api/v1/effects`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
    pub pagination: Pagination,
}

/// One effect in the list response.
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

/// Response for `GET /api/v1/effects/active` — the primary zone's
/// effect, or the idle shape (`state == "idle"`, `id`/`name` null).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ActiveEffectResponse {
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
    #[serde(default)]
    pub active_preset_id: Option<String>,
    #[serde(default)]
    pub active_preset_modified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
    /// Server-side version token for the zone's controls. Clients
    /// that want to use optimistic concurrency on the effect-id PATCH
    /// endpoint echo this value back via `If-Match`. Idle responses
    /// omit it (there's nothing to version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

impl ActiveEffectResponse {
    /// The canonical idle response: no effect running.
    #[must_use]
    pub fn idle() -> Self {
        Self {
            id: None,
            name: None,
            state: "idle".to_owned(),
            controls: Vec::new(),
            control_values: HashMap::new(),
            active_preset_id: None,
            active_preset_modified: false,
            zone_id: None,
            controls_version: None,
            cover_image_url: None,
        }
    }
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
    #[serde(default)]
    pub active_control_values: Option<HashMap<String, ControlValue>>,
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

/// Request body for `POST /api/v1/effects/{id}/apply`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyEffectRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<TransitionRequest>,
    /// Optional preset ID to associate with the zone in the same
    /// transaction as the effect start — lets the UI pass a remembered
    /// preset selection without a follow-up round-trip. If `controls` is
    /// also provided, the explicit controls win (they're presumed to
    /// already carry the preset's values, possibly with user tweaks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    /// Optional target zone id. Omitted applies the effect to the
    /// scene's Primary zone. A non-Primary zone id renders the effect
    /// into that zone instead, leaving its layout and device assignment
    /// untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}

/// Transition override on apply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransitionRequest {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transition_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Request body for `PATCH /api/v1/effects/active/controls`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateActiveControlsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
}

// The two control PATCH responses and the control-binding response are
// deliberately NOT defined here. Their payloads carry f32 control values,
// and the daemon builds them with `serde_json::json!`, which widens f32 to
// f64 and prints the widened digits. A derived struct writes f32 directly,
// so naming those shapes would change the bytes on the wire.

/// Request body for `PUT /api/v1/effects/{id}/layout`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetEffectLayoutRequest {
    /// The spatial layout to associate with the effect.
    pub layout_id: String,
}

/// Response for `GET /api/v1/effects/{id}/layout`.
///
/// `resolved` reports whether the linked layout still exists; a stale
/// link answers `resolved: false` with a `null` `layout` rather than a
/// 404, because the association itself is real.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectLayoutResponse {
    pub effect: EffectRefSummary,
    pub layout_id: String,
    pub resolved: bool,
    pub layout: Option<LayoutLinkSummary>,
}

/// Response for `PUT /api/v1/effects/{id}/layout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetEffectLayoutResponse {
    pub effect: EffectRefSummary,
    pub layout: LayoutLinkSummary,
    pub linked: bool,
}

/// Response for `DELETE /api/v1/effects/{id}/layout`.
///
/// `layout_id` is the association that was removed, and `null` with
/// `deleted: false` when the effect had no layout linked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteEffectLayoutResponse {
    pub effect: EffectRefSummary,
    pub layout_id: Option<String>,
    pub deleted: bool,
}

/// Optional body for `POST /api/v1/effects/active/reset` — scopes the
/// reset to one zone (`zone_id`); omitted resets the primary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResetControlsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}

/// `{ id, name }` reference to an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EffectRefSummary {
    pub id: String,
    pub name: String,
}

/// Compatibility response for `POST /api/v1/effects/pause`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PauseEffectResponse {
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EffectRefSummary>,
    pub off_output_behavior: String,
    pub off_output_color: [u8; 3],
}

/// Compatibility response for `POST /api/v1/effects/resume`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResumeEffectResponse {
    pub resumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<EffectRefSummary>,
}

/// Layout link summary in apply responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LayoutLinkSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
}

/// Result of resolving an effect's associated layout during apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EffectLayoutApplyResult {
    pub associated_layout_id: String,
    pub resolved: bool,
    pub applied: bool,
    pub layout: Option<LayoutLinkSummary>,
}

/// Transition actually applied by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyTransitionResponse {
    #[serde(rename = "type")]
    pub transition_type: String,
    pub duration_ms: u64,
}

/// Response for `POST /api/v1/effects/{id}/apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyEffectResponse {
    pub effect: EffectRefSummary,
    #[schema(value_type = Object)]
    pub applied_controls: serde_json::Value,
    #[serde(default)]
    pub layout: Option<EffectLayoutApplyResult>,
    pub transition: ApplyTransitionResponse,
    #[serde(default)]
    pub warnings: Vec<String>,
}
