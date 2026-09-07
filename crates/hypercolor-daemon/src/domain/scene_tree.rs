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
//! per-subresource counters the engine keeps (`zones_revision`,
//! `layers_version`, `controls_version`) stay internal bookkeeping and
//! never reach this surface.

use std::collections::HashMap;

use hypercolor_types::api::scene::{
    AssignMembersRequest, MemberPlacement, SceneDocument, ZoneLayoutResource, ZoneMember,
    ZoneMemberId, ZoneResource,
};
use hypercolor_types::control::ControlValue;
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Scene, SceneId, UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{Output, SpatialLayout};

use hypercolor_core::scene::{LayerMutationError, OutputPlacement};

use crate::domain::commit::SceneCommit;
use crate::domain::context::SceneContext;
use crate::domain::effect::EffectContext;
use crate::domain::layout::LayoutContext;
use crate::domain::output::OutputContext;
use crate::domain::scene::SceneMutation;
use crate::domain::{DomainError, DomainErrorDetails, MutationContext, ResourceKind};

/// Live scene-tree authority shared by REST and MCP adapters.
#[derive(Clone)]
pub struct SceneTreeContext {
    scene: SceneContext,
    effects: EffectContext,
    layout: LayoutContext,
    output: OutputContext,
}

impl SceneTreeContext {
    pub(crate) fn new(
        scene: SceneContext,
        effects: EffectContext,
        layout: LayoutContext,
        output: OutputContext,
    ) -> Self {
        Self {
            scene,
            effects,
            layout,
            output,
        }
    }
}

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
pub async fn read_document(ctx: &SceneTreeContext) -> Result<SceneDocument, DomainError> {
    let manager = ctx.scene.snapshot().await;
    let revision = ctx.scene.revision();
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
pub async fn read_zone(
    ctx: &SceneTreeContext,
    zone_id: ZoneId,
) -> Result<(ZoneResource, u64), DomainError> {
    let manager = ctx.scene.snapshot().await;
    let revision = ctx.scene.revision();
    let scene = manager
        .active_scene()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
    scene
        .zones
        .iter()
        .find(|zone| zone.id == zone_id)
        .map(|zone| (zone_resource(zone), revision))
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))
}

/// Project a scene plus the current commit generation onto the wire
/// document.
#[must_use]
pub fn scene_document(scene: &Scene, revision: u64) -> SceneDocument {
    SceneDocument {
        id: scene.id,
        name: scene.name.clone(),
        description: scene.description.clone(),
        kind: scene.kind,
        is_default: scene.id.is_default(),
        unassigned_behavior: scene.unassigned_behavior.clone(),
        layout_id: scene.layout_id.clone(),
        activation_brightness: scene.activation_brightness,
        transition: scene.transition.clone(),
        priority: scene.priority,
        enabled: scene.enabled,
        metadata: scene.metadata.clone(),
        mutation_mode: scene.mutation_mode,
        revision,
        zones: scene.zones.iter().map(zone_resource).collect(),
    }
}

/// Project one zone onto the wire resource.
///
/// The layer stack is the authored stack read by the renderer, so a
/// client patches the id the daemon actually addresses.
#[must_use]
pub fn zone_resource(zone: &Zone) -> ZoneResource {
    ZoneResource {
        id: zone.id,
        name: zone.name.clone(),
        description: zone.description.clone(),
        role: zone.role,
        enabled: zone.enabled,
        brightness: zone.brightness,
        color: zone.color.clone(),
        display_target: zone.display_target.clone(),
        members: zone.layout.zones.iter().map(zone_member).collect(),
        layout: zone_layout_resource(zone),
        layers: zone.layers.clone(),
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

/// Empty layer stacks in one non-display zone or every non-display zone.
#[derive(Debug, Clone, Default)]
pub struct ClearScene {
    /// Which non-display zone to empty; `None` empties every non-display zone.
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
    pub values: HashMap<String, ControlValue>,
    /// Bindings to remove in the same commit.
    pub clear_bindings: Vec<String>,
    /// The revision an adapter resolved names against, when it did so.
    pub expected_revision: Option<u64>,
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
    ctx: &SceneTreeContext,
    command: PatchScene,
) -> Result<TreeWritten, DomainError> {
    if let Some(name) = &command.name
        && name.trim().is_empty()
    {
        return Err(DomainError::validation_field(
            "name",
            "scene name must not be empty",
        ));
    }

    let mut mutation = ctx.scene.begin_mutation().await;
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
    }

    if let Some(behavior) = command.unassigned_behavior {
        mutation
            .set_unassigned_behavior(scene_id, behavior)
            .map_err(|_| DomainError::not_found(ResourceKind::Scene, scene_id))?;
    }

    let commit = ctx.scene.commit(mutation).await?;
    ctx.scene.save_runtime_session().await;
    Ok(TreeWritten {
        document: read_document(ctx).await?,
        commit,
    })
}

/// Empty one non-display zone's stack, or every non-display zone's.
///
/// Clearing every non-display zone also quiesces output. Clearing one zone leaves the
/// rest rendering and therefore leaves output alone.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown zone,
/// [`DomainError::PreconditionFailed`] for a stale revision, and the
/// commit path's own refusals.
pub async fn clear_scene(
    ctx: &SceneTreeContext,
    command: ClearScene,
) -> Result<TreeWritten, DomainError> {
    let effect_refs = if command.zone.is_none() {
        super::effect::effect_ref_index(&ctx.effects).await
    } else {
        HashMap::new()
    };
    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("clearing the scene")?;

    let targets: Vec<ZoneId> = match command.zone {
        Some(zone_id) => {
            let scene = active_scene(&mutation)?;
            let zone = scene
                .zones
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
            .zones
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .map(|zone| zone.id)
            .collect(),
    };

    let stopped_effects = if command.zone.is_none() {
        active_scene(&mutation)?
            .zones
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .filter_map(|zone| {
                zone.effect_ids()
                    .next()
                    .and_then(|effect_id| effect_refs.get(&effect_id).cloned())
                    .map(|effect| (zone.id, effect))
            })
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };

    for zone_id in targets {
        mutation.retire_zone_preview(scene_id, zone_id);
        mutation.clear_zone_effect(
            zone_id,
            stopped_effects.get(&zone_id).cloned(),
            hypercolor_types::event::EffectStopReason::Stopped,
        );
    }

    let commit = ctx.scene.commit(mutation).await?;
    if command.zone.is_none() {
        ctx.output.quiesce_after_effect_stop().await;
    }
    ctx.scene.save_runtime_session().await;

    Ok(TreeWritten {
        document: read_document(ctx).await?,
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
    ctx: &SceneTreeContext,
    mut command: ReplaceLayer,
) -> Result<ZoneWritten, DomainError> {
    let _effect_admission = ctx
        .effects
        .admit_layer_sources(std::iter::once(&mut command.layer.source))
        .await?;
    let media_admission = ctx.scene.media_admission_for_layer(&command.layer).await;
    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("replacing a layer")?;
    ensure_live_layer_stack_mutable(&mutation, command.zone_id)?;

    let index = active_scene(&mutation)?
        .zones
        .iter()
        .find(|zone| zone.id == command.zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, command.zone_id))?
        .layers
        .iter()
        .position(|layer| layer.id == command.layer_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Layer, command.layer_id))?;

    let zone = mutation
        .replace_layer(
            scene_id,
            command.zone_id,
            command.layer_id,
            command.layer,
            index,
        )
        .map_err(|error| layer_error(error, command.zone_id, Some(command.layer_id)))?;
    if let Some(media_admission) = media_admission {
        media_admission.validate(&active_scene(&mutation)?)?;
    }

    finish_layer_mutation(ctx, mutation, zone).await
}

/// Write control values and drop named bindings on one layer.
///
/// The REST contract stays unguarded: a patch addressing a replaced layer
/// 404s rather than landing on the newer effect. Name-resolving adapters may
/// carry the revision of their selector snapshot to keep resolution and
/// mutation atomic (Spec 78 §1.6).
///
/// # Errors
///
/// [`DomainError::ControlBound`] when a written key keeps a binding the
/// request does not clear, [`DomainError::NotFound`] for an unknown zone
/// or layer.
pub async fn patch_layer_controls(
    ctx: &SceneTreeContext,
    command: PatchLayerControls,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    if command.values.is_empty() && command.clear_bindings.is_empty() {
        return Err(DomainError::validation(
            "a control patch must carry values, bindings to clear, or both",
        ));
    }

    loop {
        let mut mutation = ctx.scene.begin_mutation().await;
        check_scene_revision(&mutation, command.expected_revision)?;
        let scene_id = mutation.active_scene_for_runtime_mutation("patching layer controls")?;
        ensure_live_layer_stack_mutable(&mutation, command.zone_id)?;
        let normalized = normalize_against_layer(
            ctx,
            &mutation,
            command.zone_id,
            command.layer_id,
            command.values.clone(),
        )
        .await?;
        let zone = mutation
            .patch_layer_controls_and_bindings(
                scene_id,
                command.zone_id,
                command.layer_id,
                normalized.values,
                &command.clear_bindings,
                None,
                meta.trigger.clone(),
                &normalized.previous,
            )
            .map_err(|error| layer_error(error, command.zone_id, Some(command.layer_id)))?;

        match finish_layer_mutation(ctx, mutation, zone).await {
            Err(error) if scene_commit_was_superseded(&error) => {}
            outcome => return outcome,
        }
    }
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
    ctx: &SceneTreeContext,
    command: AssignMembers,
) -> Result<ZoneWritten, DomainError> {
    let minted = mint_missing_outputs(ctx, &command.request).await?;

    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("assigning zone members")?;
    ensure_live_zone_mutable(&mutation, command.zone_id)?;

    let scene = active_scene(&mutation)?;
    if !scene.zones.iter().any(|zone| zone.id == command.zone_id) {
        return Err(DomainError::not_found(ResourceKind::Zone, command.zone_id));
    }
    let previous_zones = scene.zones.clone();
    let outputs = resolve_members(&scene, &command.request, minted)?;

    for output in outputs {
        mutation
            .assign_output(scene_id, command.zone_id, output, OutputPlacement::Preserve)
            .map_err(|_| DomainError::not_found(ResourceKind::Zone, command.zone_id))?;
    }

    let zone = zone_in_candidate(&mutation, command.zone_id)?;
    let written = finish_zone_mutation(ctx, mutation, scene_id, zone).await?;
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;
    reconcile_member_exclusions(ctx, scene_id, &previous_zones).await;
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
    ctx: &SceneTreeContext,
    zone_id: ZoneId,
    member: &ZoneMemberId,
    expected_revision: Option<u64>,
) -> Result<ZoneWritten, DomainError> {
    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("unassigning a zone member")?;
    ensure_live_zone_mutable(&mutation, zone_id)?;

    let scene = active_scene(&mutation)?;
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id == zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
    if !zone.layout.zones.iter().any(|output| output.id == member.0) {
        return Err(DomainError::not_found(ResourceKind::Device, &member.0));
    }
    let previous_zones = scene.zones.clone();

    mutation
        .unassign_output(scene_id, &member.0)
        .map_err(|_| DomainError::not_found(ResourceKind::Device, &member.0))?;

    let zone = zone_in_candidate(&mutation, zone_id)?;
    let written = finish_zone_mutation(ctx, mutation, scene_id, zone).await?;
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;
    reconcile_member_exclusions(ctx, scene_id, &previous_zones).await;
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
    ctx: &SceneTreeContext,
    zone_id: ZoneId,
    placements: Vec<MemberPlacement>,
    expected_revision: Option<u64>,
) -> Result<ZoneWritten, DomainError> {
    let mut mutation = ctx.scene.begin_mutation().await;
    check_scene_revision(&mutation, expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("laying out a zone")?;
    ensure_live_zone_mutable(&mutation, zone_id)?;

    let stored = active_scene(&mutation)?
        .zones
        .iter()
        .find(|zone| zone.id == zone_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?
        .layout
        .clone();
    let layout = layout_from_placements(&stored, placements)?;
    let zone = mutation
        .set_zone_layout(scene_id, zone_id, layout)
        .map_err(|error| zone_layout_error(error, zone_id))?;
    mutation.retire_zone_preview(scene_id, zone_id);

    let written = finish_zone_mutation(ctx, mutation, scene_id, zone).await?;
    Ok(written)
}

// ── Shared steps ─────────────────────────────────────────────────────────

async fn finish_zone_mutation(
    ctx: &SceneTreeContext,
    mutation: SceneMutation,
    _scene_id: SceneId,
    zone: Zone,
) -> Result<ZoneWritten, DomainError> {
    let commit = ctx.scene.commit(mutation).await?;
    ctx.scene.save_runtime_session().await;
    Ok(ZoneWritten {
        zone: zone_resource(&zone),
        revision: commit.revision(),
        commit,
    })
}

async fn finish_layer_mutation(
    ctx: &SceneTreeContext,
    mutation: SceneMutation,
    zone: Zone,
) -> Result<ZoneWritten, DomainError> {
    let commit = ctx.scene.commit(mutation).await?;
    ctx.scene.save_runtime_session().await;
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

pub(crate) fn ensure_live_zone_mutable(
    mutation: &SceneMutation,
    zone_id: ZoneId,
) -> Result<(), DomainError> {
    let zone = mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
    if zone.role == ZoneRole::Display {
        return Err(DomainError::validation(
            "display zones are managed through the display face API",
        ));
    }
    Ok(())
}

/// Ensure a live scene owns the addressed layer stack.
///
/// Display-zone structure stays exclusive to the display domain, but its
/// authored layers use the same live-tree contract as every other surface.
pub(crate) fn ensure_live_layer_stack_mutable(
    mutation: &SceneMutation,
    zone_id: ZoneId,
) -> Result<(), DomainError> {
    mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
        .map(|_| ())
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))
}

fn zone_in_candidate(mutation: &SceneMutation, zone_id: ZoneId) -> Result<Zone, DomainError> {
    mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id).cloned())
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
        ZoneMutationError::ZoneMissing => DomainError::not_found(ResourceKind::Zone, zone_id),
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
        LayerMutationError::SceneMissing => DomainError::not_found(ResourceKind::Scene, "active"),
        LayerMutationError::ZoneMissing => DomainError::not_found(ResourceKind::Zone, zone_id),
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
            DomainErrorDetails::Errors { errors },
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
struct NormalizedLayerControls {
    values: HashMap<String, ControlValue>,
    previous: HashMap<String, ControlValue>,
    _admission: Option<super::effect::EffectMutationAdmission>,
}

async fn normalize_against_layer(
    ctx: &SceneTreeContext,
    mutation: &SceneMutation,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    values: HashMap<String, ControlValue>,
) -> Result<NormalizedLayerControls, DomainError> {
    let effect = {
        let scene = mutation
            .scenes()
            .active_scene()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let zone = scene
            .zones
            .iter()
            .find(|zone| zone.id == zone_id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))?;
        zone.layers
            .iter()
            .find(|layer| layer.id == layer_id)
            .cloned()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Layer, layer_id))
            .map(|layer| match layer.source {
                LayerSource::Effect {
                    effect_id,
                    controls,
                    ..
                } => Some((effect_id, controls)),
                _ => None,
            })?
    };

    let Some((effect_id, layer_controls)) = effect else {
        return Ok(NormalizedLayerControls {
            values,
            previous: HashMap::new(),
            _admission: None,
        });
    };
    let admitted = ctx
        .effects
        .admit_current_controls(effect_id, &values)
        .await?;
    let (metadata, values, admission) = admitted.into_parts();
    let mut previous = super::effect::default_control_values(&metadata);
    previous.extend(layer_controls);

    Ok(NormalizedLayerControls {
        values,
        previous,
        _admission: Some(admission),
    })
}

fn scene_commit_was_superseded(error: &DomainError) -> bool {
    matches!(
        error,
        DomainError::Conflict {
            details: Some(DomainErrorDetails::SceneCommitSuperseded { .. }),
            ..
        }
    )
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
        .zones
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
                DomainErrorDetails::Segments {
                    segments: candidates
                        .iter()
                        .filter_map(|output| output.zone_name.clone())
                        .collect(),
                },
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
    ctx: &SceneTreeContext,
    request: &AssignMembersRequest,
) -> Result<Vec<Output>, DomainError> {
    Ok(ctx
        .layout
        .layout_outputs_for(ctx.scene.layout_runtime(), &request.device_id)
        .await)
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
                DomainErrorDetails::UnknownMember {
                    unknown_member: placement.member.0,
                },
            ));
        };
        if placement.topology != output.topology {
            return Err(DomainError::validation_details(
                "layout placements cannot change a member's LED topology",
                DomainErrorDetails::Member {
                    member: placement.member.0,
                },
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
            DomainErrorDetails::MemberCount {
                expected: stored.zones.len(),
                received: ordered.len(),
            },
        ));
    }

    layout.zones = ordered;
    Ok(layout)
}

/// Carry the discovery auto-sync exclusions across a membership edit,
/// the same way the structural zone services do.
async fn reconcile_member_exclusions(ctx: &SceneTreeContext, scene_id: SceneId, previous: &[Zone]) {
    let updated = {
        let manager = ctx.scene.snapshot().await;
        manager
            .get(&scene_id)
            .map(|scene| scene.zones.clone())
            .unwrap_or_default()
    };
    ctx.layout
        .reconcile_zone_auto_exclusions(scene_id, previous, &updated)
        .await;
}
