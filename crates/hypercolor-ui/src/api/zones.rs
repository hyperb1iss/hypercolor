//! Live scene zone API client.
use std::collections::{BTreeMap, BTreeSet};

use gloo_net::http::Method;

use hypercolor_types::api::scene::{
    AssignMembersRequest, CreateZoneRequest, MemberPlacement, SceneDocument, ScenePatchRequest,
    ZoneLayoutRequest, ZoneMemberId, ZoneResource,
};
use hypercolor_types::scene::UnassignedBehavior;
use hypercolor_types::spatial::{Output, SpatialLayout};

use super::client;
use super::client::MutationOutcome;
use crate::control_surface_api::path_segment;

pub type ZoneOutcome<T> = MutationOutcome<T>;

pub use hypercolor_types::api::scene::PatchZoneRequest as UpdateZoneRequest;

#[derive(Debug, Clone, PartialEq)]
pub enum OutputAssignment {
    New(Box<Output>),
    Existing { id: String },
}

pub async fn create_zone(
    name: &str,
    color: Option<&str>,
    expected_revision: u64,
) -> Result<ZoneOutcome<ZoneResource>, String> {
    let request = CreateZoneRequest {
        name: name.to_owned(),
        role: None,
        color: color.map(str::to_owned),
    };
    client::send_json_versioned::<_, ZoneResource>(
        Method::POST,
        "/api/v1/scene/zones",
        Some(&request),
        Some(expected_revision),
    )
    .await
    .map_err(Into::into)
}

pub async fn update_zone(
    zone_id: &str,
    request: &UpdateZoneRequest,
    expected_revision: u64,
) -> Result<ZoneOutcome<ZoneResource>, String> {
    client::send_json_versioned::<_, ZoneResource>(
        Method::PATCH,
        &format!("/api/v1/scene/zones/{}", path_segment(zone_id)),
        Some(request),
        Some(expected_revision),
    )
    .await
    .map_err(Into::into)
}

pub async fn update_zone_layout(
    zone_id: &str,
    layout: &SpatialLayout,
    expected_revision: u64,
) -> Result<ZoneOutcome<ZoneResource>, String> {
    let request = ZoneLayoutRequest {
        placements: layout.zones.iter().map(member_placement).collect(),
    };
    client::send_json_versioned::<_, ZoneResource>(
        Method::PUT,
        &format!("/api/v1/scene/zones/{}/layout", path_segment(zone_id)),
        Some(&request),
        Some(expected_revision),
    )
    .await
    .map_err(Into::into)
}

pub async fn delete_zone(zone_id: &str, expected_revision: u64) -> Result<ZoneOutcome<()>, String> {
    client::send_json_versioned::<(), SceneDocument>(
        Method::DELETE,
        &format!("/api/v1/scene/zones/{}", path_segment(zone_id)),
        None,
        Some(expected_revision),
    )
    .await
    .map(|outcome| outcome.map(|_| ()))
    .map_err(Into::into)
}

/// Assign or move outputs through canonical device-and-segment membership
/// writes. Deliberate geometry is restored in one follow-up layout write.
pub async fn assign_devices(
    zone_id: &str,
    assignments: Vec<OutputAssignment>,
    preserve_placement: bool,
    expected_revision: u64,
) -> Result<ZoneOutcome<u64>, String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let mut by_device = BTreeMap::<String, BTreeSet<String>>::new();
    let mut whole_devices = BTreeSet::<String>::new();
    let mut desired_outputs = Vec::new();

    for assignment in assignments {
        match assignment {
            OutputAssignment::New(output) => {
                add_member_target(
                    &mut by_device,
                    &mut whole_devices,
                    &output.device_id,
                    output.zone_name.as_deref(),
                );
                desired_outputs.push(*output);
            }
            OutputAssignment::Existing { id } => {
                let member = scene
                    .zones
                    .iter()
                    .flat_map(|zone| zone.members.iter())
                    .find(|member| member.id.0 == id)
                    .ok_or_else(|| format!("Scene member {id} no longer exists"))?;
                add_member_target(
                    &mut by_device,
                    &mut whole_devices,
                    &member.device_id,
                    member.segment.as_deref(),
                );
            }
        }
    }

    let mut revision = expected_revision;
    let mut written_zone = scene
        .zones
        .into_iter()
        .find(|zone| zone.id.to_string() == zone_id)
        .ok_or_else(|| format!("Zone {zone_id} is not present in the live scene"))?;

    for (device_id, segments) in by_device {
        let request = AssignMembersRequest {
            segments: if whole_devices.contains(&device_id) {
                Vec::new()
            } else {
                segments.into_iter().collect()
            },
            device_id,
        };
        match client::send_json_versioned::<_, ZoneResource>(
            Method::POST,
            &format!("/api/v1/scene/zones/{}/members", path_segment(zone_id)),
            Some(&request),
            Some(revision),
        )
        .await?
        {
            MutationOutcome::Applied(zone) => {
                written_zone = zone;
                revision = revision.saturating_add(1);
            }
            MutationOutcome::Stale { current } => {
                return Ok(MutationOutcome::Stale { current });
            }
        }
    }

    if preserve_placement && !desired_outputs.is_empty() {
        let request = preserved_layout(&written_zone, &desired_outputs);
        match client::send_json_versioned::<_, ZoneResource>(
            Method::PUT,
            &format!("/api/v1/scene/zones/{}/layout", path_segment(zone_id)),
            Some(&request),
            Some(revision),
        )
        .await?
        {
            MutationOutcome::Applied(_) => revision = revision.saturating_add(1),
            MutationOutcome::Stale { current } => {
                return Ok(MutationOutcome::Stale { current });
            }
        }
    }

    Ok(MutationOutcome::Applied(revision))
}

pub async fn unassign_device(
    zone_id: &str,
    member_id: &str,
    expected_revision: u64,
) -> Result<ZoneOutcome<u64>, String> {
    let outcome = client::send_json_versioned::<(), ZoneResource>(
        Method::DELETE,
        &format!(
            "/api/v1/scene/zones/{}/members/{}",
            path_segment(zone_id),
            path_segment(member_id)
        ),
        None,
        Some(expected_revision),
    )
    .await?;
    match outcome {
        MutationOutcome::Applied(_) => Ok(MutationOutcome::Applied(
            expected_revision.saturating_add(1),
        )),
        MutationOutcome::Stale { current } => Ok(MutationOutcome::Stale { current }),
    }
}

pub async fn update_unassigned_behavior(
    behavior: &UnassignedBehavior,
    expected_revision: u64,
) -> Result<ZoneOutcome<UnassignedBehavior>, String> {
    let request = ScenePatchRequest {
        name: None,
        unassigned_behavior: Some(behavior.clone()),
    };
    client::send_json_versioned::<_, SceneDocument>(
        Method::PATCH,
        "/api/v1/scene",
        Some(&request),
        Some(expected_revision),
    )
    .await
    .map(|outcome| outcome.map(|scene| scene.unassigned_behavior))
    .map_err(Into::into)
}

fn add_member_target(
    by_device: &mut BTreeMap<String, BTreeSet<String>>,
    whole_devices: &mut BTreeSet<String>,
    device_id: &str,
    segment: Option<&str>,
) {
    let segments = by_device.entry(device_id.to_owned()).or_default();
    if let Some(segment) = segment {
        segments.insert(segment.to_owned());
    } else {
        whole_devices.insert(device_id.to_owned());
    }
}

fn member_placement(output: &Output) -> MemberPlacement {
    MemberPlacement {
        member: ZoneMemberId(output.id.clone()),
        position: output.position,
        size: output.size,
        rotation: output.rotation,
        scale: output.scale,
        orientation: output.orientation,
        topology: output.topology.clone(),
    }
}

fn preserved_layout(zone: &ZoneResource, desired_outputs: &[Output]) -> ZoneLayoutRequest {
    let mut placements = zone
        .layout
        .as_ref()
        .map(|layout| layout.placements.clone())
        .unwrap_or_default();
    for placement in &mut placements {
        let Some(member) = zone
            .members
            .iter()
            .find(|member| member.id == placement.member)
        else {
            continue;
        };
        let Some(output) = desired_outputs.iter().find(|output| {
            output.device_id == member.device_id && output.zone_name == member.segment
        }) else {
            continue;
        };
        placement.position = output.position;
        placement.size = output.size;
        placement.rotation = output.rotation;
        placement.scale = output.scale;
        placement.orientation = output.orientation;
    }
    ZoneLayoutRequest { placements }
}
