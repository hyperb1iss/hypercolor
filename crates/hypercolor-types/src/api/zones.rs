//! Zone (render group) API contracts — `/api/v1/scenes/{id}/zones/*`.
//!
//! Every mutation is guarded by an `If-Match: <groups_revision>`
//! precondition; the daemon replies 412 with the authoritative revision
//! when it fails.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::scene::{UnassignedBehavior, Zone};
use crate::spatial::Output;

/// Response for `GET /api/v1/scenes/{id}/zones`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ZoneListResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<Zone>,
    pub groups_revision: u64,
}

/// Response carrying one zone after a create/get/update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ZoneResponse {
    #[schema(value_type = Object)]
    pub zone: Zone,
    pub groups_revision: u64,
}

/// Response carrying the full zone set after a bulk mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ZoneMutationResponse {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<Zone>,
    pub groups_revision: u64,
}

/// Response for the unassigned-behavior PATCH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UnassignedBehaviorResponse {
    #[schema(value_type = String)]
    pub unassigned_behavior: UnassignedBehavior,
    pub groups_revision: u64,
}

/// Request body for `POST /api/v1/scenes/{id}/zones`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateZoneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Partial zone-metadata patch for `PATCH /api/v1/scenes/{id}/zones/{zone_id}`.
///
/// Every field is optional; only supplied ones change. `description` and
/// `color` are doubly-optional so clients can distinguish "leave
/// unchanged" (`None`, skipped on the wire) from "clear it"
/// (`Some(None)`, serialized as `null`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct UpdateZoneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub color: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_primary: Option<bool>,
}

/// One device-output assignment in an [`AssignDevicesRequest`].
///
/// Untagged: serde tries variants in declaration order and the first
/// match wins. `Existing { id }` ignores unknown fields by default, so
/// it would silently swallow any object that has an `id`, including a
/// full `Output`. `New` is declared FIRST so a brand-new output (which
/// has every required `Output` field) matches it; a bare `{ "id": …  }`
/// lacks those fields, `New` fails, and the parser falls through to
/// `Existing`. Do not reorder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputAssignment {
    New(Box<Output>),
    Existing { id: String },
}

/// Request body for `POST /api/v1/scenes/{id}/zones/{zone_id}/devices`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AssignDevicesRequest {
    #[schema(value_type = Vec<Object>)]
    pub device_zones: Vec<OutputAssignment>,
}

/// Request body for `PATCH /api/v1/scenes/{id}/unassigned-behavior`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateUnassignedBehaviorRequest {
    #[schema(value_type = String)]
    pub unassigned_behavior: UnassignedBehavior,
}
