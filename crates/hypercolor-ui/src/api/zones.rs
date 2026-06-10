//! Multi-zone scene API client — Spec 64 `/scenes/{id}/zones/*` routes.
//!
//! Every mutation is guarded by an `If-Match: "<groups_revision>"`
//! precondition, mirroring the layer-stack concurrency model in
//! [`super::layers`]. The daemon replies `412` with the authoritative
//! `current` revision when the precondition fails; that is surfaced as
//! [`ZoneOutcome::Stale`] so callers can refetch the active scene and
//! retry rather than silently clobbering a concurrent edit.
//!
//! This is a complete client for the zone routes; the device-assignment
//! and unassigned-behavior calls are consumed by the Wave 10 Layout-view
//! work, so `dead_code` is allowed module-wide rather than annotated call
//! by call.
#![allow(dead_code)]

use gloo_net::http::Method;

use hypercolor_types::scene::{UnassignedBehavior, Zone};
use hypercolor_types::spatial::SpatialLayout;

use super::client;
use super::client::MutationOutcome;

/// Outcome of a zone mutation guarded by a `groups_revision` precondition.
/// `Stale { current }` carries the daemon's authoritative `groups_revision`
/// to rebase on before retrying.
pub type ZoneOutcome<T> = MutationOutcome<T>;

// Wire contracts are shared with the daemon (hypercolor-types::api::zones);
// OutputAssignment's untagged variant order is part of the contract — see
// the shared definition.
pub use hypercolor_types::api::zones::{
    AssignDevicesRequest, CreateZoneRequest, OutputAssignment, UnassignedBehaviorResponse,
    UpdateUnassignedBehaviorRequest, UpdateZoneRequest, ZoneListResponse, ZoneMutationResponse,
    ZoneResponse,
};

pub async fn list_zones(scene_id: &str) -> Result<ZoneListResponse, String> {
    client::fetch_json(&format!("/api/v1/scenes/{scene_id}/zones"))
        .await
        .map_err(Into::into)
}

pub async fn create_zone(
    scene_id: &str,
    name: &str,
    color: Option<&str>,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<Zone>, String> {
    let request = CreateZoneRequest {
        name: name.to_owned(),
        color: color.map(str::to_owned),
    };
    client::send_json_versioned::<_, ZoneResponse>(
        Method::POST,
        &format!("/api/v1/scenes/{scene_id}/zones"),
        Some(&request),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value| value.zone))
    .map_err(Into::into)
}

pub async fn update_zone(
    scene_id: &str,
    zone_id: &str,
    request: &UpdateZoneRequest,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<Zone>, String> {
    client::send_json_versioned::<_, ZoneResponse>(
        Method::PATCH,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}"),
        Some(request),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value| value.zone))
    .map_err(Into::into)
}

/// Apply a placement-only update to a zone's spatial layout (§5.1). The
/// daemon rejects an output-set change with 422; only the placement,
/// ordering, and canvas of the outputs the zone already owns may change.
pub async fn update_zone_layout(
    scene_id: &str,
    zone_id: &str,
    layout: &SpatialLayout,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<Zone>, String> {
    client::send_json_versioned::<_, ZoneResponse>(
        Method::PUT,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layout"),
        Some(layout),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value| value.zone))
    .map_err(Into::into)
}

pub async fn delete_zone(
    scene_id: &str,
    zone_id: &str,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<()>, String> {
    client::send_json_versioned::<(), serde_json::Value>(
        Method::DELETE,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}"),
        None,
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|_| ()))
    .map_err(Into::into)
}

/// Reassign device outputs into `zone_id`. Existing outputs are
/// referenced by id and moved between zones; brand-new ones carry a
/// full `Output` so an unplaced device can be placed for the first
/// time. Returns the new `groups_revision` so a follow-up mutation can
/// chain without a refetch.
pub async fn assign_devices(
    scene_id: &str,
    zone_id: &str,
    assignments: Vec<OutputAssignment>,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<u64>, String> {
    let request = AssignDevicesRequest {
        device_zones: assignments,
    };
    client::send_json_versioned::<_, ZoneListResponse>(
        Method::POST,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/devices"),
        Some(&request),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|response| response.groups_revision))
    .map_err(Into::into)
}

/// Remove one device output from `zone_id`. Returns the new
/// `groups_revision` so sequential removals can chain without a refetch.
pub async fn unassign_device(
    scene_id: &str,
    zone_id: &str,
    device_zone_id: &str,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<u64>, String> {
    client::send_json_versioned::<(), ZoneListResponse>(
        Method::DELETE,
        &format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/devices/{device_zone_id}"),
        None,
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|response| response.groups_revision))
    .map_err(Into::into)
}

pub async fn update_unassigned_behavior(
    scene_id: &str,
    behavior: &UnassignedBehavior,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<UnassignedBehavior>, String> {
    let request = UpdateUnassignedBehaviorRequest {
        unassigned_behavior: behavior.clone(),
    };
    client::send_json_versioned::<_, UnassignedBehaviorResponse>(
        Method::PATCH,
        &format!("/api/v1/scenes/{scene_id}/unassigned-behavior"),
        Some(&request),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value| value.unassigned_behavior))
    .map_err(Into::into)
}
