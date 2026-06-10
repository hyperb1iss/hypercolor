//! Effect API contracts — `/api/v1/effects/*`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::common::Pagination;
use crate::effect::{ControlDefinition, ControlValue, PresetTemplate};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_group_id: Option<String>,
    /// Server-side version token for the group's controls. Clients
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
            render_group_id: None,
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
    /// Optional target zone (render-group id). Omitted applies the effect
    /// to the scene's Primary zone — the legacy behavior. A non-Primary
    /// zone id renders the effect into that zone instead, leaving its
    /// layout and device assignment untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_group: Option<String>,
}

/// Transition override on apply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransitionRequest {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transition_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Request body for `PATCH /api/v1/effects/current/controls`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateCurrentControlsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub controls: Option<serde_json::Value>,
}

/// Optional body for `POST /api/v1/effects/current/reset` — scopes the
/// reset to one zone (`render_group`); omitted resets the primary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResetControlsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_group: Option<String>,
}

/// `{ id, name }` reference to an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EffectRefSummary {
    pub id: String,
    pub name: String,
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
