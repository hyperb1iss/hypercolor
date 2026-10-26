//! Live scene zone API client.

use hypercolor_types::api::scene::{
    CreateZoneRequest, MemberPlacement, SceneDocument, ScenePatchRequest, ZoneLayoutRequest,
    ZoneMemberId, ZoneResource,
};
use hypercolor_types::scene::UnassignedBehavior;
use hypercolor_types::spatial::{Output, SpatialLayout};

use super::client::{Fence, KeyedResponse, MutationOutcome};
use super::http_transport::HttpMethod;
use super::{ApiResult, client};
use crate::control_surface_api::path_segment;

pub type ZoneOutcome<T> = MutationOutcome<T>;

// Studio's scene writes go out under an idempotency key naming the one
// logical write, so a retry of a lost reply never applies twice.

/// Apply one reversible membership transaction against the active scene.
pub async fn edit_members(
    request: &hypercolor_types::api::scene::EditMembersRequest,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<hypercolor_types::api::scene::EditMembersResponse> {
    client::send_json_keyed(
        HttpMethod::Post,
        "/api/v1/scene/members/edit",
        Some(request),
        Some(fence),
        key,
    )
    .await
    .map(|(response, _)| response)
}

pub use hypercolor_types::api::scene::PatchZoneRequest;

pub async fn create_zone(
    name: &str,
    color: Option<&str>,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<ZoneResource> {
    let request = CreateZoneRequest {
        name: name.to_owned(),
        role: None,
        color: color.map(str::to_owned),
    };
    client::send_json_keyed::<_, ZoneResource>(
        HttpMethod::Post,
        "/api/v1/scene/zones",
        Some(&request),
        Some(fence),
        key,
    )
    .await
    .map(|(zone, _)| zone)
}

pub async fn update_zone(
    zone_id: &str,
    request: &PatchZoneRequest,
    fence: &Fence,
) -> ApiResult<ZoneOutcome<ZoneResource>> {
    client::send_json_versioned::<_, ZoneResource>(
        HttpMethod::Patch,
        &format!("/api/v1/scene/zones/{}", path_segment(zone_id)),
        Some(request),
        Some(fence),
    )
    .await
}

/// [`update_zone`] under an idempotency key.
pub async fn update_zone_keyed(
    zone_id: &str,
    request: &PatchZoneRequest,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<ZoneResource> {
    client::send_json_keyed::<_, ZoneResource>(
        HttpMethod::Patch,
        &format!("/api/v1/scene/zones/{}", path_segment(zone_id)),
        Some(request),
        Some(fence),
        key,
    )
    .await
    .map(|(zone, _)| zone)
}

/// Save a zone's placements. The applied outcome carries the scene revision
/// the daemon reported, which a save of a primary zone that follows the
/// active layout moves by two.
pub async fn update_zone_layout(
    zone_id: &str,
    layout: &SpatialLayout,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<(ZoneResource, Option<u64>)> {
    let request = ZoneLayoutRequest {
        placements: layout.zones.iter().map(member_placement).collect(),
    };
    client::send_json_keyed::<_, ZoneResource>(
        HttpMethod::Put,
        &format!("/api/v1/scene/zones/{}/layout", path_segment(zone_id)),
        Some(&request),
        Some(fence),
        key,
    )
    .await
}

pub async fn delete_zone(zone_id: &str, fence: &Fence, key: &str) -> KeyedResponse<()> {
    client::send_json_keyed::<(), SceneDocument>(
        HttpMethod::Delete,
        &format!("/api/v1/scene/zones/{}", path_segment(zone_id)),
        None,
        Some(fence),
        key,
    )
    .await
    .map(|_| ())
}

pub async fn update_unassigned_behavior(
    behavior: &UnassignedBehavior,
    fence: &Fence,
    key: &str,
) -> KeyedResponse<UnassignedBehavior> {
    let request = ScenePatchRequest {
        name: None,
        unassigned_behavior: Some(behavior.clone()),
    };
    client::send_json_keyed::<_, SceneDocument>(
        HttpMethod::Patch,
        "/api/v1/scene",
        Some(&request),
        Some(fence),
        key,
    )
    .await
    .map(|(scene, _)| scene.unassigned_behavior)
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
