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

use gloo_net::http::{Request, RequestBuilder};
use serde::{Deserialize, Serialize};

use hypercolor_types::scene::{UnassignedBehavior, Zone};
use hypercolor_types::spatial::{Output, SpatialLayout};

use super::{ApiEnvelope, client};

/// Outcome of a zone mutation guarded by a `groups_revision` precondition.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneOutcome<T> {
    /// The mutation applied; carries whatever the route returned.
    Applied(T),
    /// The `If-Match` precondition failed. `current` is the daemon's
    /// authoritative `groups_revision` to rebase on before retrying.
    Stale { current: u64 },
}

/// Response shape of the zone list / bulk-mutation routes. Studio reads
/// the zone set from the active scene, so this is exercised only by the
/// device-assignment routes (Wave 10).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ZoneListResponse {
    pub items: Vec<Zone>,
    pub groups_revision: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ZoneResponse {
    pub zone: Zone,
    pub groups_revision: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UnassignedBehaviorResponse {
    pub unassigned_behavior: UnassignedBehavior,
    pub groups_revision: u64,
}

#[derive(Debug, Clone, Serialize)]
struct CreateZoneRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

/// Partial zone-metadata patch. Every field is optional; only the supplied
/// ones change. `description` and `color` are doubly-optional so the UI can
/// distinguish "leave unchanged" (`None`) from "clear it" (`Some(None)`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateZoneRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make_primary: Option<bool>,
}

/// One device-output assignment in an [`assign_devices`] request.
/// Mirrors the daemon's untagged enum: `Existing { id }` references an
/// output already in the scene (the daemon moves it); `New(Output)`
/// carries a brand-new output the daemon will place for the first time.
/// Untagged + struct variant makes wire order matter, so `New` is
/// declared first; the daemon expects the same order on its decoder.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OutputAssignment {
    New(Box<Output>),
    Existing { id: String },
}

#[derive(Debug, Clone, Serialize)]
struct AssignDevicesRequest {
    device_zones: Vec<OutputAssignment>,
}

#[derive(Debug, Clone, Serialize)]
struct UpdateUnassignedBehaviorRequest {
    unassigned_behavior: UnassignedBehavior,
}

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
    let body = serde_json::to_string(&CreateZoneRequest {
        name: name.to_owned(),
        color: color.map(str::to_owned),
    })
    .map_err(|error| error.to_string())?;
    send_zone_mutation(
        Request::post(&format!("/api/v1/scenes/{scene_id}/zones")),
        Some(body),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value: ZoneResponse| value.zone))
}

pub async fn update_zone(
    scene_id: &str,
    zone_id: &str,
    request: &UpdateZoneRequest,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<Zone>, String> {
    let body = serde_json::to_string(request).map_err(|error| error.to_string())?;
    send_zone_mutation(
        Request::patch(&format!("/api/v1/scenes/{scene_id}/zones/{zone_id}")),
        Some(body),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value: ZoneResponse| value.zone))
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
    let body = serde_json::to_string(layout).map_err(|error| error.to_string())?;
    send_zone_mutation(
        Request::put(&format!("/api/v1/scenes/{scene_id}/zones/{zone_id}/layout")),
        Some(body),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value: ZoneResponse| value.zone))
}

pub async fn delete_zone(
    scene_id: &str,
    zone_id: &str,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<()>, String> {
    send_zone_mutation(
        Request::delete(&format!("/api/v1/scenes/{scene_id}/zones/{zone_id}")),
        None,
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|_: serde_json::Value| ()))
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
    let body = serde_json::to_string(&AssignDevicesRequest {
        device_zones: assignments,
    })
    .map_err(|error| error.to_string())?;
    send_zone_mutation(
        Request::post(&format!(
            "/api/v1/scenes/{scene_id}/zones/{zone_id}/devices"
        )),
        Some(body),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|response: ZoneListResponse| response.groups_revision))
}

/// Remove one device output from `zone_id`. Returns the new
/// `groups_revision` so sequential removals can chain without a refetch.
pub async fn unassign_device(
    scene_id: &str,
    zone_id: &str,
    device_zone_id: &str,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<u64>, String> {
    send_zone_mutation(
        Request::delete(&format!(
            "/api/v1/scenes/{scene_id}/zones/{zone_id}/devices/{device_zone_id}"
        )),
        None,
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|response: ZoneListResponse| response.groups_revision))
}

pub async fn update_unassigned_behavior(
    scene_id: &str,
    behavior: &UnassignedBehavior,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<UnassignedBehavior>, String> {
    let body = serde_json::to_string(&UpdateUnassignedBehaviorRequest {
        unassigned_behavior: behavior.clone(),
    })
    .map_err(|error| error.to_string())?;
    send_zone_mutation(
        Request::patch(&format!("/api/v1/scenes/{scene_id}/unassigned-behavior")),
        Some(body),
        expected_revision,
    )
    .await
    .map(|outcome| outcome.map(|value: UnassignedBehaviorResponse| value.unassigned_behavior))
}

impl<T> ZoneOutcome<T> {
    fn map<U>(self, transform: impl FnOnce(T) -> U) -> ZoneOutcome<U> {
        match self {
            ZoneOutcome::Applied(value) => ZoneOutcome::Applied(transform(value)),
            ZoneOutcome::Stale { current } => ZoneOutcome::Stale { current },
        }
    }
}

/// Issue one zone mutation, attaching the `If-Match` precondition when a
/// revision is supplied and classifying a `412` reply as
/// [`ZoneOutcome::Stale`]. Hand-rolls the request because the shared
/// `client` helpers expose no header hook; `client::with_auth` is still
/// applied so the daemon's network API-key requirement is honored.
async fn send_zone_mutation<T>(
    builder: RequestBuilder,
    body: Option<String>,
    expected_revision: Option<u64>,
) -> Result<ZoneOutcome<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let mut builder = client::with_auth(builder);
    if let Some(revision) = expected_revision {
        builder = builder.header("If-Match", &revision.to_string());
    }

    let response = match body {
        Some(body) => builder
            .header("Content-Type", "application/json")
            .body(body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?,
        None => builder.send().await.map_err(|error| error.to_string())?,
    };

    match response.status() {
        200..=299 => {
            let envelope: ApiEnvelope<T> =
                response.json().await.map_err(|error| error.to_string())?;
            Ok(ZoneOutcome::Applied(envelope.data))
        }
        412 => {
            let body: serde_json::Value =
                response.json().await.map_err(|error| error.to_string())?;
            let current = body["current"]
                .as_u64()
                .ok_or_else(|| "412 response missing `current` groups_revision".to_owned())?;
            Ok(ZoneOutcome::Stale { current })
        }
        status => Err(format!("HTTP {status}")),
    }
}
