//! The live scene tree's domain services (Spec 78 §1).
//!
//! `/scene` is a view of one thing — the active scene — so everything
//! here reads or mutates the live tree and nothing takes a scene id.
//! The projection turns the engine's `Scene`/`Zone` into the wire
//! vocabulary (`SceneDocument`, `ZoneResource`, `ZoneMember`), and the
//! services below cover the mutations the pre-existing zone and layer
//! services do not: the combined scene patch, the clear gesture, the
//! device-and-segment member assignment, and whole-layer replacement,
//! which mints a fresh id rather than writing in place (§1.4).
//!
//! One wire version governs the tree: the commit generation
//! ([`SceneCommit::revision`](super::commit::SceneCommit::revision)),
//! served as `ETag` and checked by [`check_scene_revision`]. The
//! per-subresource counters the engine keeps (`groups_revision`,
//! `layers_version`, `controls_version`) stay internal bookkeeping and
//! never reach this surface.

use std::collections::HashMap;

use hypercolor_types::api::scene::{
    AssignMembersRequest, MemberPlacement, SceneDocument, ZoneLayoutResource, ZoneMember,
    ZoneMemberId, ZoneResource,
};
use hypercolor_types::event::{
    HypercolorEvent, LayerStackChangeKind, SceneLibraryChangeKind, SceneSettingsChangeKind,
    ZoneChangeKind,
};
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Scene, SceneId, UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};

use hypercolor_core::scene::{LayerMutationError, OutputPlacement};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{SceneMutation, commit_scene, zone_changed_event};
use crate::domain::{DomainError, MutationContext, ResourceKind};

// ── Projection ───────────────────────────────────────────────────────────

/// Read the live tree as the `GET /scene` document.
///
/// An active scene always exists (Spec 78 §1.1), so the only failure
/// this can report is an engine that lost its default scene, which the
/// invariant forbids.
///
/// # Errors
///
/// [`DomainError::NotFound`] when no scene is active, which the
/// always-a-default invariant makes unreachable in practice.
pub async fn read_document(state: &AppState) -> Result<SceneDocument, DomainError> {
    let revision = state.scene_commits.revision();
    let manager = state.scene_manager.read().await;
    let scene = manager
        .active_scene()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
    Ok(scene_document(scene, revision))
}

/// Read one zone of the live tree.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone or a missing active
/// scene.
pub async fn read_zone(state: &AppState, zone_id: ZoneId) -> Result<ZoneResource, DomainError> {
    let manager = state.scene_manager.read().await;
    let scene = manager
        .active_scene()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
    scene
        .groups
        .iter()
        .find(|zone| zone.id == zone_id)
        .map(zone_resource)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))
}

/// Project a scene plus the current commit generation onto the wire
/// document.
#[must_use]
pub fn scene_document(scene: &Scene, revision: u64) -> SceneDocument {
    SceneDocument {
        id: scene.id,
        name: scene.name.clone(),
        kind: scene.kind,
        is_default: scene.id.is_default(),
        unassigned_behavior: scene.unassigned_behavior.clone(),
        layout_id: scene.layout_id.clone(),
        revision,
        zones: scene.groups.iter().map(zone_resource).collect(),
    }
}

/// Project one zone onto the wire resource.
///
/// The layer stack comes from `effective_layers`, which is what the
/// renderer reads, so a client patches the id the daemon actually
/// addresses rather than synthesizing one (Spec 78 §1.3).
#[must_use]
pub fn zone_resource(zone: &Zone) -> ZoneResource {
    ZoneResource {
        id: zone.id,
        name: zone.name.clone(),
        role: zone.role,
        enabled: zone.enabled,
        brightness: zone.brightness,
        color: zone.color.clone(),
        display_target: zone.display_target.clone(),
        members: zone.layout.zones.iter().map(zone_member).collect(),
        layout: zone_layout_resource(zone),
        layers: zone.effective_layers(),
    }
}

fn zone_member(output: &Output) -> ZoneMember {
    ZoneMember {
        id: ZoneMemberId(output.id.clone()),
        device_id: output.device_id.clone(),
        segment: output.zone_name.clone(),
        name: output.name.clone(),
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

/// A zone with no members overrides nothing, so it reads back as
/// `None` rather than an empty placement list.
fn zone_layout_resource(zone: &Zone) -> Option<ZoneLayoutResource> {
    if zone.layout.zones.is_empty() {
        return None;
    }
    Some(ZoneLayoutResource {
        placements: zone.layout.zones.iter().map(member_placement).collect(),
    })
}

// ── Concurrency ──────────────────────────────────────────────────────────

/// Refuse a mutation whose caller last saw a different tree.
///
/// The token is the commit generation, so one number covers every
/// structural write anywhere in the tree (Spec 78 §1.6). A control
/// write passes `None`: value writes are unguarded by contract, fenced
/// instead by layer identity.
///
/// # Errors
///
/// [`DomainError::PreconditionFailed`] carrying the current revision.
pub fn check_scene_revision(
    mutation: &SceneMutation,
    expected: Option<u64>,
) -> Result<(), DomainError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let current = mutation.base_revision();
    if expected == current {
        return Ok(());
    }
    Err(DomainError::PreconditionFailed {
        resource: ResourceKind::Scene,
        expected,
        current,
    })
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Patch the live scene's own fields (Spec 78 §1.2).
#[derive(Debug, Clone, Default)]
pub struct PatchScene {
    /// Rename; refused for the auto-managed default scene.
    pub name: Option<String>,
    /// What unclaimed outputs should render.
    pub unassigned_behavior: Option<UnassignedBehavior>,
    /// The `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Empty layer stacks — one zone's, or every zone's (Spec 78 §1.2).
#[derive(Debug, Clone, Default)]
pub struct ClearScene {
    /// Which zone to empty; `None` empties every zone.
    pub zone: Option<ZoneId>,
    /// The `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Assign one device's segments to a zone (Spec 78 §1.2).
#[derive(Debug, Clone)]
pub struct AssignMembers {
    /// Which zone receives the segments.
    pub zone_id: ZoneId,
    /// The device and segments to assign.
    pub request: AssignMembersRequest,
    /// The `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Replace one layer, minting a fresh identity (Spec 78 §1.4).
#[derive(Debug, Clone)]
pub struct ReplaceLayer {
    /// Which zone owns the stack.
    pub zone_id: ZoneId,
    /// Which layer is being replaced.
    pub layer_id: SceneLayerId,
    /// The replacement, already validated and carrying its new id.
    pub layer: SceneLayer,
    /// The `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Write control values and drop named bindings on one layer
/// (Spec 78 §1.6).
#[derive(Debug, Clone)]
pub struct PatchLayerControls {
    /// Which zone owns the layer.
    pub zone_id: ZoneId,
    /// Which layer to patch.
    pub layer_id: SceneLayerId,
    /// The values to write.
    pub values: HashMap<String, hypercolor_types::effect::ControlValue>,
    /// Bindings to remove in the same commit.
    pub clear_bindings: Vec<String>,
}

/// What a tree mutation reports back.
#[derive(Debug)]
pub struct TreeWritten {
    /// The live document as it now stands.
    pub document: SceneDocument,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// What a zone-scoped mutation reports back.
#[derive(Debug)]
pub struct ZoneWritten {
    /// The zone as it now stands.
    pub zone: ZoneResource,
    /// The scene revision after the mutation.
    pub revision: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

// ── Services ─────────────────────────────────────────────────────────────

/// Patch the live scene's name and unassigned-output policy.
///
/// Both fields land in one commit, so a caller that changes both reads
/// one revision back rather than racing its own two writes.
///
/// # Errors
///
/// [`DomainError::Validation`] for a blank name or a rename of the
/// default scene, [`DomainError::PreconditionFailed`] for a stale
/// revision, and the commit path's own refusals.
pub async fn patch_scene(
    state: &AppState,
    command: PatchScene,
    meta: MutationContext,
) -> Result<TreeWritten, DomainError> {
    let _ = meta;

    if let Some(name) = &command.name
        && name.trim().is_empty()
    {
        return Err(DomainError::validation_field(
            "name",
            "scene name must not be empty",
        ));
    }

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("patching the scene")?;

    if let Some(name) = command.name {
        if scene_id.is_default() {
            return Err(DomainError::validation_field(
                "name",
                "the default scene cannot be renamed",
            ));
        }
        let mut scene = active_scene(&mutation)?;
        scene.name = name;
        mutation.update_scene(scene.clone())?;
        mutation.record(HypercolorEvent::SceneLibraryChanged {
            scene_id,
            kind: SceneLibraryChangeKind::Updated,
            name: Some(scene.name),
        });
    }

    if let Some(behavior) = command.unassigned_behavior {
        mutation
            .set_unassigned_behavior(scene_id, behavior)
            .map_err(|_| DomainError::not_found(ResourceKind::Scene, scene_id))?;
        let zones_revision = active_scene(&mutation)?.groups_revision;
        mutation.record(HypercolorEvent::SceneSettingsChanged {
            scene_id,
            zones_revision,
            kind: SceneSettingsChangeKind::UnassignedBehavior,
        });
    }

    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;
    Ok(TreeWritten {
        document: read_document(state).await?,
        commit,
    })
}

/// Empty one zone's layer stack, or every zone's — the stop gesture.
///
/// Clearing every zone also quiesces output, which is what the deleted
/// `/effects/stop` did; clearing one zone leaves the rest rendering and
/// therefore leaves output alone.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone,
/// [`DomainError::PreconditionFailed`] for a stale revision, and the
/// commit path's own refusals.
pub async fn clear_scene(
    state: &AppState,
    command: ClearScene,
    meta: MutationContext,
) -> Result<TreeWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("clearing the scene")?;

    let targets: Vec<ZoneId> = match command.zone {
        Some(zone_id) => {
            let scene = active_scene(&mutation)?;
            let zone = scene
                .groups
                .iter()
                .find(|zone| zone.id == zone_id)
                .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
            if zone.role == ZoneRole::Display {
                return Err(DomainError::validation(
                    "display zones carry a face, which is cleared through \
                     DELETE /displays/{id}/face",
                ));
            }
            vec![zone_id]
        }
        None => active_scene(&mutation)?
            .groups
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .map(|zone| zone.id)
            .collect(),
    };

    for zone_id in targets {
        if let Some(zone) = mutation.clear_zone_effect(zone_id) {
            mutation.record(zone_changed_event(scene_id, &zone, ZoneChangeKind::Updated));
        }
    }

    let commit = commit_scene(state, mutation).await?;
    if command.zone.is_none() {
        crate::api::effects::quiesce_output_after_effect_stop(state).await;
    }
    crate::api::save_runtime_session_snapshot(state).await;

    Ok(TreeWritten {
        document: read_document(state).await?,
        commit,
    })
}

/// Replace one layer with a freshly identified one.
///
/// Replacement is creation (Spec 78 §1.4): the old layer is removed and
/// the new one takes its slot, so the caller's next control patch either
/// names the id this returns or 404s.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone or layer,
/// [`DomainError::Validation`] for an invalid layer payload,
/// [`DomainError::PreconditionFailed`] for a stale revision.
pub async fn replace_layer(
    state: &AppState,
    command: ReplaceLayer,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("replacing a layer")?;

    let index = active_scene(&mutation)?
        .groups
        .iter()
        .find(|zone| zone.id == command.zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, command.zone_id))?
        .effective_layers()
        .iter()
        .position(|layer| layer.id == command.layer_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Layer, command.layer_id))?;

    mutation
        .remove_layer(scene_id, command.zone_id, command.layer_id, None)
        .map_err(|error| layer_error(error, command.zone_id, Some(command.layer_id)))?;
    let zone = mutation
        .insert_layer(scene_id, command.zone_id, command.layer, Some(index), None)
        .map_err(|error| layer_error(error, command.zone_id, None))?;
    crate::domain::layer::validate_candidate_media_admission(state, &mutation, scene_id).await?;

    finish_layer_mutation(
        state,
        mutation,
        scene_id,
        zone,
        LayerStackChangeKind::Updated,
    )
    .await
}

/// Write control values and drop named bindings on one layer.
///
/// Unguarded by contract: value writes take no revision token, and a
/// patch addressing a replaced layer 404s rather than landing on the
/// newer effect (Spec 78 §1.6).
///
/// # Errors
///
/// [`DomainError::ControlBound`] when a written key keeps a binding the
/// request does not clear, [`DomainError::NotFound`] for an unknown zone
/// or layer.
pub async fn patch_layer_controls(
    state: &AppState,
    command: PatchLayerControls,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    if command.values.is_empty() && command.clear_bindings.is_empty() {
        return Err(DomainError::validation(
            "a control patch must carry values, bindings to clear, or both",
        ));
    }

    let values =
        normalize_against_layer(state, command.zone_id, command.layer_id, command.values).await?;

    let mut mutation = state.begin_scene_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("patching layer controls")?;
    let zone = mutation
        .patch_layer_controls_and_bindings(
            scene_id,
            command.zone_id,
            command.layer_id,
            values,
            &command.clear_bindings,
            None,
        )
        .map_err(|error| layer_error(error, command.zone_id, Some(command.layer_id)))?;

    finish_layer_mutation(
        state,
        mutation,
        scene_id,
        zone,
        LayerStackChangeKind::ControlsPatched,
    )
    .await
}

/// Assign a device's segments to a zone.
///
/// A segment the scene already holds is moved out of whatever zone owns
/// it; a segment the scene has never seen is minted from the connected
/// device through the same auto-layout factory discovery uses, so a
/// hand-assigned output is shaped exactly like a discovered one.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone, an unknown device, or
/// a segment neither the scene nor the device knows;
/// [`DomainError::PreconditionFailed`] for a stale revision.
pub async fn assign_members(
    state: &AppState,
    command: AssignMembers,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let minted = mint_missing_outputs(state, &command.request).await?;

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("assigning zone members")?;

    let scene = active_scene(&mutation)?;
    if !scene.groups.iter().any(|zone| zone.id == command.zone_id) {
        return Err(DomainError::not_found(ResourceKind::Zone, command.zone_id));
    }
    let previous_zones = scene.groups.clone();
    let outputs = resolve_members(&scene, &command.request, minted)?;

    for output in outputs {
        mutation
            .assign_output(scene_id, command.zone_id, output, OutputPlacement::Preserve)
            .map_err(|_| DomainError::not_found(ResourceKind::Zone, command.zone_id))?;
    }

    let zone = zone_in_candidate(&mutation, command.zone_id)?;
    let written = finish_zone_mutation(state, mutation, scene_id, zone).await?;
    crate::api::sync_connectivity(state).await;
    reconcile_member_exclusions(state, scene_id, &previous_zones).await;
    Ok(written)
}

/// Drop one membership out of the zone that holds it.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone or a membership that
/// zone does not hold, [`DomainError::PreconditionFailed`] for a stale
/// revision.
pub async fn unassign_member(
    state: &AppState,
    zone_id: ZoneId,
    member: &ZoneMemberId,
    expected_revision: Option<u64>,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("unassigning a zone member")?;

    let scene = active_scene(&mutation)?;
    let zone = scene
        .groups
        .iter()
        .find(|zone| zone.id == zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
    if !zone.layout.zones.iter().any(|output| output.id == member.0) {
        return Err(DomainError::not_found(ResourceKind::Device, &member.0));
    }
    let previous_zones = scene.groups.clone();

    mutation
        .unassign_output(scene_id, &member.0)
        .map_err(|_| DomainError::not_found(ResourceKind::Device, &member.0))?;

    let zone = zone_in_candidate(&mutation, zone_id)?;
    let written = finish_zone_mutation(state, mutation, scene_id, zone).await?;
    crate::api::sync_connectivity(state).await;
    reconcile_member_exclusions(state, scene_id, &previous_zones).await;
    Ok(written)
}

/// Reposition a zone's members from the compact placement contract.
///
/// The request carries placements only; hardware bindings and LED
/// topology come from what the zone already stores, so a layout write
/// can never rebind a device (Spec 78 §1.2).
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone,
/// [`DomainError::Validation`] when the placements are not exactly the
/// zone's current members, [`DomainError::PreconditionFailed`] for a
/// stale revision.
pub async fn set_zone_layout(
    state: &AppState,
    zone_id: ZoneId,
    placements: Vec<MemberPlacement>,
    expected_revision: Option<u64>,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_scene_revision(&mutation, expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("laying out a zone")?;

    let stored = active_scene(&mutation)?
        .groups
        .iter()
        .find(|zone| zone.id == zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?
        .layout
        .clone();
    let layout = layout_from_placements(&stored, placements)?;

    let zone = mutation
        .set_zone_layout(scene_id, zone_id, layout)
        .map_err(|error| zone_layout_error(error, zone_id))?;

    let written = finish_zone_mutation(state, mutation, scene_id, zone).await?;
    state.zone_layout_previews.clear(scene_id, zone_id).await;
    Ok(written)
}

// ── Shared steps ─────────────────────────────────────────────────────────

async fn finish_zone_mutation(
    state: &AppState,
    mut mutation: SceneMutation,
    scene_id: SceneId,
    zone: Zone,
) -> Result<ZoneWritten, DomainError> {
    mutation.record(zone_changed_event(scene_id, &zone, ZoneChangeKind::Updated));
    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;
    Ok(ZoneWritten {
        zone: zone_resource(&zone),
        revision: commit.revision(),
        commit,
    })
}

async fn finish_layer_mutation(
    state: &AppState,
    mut mutation: SceneMutation,
    scene_id: SceneId,
    zone: Zone,
    kind: LayerStackChangeKind,
) -> Result<ZoneWritten, DomainError> {
    let zone_kind = if kind == LayerStackChangeKind::ControlsPatched {
        ZoneChangeKind::ControlsPatched
    } else {
        ZoneChangeKind::Updated
    };
    mutation.record(zone_changed_event(scene_id, &zone, zone_kind));
    mutation.record(HypercolorEvent::LayerStackChanged {
        scene_id,
        zone_id: zone.id,
        layers_version: zone.layers_version,
        kind,
    });
    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;
    Ok(ZoneWritten {
        zone: zone_resource(&zone),
        revision: commit.revision(),
        commit,
    })
}

fn active_scene(mutation: &SceneMutation) -> Result<Scene, DomainError> {
    mutation
        .scenes()
        .active_scene()
        .cloned()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))
}

fn zone_in_candidate(mutation: &SceneMutation, zone_id: ZoneId) -> Result<Zone, DomainError> {
    mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.groups.iter().find(|zone| zone.id == zone_id).cloned())
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))
}

/// Project a zone-layout refusal onto the canonical error set.
fn zone_layout_error(
    error: hypercolor_core::scene::ZoneMutationError,
    zone_id: ZoneId,
) -> DomainError {
    use hypercolor_core::scene::ZoneMutationError;
    match error {
        ZoneMutationError::SceneMissing => DomainError::not_found(ResourceKind::Scene, "active"),
        ZoneMutationError::GroupMissing => DomainError::not_found(ResourceKind::Zone, zone_id),
        ZoneMutationError::OutputMissing => DomainError::not_found(ResourceKind::Device, "member"),
        ZoneMutationError::SnapshotLocked => {
            DomainError::conflict("Snapshot scene cannot be structurally edited")
        }
        ZoneMutationError::InvalidRole { .. } => {
            DomainError::conflict("Zone role does not support this mutation")
        }
        ZoneMutationError::LayoutOutputMismatch => DomainError::validation(
            "layout placements must name exactly the zone's current members, each once",
        ),
    }
}

/// Project a layer-stack refusal onto the canonical error set.
pub(crate) fn layer_error(
    error: LayerMutationError,
    zone_id: ZoneId,
    layer_id: Option<SceneLayerId>,
) -> DomainError {
    match error {
        LayerMutationError::NoActiveScene | LayerMutationError::SceneMissing => {
            DomainError::not_found(ResourceKind::Scene, "active")
        }
        LayerMutationError::GroupMissing => DomainError::not_found(ResourceKind::Zone, zone_id),
        LayerMutationError::LayerMissing { layer_id } => {
            DomainError::not_found(ResourceKind::Layer, layer_id)
        }
        LayerMutationError::DuplicateLayer { layer_id } => {
            DomainError::validation(format!("Layer already exists: {layer_id}"))
        }
        LayerMutationError::Stale { expected, current } => DomainError::PreconditionFailed {
            resource: ResourceKind::Scene,
            expected,
            current,
        },
        LayerMutationError::InvalidLayer { errors } => DomainError::validation_details(
            "Layer payload is invalid",
            serde_json::json!({ "errors": errors }),
        ),
        LayerMutationError::InvalidIndex { index, len } => DomainError::validation(format!(
            "Layer index {index} is out of range for stack length {len}"
        )),
        LayerMutationError::InvalidOrder => DomainError::validation(
            "order must name every layer in the zone exactly once, bottom to top",
        ),
        LayerMutationError::ControlBound { keys } => {
            let _ = layer_id;
            DomainError::ControlBound { keys }
        }
    }
}

/// Range-check and coerce control values against the effect the target
/// layer runs, so a junk key or an out-of-range value is refused rather
/// than persisted into the scene and handed to the renderer.
///
/// A layer whose effect the registry does not know keeps the caller's
/// values as written: the schema is what validates, and without one
/// there is nothing to validate against.
async fn normalize_against_layer(
    state: &AppState,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    values: HashMap<String, hypercolor_types::effect::ControlValue>,
) -> Result<HashMap<String, hypercolor_types::effect::ControlValue>, DomainError> {
    let effect_id = {
        let manager = state.scene_manager.read().await;
        let scene = manager
            .active_scene()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let zone = scene
            .groups
            .iter()
            .find(|zone| zone.id == zone_id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
        zone.effective_layers()
            .into_iter()
            .find(|layer| layer.id == layer_id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Layer, layer_id))
            .map(|layer| match layer.source {
                hypercolor_types::layer::LayerSource::Effect { effect_id, .. } => Some(effect_id),
                _ => None,
            })?
    };

    let Some(effect_id) = effect_id else {
        return Ok(values);
    };
    let metadata = {
        let registry = state.effect_registry.read().await;
        registry.get(&effect_id).map(|entry| entry.metadata.clone())
    };
    let Some(metadata) = metadata else {
        return Ok(values);
    };

    let (normalized, rejected) = crate::api::effects::normalize_control_values(&metadata, &values);
    if !rejected.is_empty() {
        return Err(DomainError::validation_details(
            "one or more control values were rejected",
            serde_json::json!({ "rejected": rejected }),
        ));
    }
    Ok(normalized)
}

// ── Member resolution ────────────────────────────────────────────────────

/// Resolve the requested segments against the scene, falling back to
/// the outputs minted from the connected device.
fn resolve_members(
    scene: &Scene,
    request: &AssignMembersRequest,
    minted: Vec<Output>,
) -> Result<Vec<Output>, DomainError> {
    let held: Vec<&Output> = scene
        .groups
        .iter()
        .flat_map(|zone| zone.layout.zones.iter())
        .filter(|output| output.device_id == request.device_id)
        .collect();

    if request.segments.is_empty() {
        let candidates: Vec<Output> = if held.is_empty() {
            minted
        } else {
            held.into_iter().cloned().collect()
        };
        return match candidates.len() {
            0 => Err(DomainError::not_found(
                ResourceKind::Device,
                &request.device_id,
            )),
            1 => Ok(candidates),
            // Omitting segments means "the whole device", which is only
            // unambiguous on single-segment hardware.
            _ => Err(DomainError::validation_details(
                "this device has more than one segment; name the segments to assign",
                serde_json::json!({
                    "segments": candidates
                        .iter()
                        .filter_map(|output| output.zone_name.clone())
                        .collect::<Vec<_>>(),
                }),
            )),
        };
    }

    request
        .segments
        .iter()
        .map(|segment| {
            held.iter()
                .find(|output| output.zone_name.as_deref() == Some(segment.as_str()))
                .map(|output| (*output).clone())
                .or_else(|| {
                    minted
                        .iter()
                        .find(|output| output.zone_name.as_deref() == Some(segment.as_str()))
                        .cloned()
                })
                .ok_or_else(|| {
                    DomainError::not_found(
                        ResourceKind::Device,
                        format!("{}:{segment}", request.device_id),
                    )
                })
        })
        .collect()
}

/// Build the outputs a connected device would contribute, so a segment
/// the scene has never held can still be assigned.
///
/// A device that is not connected mints nothing; the caller then
/// reports whichever segment it could not resolve.
async fn mint_missing_outputs(
    state: &AppState,
    request: &AssignMembersRequest,
) -> Result<Vec<Output>, DomainError> {
    let tracked = state.device_registry.list().await;
    for device in &tracked {
        let layout_device_id =
            crate::api::devices::resolved_layout_device_id(state, &device.info).await;
        if layout_device_id != request.device_id {
            continue;
        }
        let mut scratch = SpatialLayout {
            id: format!("mint-{layout_device_id}"),
            name: device.info.name.clone(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        };
        let _minted = crate::discovery::auto_layout::append_auto_layout_zones_for_device(
            &mut scratch,
            &layout_device_id,
            &device.info,
        );
        return Ok(scratch.zones);
    }
    Ok(Vec::new())
}

/// Rebuild a zone's stored layout from the compact placement contract.
///
/// The placements must name exactly the zone's current members: adds
/// and drops route through the member endpoints, which is the same
/// fence the engine enforces one level down.
fn layout_from_placements(
    stored: &SpatialLayout,
    placements: Vec<MemberPlacement>,
) -> Result<SpatialLayout, DomainError> {
    let mut layout = stored.clone();
    let mut ordered = Vec::with_capacity(placements.len());

    for placement in placements {
        let Some(mut output) = stored
            .zones
            .iter()
            .find(|output| output.id == placement.member.0)
            .cloned()
        else {
            return Err(DomainError::validation_details(
                "layout placements must name exactly the zone's current members",
                serde_json::json!({ "unknown_member": placement.member.0 }),
            ));
        };
        if placement.topology != output.topology {
            return Err(DomainError::validation_details(
                "layout placements cannot change a member's LED topology",
                serde_json::json!({ "member": placement.member.0 }),
            ));
        }
        output.position = placement.position;
        output.size = placement.size;
        output.rotation = placement.rotation;
        output.scale = placement.scale;
        output.orientation = placement.orientation;
        ordered.push(output);
    }

    if ordered.len() != stored.zones.len() {
        return Err(DomainError::validation_details(
            "layout placements must name exactly the zone's current members",
            serde_json::json!({
                "expected": stored.zones.len(),
                "received": ordered.len(),
            }),
        ));
    }

    layout.zones = ordered;
    Ok(layout)
}

/// Carry the discovery auto-sync exclusions across a membership edit,
/// the same way the structural zone services do.
async fn reconcile_member_exclusions(state: &AppState, scene_id: SceneId, previous: &[Zone]) {
    let updated = {
        let manager = state.scene_manager.read().await;
        manager
            .get(&scene_id)
            .map(|scene| scene.groups.clone())
            .unwrap_or_default()
    };
    crate::domain::zone::reconcile_zone_auto_exclusions(state, scene_id, previous, &updated).await;
}
