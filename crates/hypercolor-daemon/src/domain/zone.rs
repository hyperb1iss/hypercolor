//! Zone domain services (Spec 76 §2.2, §2.3).
//!
//! A scene's zones are the partition its outputs live in, so every
//! mutation here is structural: it moves persisted scene content, bumps
//! the scene's `groups_revision`, and changes which devices are worth
//! connecting. All seven transactions share the same shape — resolve on
//! a candidate, check the caller's `groups_revision` precondition
//! against that candidate, mutate, commit, then reconcile the
//! layout-exclusion bookkeeping and connectivity the new partition
//! implies.
//!
//! Identity resolution stays with the adapter, matching
//! [`apply_effect`](super::effect::apply_effect): a REST path segment
//! naming a scene by id or name is a transport concern, and the service
//! takes the [`SceneId`] it found.

use hypercolor_core::scene::{OutputPlacement, ZoneMetaPatch, ZoneMutationError};
use hypercolor_types::event::{HypercolorEvent, SceneSettingsChangeKind, ZoneChangeKind};
use hypercolor_types::scene::{SceneId, UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{Output, SpatialLayout};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{SceneMutation, commit_scene, zone_changed_event};
use crate::domain::{DomainError, MutationContext, ResourceKind};
use crate::layout_auto_exclusions;

// ── Commands ─────────────────────────────────────────────────────────────

/// Add a custom zone to a scene.
#[derive(Debug, Clone)]
pub struct CreateZone {
    /// Which scene gains the zone.
    pub scene_id: SceneId,
    /// The zone's name. Must not be blank.
    pub name: String,
    /// Optional swatch for the Studio zone tree.
    pub color: Option<String>,
    /// Canvas dimensions the new zone's empty layout inherits.
    pub fallback_canvas: (u32, u32),
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Patch a zone's presentation metadata.
#[derive(Debug, Clone)]
pub struct UpdateZone {
    /// Which scene owns the zone.
    pub scene_id: SceneId,
    /// Which zone to patch.
    pub zone_id: ZoneId,
    /// The fields to change; `None` fields keep their current values.
    pub patch: ZoneMetaPatch,
    /// The `groups_revision` the caller last saw, when it sent one.
    ///
    /// Only a structural patch (promoting a zone to primary) checks it —
    /// renaming or recoloring a zone races with nothing.
    pub expected_revision: Option<u64>,
}

/// Remove a custom zone from a scene.
#[derive(Debug, Clone)]
pub struct DeleteZone {
    /// Which scene owns the zone.
    pub scene_id: SceneId,
    /// Which zone to remove.
    pub zone_id: ZoneId,
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Move outputs into a zone.
#[derive(Debug, Clone)]
pub struct AssignOutputs {
    /// Which scene owns the zone.
    pub scene_id: SceneId,
    /// Which zone receives the outputs.
    pub zone_id: ZoneId,
    /// The outputs to move, already resolved against the scene.
    pub outputs: Vec<Output>,
    /// Whether moved outputs keep their coordinates or get re-gridded.
    pub placement: OutputPlacement,
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Drop one output out of whatever zone holds it.
#[derive(Debug, Clone)]
pub struct UnassignOutput {
    /// Which scene owns the zone.
    pub scene_id: SceneId,
    /// Which zone the caller addressed.
    pub zone_id: ZoneId,
    /// Which output to drop.
    pub output_id: String,
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Reposition a zone's outputs without changing which it owns.
#[derive(Debug, Clone)]
pub struct SetZoneLayout {
    /// Which scene owns the zone.
    pub scene_id: SceneId,
    /// Which zone to reposition.
    pub zone_id: ZoneId,
    /// The new placement. Must carry exactly the zone's current outputs.
    pub layout: SpatialLayout,
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Choose what a scene does with outputs no zone claims.
#[derive(Debug, Clone)]
pub struct SetUnassignedBehavior {
    /// Which scene to configure.
    pub scene_id: SceneId,
    /// What unclaimed outputs should render.
    pub behavior: UnassignedBehavior,
    /// The `groups_revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

// ── Outcomes ─────────────────────────────────────────────────────────────

/// The outcome of a mutation that reports one zone.
#[derive(Debug)]
pub struct ZoneWritten {
    /// The zone as it now stands.
    pub zone: Zone,
    /// The scene's `groups_revision` after the mutation.
    pub groups_revision: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of a mutation that reports the whole partition.
#[derive(Debug)]
pub struct ZonesWritten {
    /// Every zone in the scene after the mutation.
    pub zones: Vec<Zone>,
    /// The zone the caller addressed.
    pub target_zone: Zone,
    /// The scene's `groups_revision` after the mutation.
    pub groups_revision: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of removing a zone.
#[derive(Debug)]
pub struct ZoneRemoved {
    /// The zone that was removed.
    pub zone: Zone,
    /// The scene's `groups_revision` after the mutation.
    pub groups_revision: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of changing a scene's unassigned-output behavior.
#[derive(Debug)]
pub struct UnassignedBehaviorWritten {
    /// The behavior that is now in force.
    pub behavior: UnassignedBehavior,
    /// The scene's `groups_revision` after the mutation.
    pub groups_revision: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

// ── Services ─────────────────────────────────────────────────────────────

/// Add a custom zone to a scene.
///
/// # Errors
///
/// [`DomainError::Validation`] for a blank name,
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Conflict`] for a snapshot-locked scene, and
/// [`DomainError::PreconditionFailed`] for a stale `groups_revision` or
/// a concurrent scene mutation.
pub async fn create_zone(
    state: &AppState,
    command: CreateZone,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    if command.name.trim().is_empty() {
        return Err(DomainError::validation_field(
            "name",
            "zone name must not be empty",
        ));
    }

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;

    let zone_id = mutation
        .create_zone(
            command.scene_id,
            command.name,
            command.color,
            command.fallback_canvas,
        )
        .map_err(|error| zone_error(error, command.scene_id, None, None))?;
    let zone = zone_in_scene(&mutation, command.scene_id, zone_id)?;
    let groups_revision = groups_revision(&mutation, command.scene_id);
    mutation.record(zone_changed_event(
        command.scene_id,
        &zone,
        ZoneChangeKind::Created,
    ));

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;

    Ok(ZoneWritten {
        zone,
        groups_revision,
        commit,
    })
}

/// Patch a zone's presentation metadata.
///
/// # Errors
///
/// As [`create_zone`], plus [`DomainError::NotFound`] for an unknown
/// zone.
pub async fn update_zone(
    state: &AppState,
    command: UpdateZone,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let structural = command.patch.make_primary == Some(true);
    let mut mutation = state.begin_scene_mutation().await;
    if structural {
        check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;
    }

    let zone = mutation
        .update_zone_meta(command.scene_id, command.zone_id, command.patch)
        .map_err(|error| zone_error(error, command.scene_id, Some(command.zone_id), None))?;
    let groups_revision = groups_revision(&mutation, command.scene_id);
    mutation.record(zone_changed_event(
        command.scene_id,
        &zone,
        ZoneChangeKind::Updated,
    ));

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;

    Ok(ZoneWritten {
        zone,
        groups_revision,
        commit,
    })
}

/// Remove a custom zone from a scene, dropping the layout exclusions it
/// owned.
///
/// # Errors
///
/// As [`update_zone`], plus [`DomainError::Conflict`] when the zone's
/// role forbids deletion through this path.
pub async fn delete_zone(
    state: &AppState,
    command: DeleteZone,
    meta: MutationContext,
) -> Result<ZoneRemoved, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;

    let zone = zone_in_scene(&mutation, command.scene_id, command.zone_id)?;
    mutation
        .delete_zone(command.scene_id, command.zone_id)
        .map_err(|error| zone_error(error, command.scene_id, Some(command.zone_id), None))?;
    let groups_revision = groups_revision(&mutation, command.scene_id);
    mutation.record(zone_changed_event(
        command.scene_id,
        &zone,
        ZoneChangeKind::Removed,
    ));

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;
    remove_zone_auto_exclusions(state, command.scene_id, command.zone_id).await;

    Ok(ZoneRemoved {
        zone,
        groups_revision,
        commit,
    })
}

/// Move outputs into a zone.
///
/// # Errors
///
/// As [`update_zone`], plus [`DomainError::NotFound`] for an output no
/// zone in the scene owns.
pub async fn assign_outputs(
    state: &AppState,
    command: AssignOutputs,
    meta: MutationContext,
) -> Result<ZonesWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;
    let previous_zones = scene_zones(&mutation, command.scene_id)?;
    zone_in_scene(&mutation, command.scene_id, command.zone_id)?;

    for output in command.outputs {
        mutation
            .assign_output(command.scene_id, command.zone_id, output, command.placement)
            .map_err(|error| zone_error(error, command.scene_id, Some(command.zone_id), None))?;
    }

    finish_partition_mutation(
        state,
        mutation,
        command.scene_id,
        command.zone_id,
        &previous_zones,
    )
    .await
}

/// Drop one output out of whatever zone holds it.
///
/// # Errors
///
/// As [`assign_outputs`].
pub async fn unassign_output(
    state: &AppState,
    command: UnassignOutput,
    meta: MutationContext,
) -> Result<ZonesWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;
    let previous_zones = scene_zones(&mutation, command.scene_id)?;
    let target = zone_in_scene(&mutation, command.scene_id, command.zone_id)?;
    if !target
        .layout
        .zones
        .iter()
        .any(|output| output.id == command.output_id)
    {
        return Err(DomainError::not_found(
            ResourceKind::Device,
            &command.output_id,
        ));
    }

    mutation
        .unassign_output(command.scene_id, &command.output_id)
        .map_err(|error| {
            zone_error(
                error,
                command.scene_id,
                Some(command.zone_id),
                Some(&command.output_id),
            )
        })?;

    finish_partition_mutation(
        state,
        mutation,
        command.scene_id,
        command.zone_id,
        &previous_zones,
    )
    .await
}

/// Reposition a zone's outputs without changing which it owns.
///
/// # Errors
///
/// As [`update_zone`], plus [`DomainError::Validation`] when the layout
/// does not carry exactly the zone's current outputs.
pub async fn set_zone_layout(
    state: &AppState,
    command: SetZoneLayout,
    meta: MutationContext,
) -> Result<ZoneWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;

    let zone = mutation
        .set_zone_layout(command.scene_id, command.zone_id, command.layout)
        .map_err(|error| zone_error(error, command.scene_id, Some(command.zone_id), None))?;
    let groups_revision = groups_revision(&mutation, command.scene_id);
    mutation.record(zone_changed_event(
        command.scene_id,
        &zone,
        ZoneChangeKind::Updated,
    ));

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;

    Ok(ZoneWritten {
        zone,
        groups_revision,
        commit,
    })
}

/// Choose what a scene does with outputs no zone claims.
///
/// # Errors
///
/// As [`update_zone`].
pub async fn set_unassigned_behavior(
    state: &AppState,
    command: SetUnassignedBehavior,
    meta: MutationContext,
) -> Result<UnassignedBehaviorWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    check_groups_revision(&mutation, command.scene_id, command.expected_revision)?;

    let behavior = mutation
        .set_unassigned_behavior(command.scene_id, command.behavior)
        .map_err(|error| zone_error(error, command.scene_id, None, None))?;
    let groups_revision = groups_revision(&mutation, command.scene_id);
    mutation.record(HypercolorEvent::SceneSettingsChanged {
        scene_id: command.scene_id,
        groups_revision,
        kind: SceneSettingsChangeKind::UnassignedBehavior,
    });

    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;

    Ok(UnassignedBehaviorWritten {
        behavior,
        groups_revision,
        commit,
    })
}

// ── Shared steps ─────────────────────────────────────────────────────────

/// Commit a mutation that rewrote which outputs sit in which zone, then
/// reconcile the exclusions and connectivity the new partition implies.
async fn finish_partition_mutation(
    state: &AppState,
    mut mutation: SceneMutation,
    scene_id: SceneId,
    zone_id: ZoneId,
    previous_zones: &[Zone],
) -> Result<ZonesWritten, DomainError> {
    let zones = scene_zones(&mutation, scene_id)?;
    let target_zone = zone_in_scene(&mutation, scene_id, zone_id)?;
    let groups_revision = groups_revision(&mutation, scene_id);
    mutation.record(zone_changed_event(
        scene_id,
        &target_zone,
        ZoneChangeKind::Updated,
    ));

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;
    update_zone_auto_exclusions(state, scene_id, previous_zones, &zones).await;

    Ok(ZonesWritten {
        zones,
        target_zone,
        groups_revision,
        commit,
    })
}

/// Everything a structural zone change implies once it is committed:
/// the session snapshot records the new partition, and a device that
/// just entered or left the active scene reconnects or releases.
async fn settle_zone_mutation(state: &AppState) {
    crate::api::save_runtime_session_snapshot(state).await;
    crate::api::sync_connectivity(state).await;
}

/// Refuse a candidate the caller's `groups_revision` no longer
/// describes.
///
/// A scene the candidate does not hold has no revision to compare, and
/// the mutation that follows reports the missing scene with its own
/// error, so the precondition passes through.
fn check_groups_revision(
    mutation: &SceneMutation,
    scene_id: SceneId,
    expected: Option<u64>,
) -> Result<(), DomainError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(current) = mutation
        .scenes()
        .get(&scene_id)
        .map(|scene| scene.groups_revision)
    else {
        return Ok(());
    };
    if expected == current {
        return Ok(());
    }
    Err(DomainError::PreconditionFailed {
        resource: ResourceKind::Zone,
        expected,
        current,
    })
}

fn groups_revision(mutation: &SceneMutation, scene_id: SceneId) -> u64 {
    mutation
        .scenes()
        .get(&scene_id)
        .map_or(0, |scene| scene.groups_revision)
}

fn scene_zones(mutation: &SceneMutation, scene_id: SceneId) -> Result<Vec<Zone>, DomainError> {
    mutation
        .scenes()
        .get(&scene_id)
        .map(|scene| scene.groups.clone())
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, scene_id))
}

fn zone_in_scene(
    mutation: &SceneMutation,
    scene_id: SceneId,
    zone_id: ZoneId,
) -> Result<Zone, DomainError> {
    mutation
        .scenes()
        .get(&scene_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, scene_id))?
        .groups
        .iter()
        .find(|zone| zone.id == zone_id)
        .cloned()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, zone_id))
}

/// Project a structural mutation refusal onto the domain error set.
fn zone_error(
    error: ZoneMutationError,
    scene_id: SceneId,
    zone_id: Option<ZoneId>,
    output_id: Option<&str>,
) -> DomainError {
    match error {
        ZoneMutationError::SceneMissing => DomainError::not_found(ResourceKind::Scene, scene_id),
        ZoneMutationError::GroupMissing => DomainError::not_found(
            ResourceKind::Zone,
            zone_id.map_or_else(|| "unknown".to_owned(), |id| id.to_string()),
        ),
        ZoneMutationError::OutputMissing => {
            DomainError::not_found(ResourceKind::Device, output_id.unwrap_or("unknown"))
        }
        ZoneMutationError::SnapshotLocked => {
            DomainError::conflict("Snapshot scene cannot be structurally edited")
        }
        ZoneMutationError::InvalidRole {
            role: ZoneRole::Primary,
        } => {
            DomainError::conflict("Primary zone cannot be deleted through the custom zone endpoint")
        }
        ZoneMutationError::InvalidRole {
            role: ZoneRole::Display,
        } => {
            DomainError::conflict("Display zone cannot be deleted through the custom zone endpoint")
        }
        ZoneMutationError::InvalidRole { .. } => {
            DomainError::conflict("Zone role does not support this mutation")
        }
        ZoneMutationError::LayoutOutputMismatch => DomainError::validation(
            "Zone layout must carry exactly the zone's current outputs; \
             add or remove outputs through the device endpoints",
        ),
    }
}

// ── Layout auto-exclusion bookkeeping ────────────────────────────────────

/// Reconcile the discovery auto-sync exclusions a repartition implies.
///
/// A zone whose output set the user edited by hand stops accepting
/// automatic layout sync for the outputs they removed, so the exclusion
/// set travels with the edit rather than with the endpoint that made it.
async fn update_zone_auto_exclusions(
    state: &AppState,
    scene_id: SceneId,
    previous_zones: &[Zone],
    updated_zones: &[Zone],
) {
    let changed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
        let mut changed = false;
        for previous_zone in previous_zones {
            let Some(updated_zone) = updated_zones
                .iter()
                .find(|zone| zone.id == previous_zone.id)
            else {
                continue;
            };
            if previous_zone.layout.zones == updated_zone.layout.zones {
                continue;
            }

            let key =
                layout_auto_exclusions::LayoutAutoExclusionKey::zone(scene_id, previous_zone.id);
            let current = exclusions.get(&key).cloned().unwrap_or_default();
            let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
                &previous_zone.layout.zones,
                &updated_zone.layout.zones,
                &current,
            );
            if next == current {
                continue;
            }
            if next.is_empty() {
                exclusions.remove(&key);
            } else {
                exclusions.insert(key, next);
            }
            changed = true;
        }
        changed
    };

    if changed {
        crate::api::persist_layout_auto_exclusions(state).await;
    }
}

/// Drop the exclusions a removed zone owned.
async fn remove_zone_auto_exclusions(state: &AppState, scene_id: SceneId, zone_id: ZoneId) {
    let removed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
        exclusions
            .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::zone(
                scene_id, zone_id,
            ))
            .is_some()
    };

    if removed {
        crate::api::persist_layout_auto_exclusions(state).await;
    }
}
