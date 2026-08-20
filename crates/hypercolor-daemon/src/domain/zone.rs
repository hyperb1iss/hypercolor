//! Zone domain services (Spec 76 §2.2, §2.3).
//!
//! Zone identity and metadata mutations commit through the same scene
//! transaction boundary as the canonical live tree API.
//!
//! Every service targets the active scene resolved from the same candidate
//! it validates and commits.

use hypercolor_core::scene::{ZoneMetaPatch, ZoneMutationError};
use hypercolor_types::scene::{SceneId, Zone, ZoneId, ZoneRole};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{SceneMutation, commit_scene};
use crate::domain::scene_tree::check_scene_revision;
use crate::domain::{DomainError, MutationContext, ResourceKind};
use crate::layout_auto_exclusions;

// ── Commands ─────────────────────────────────────────────────────────────

/// Add a custom zone to a scene.
#[derive(Debug, Clone)]
pub struct CreateZone {
    /// The zone's name. Must not be blank.
    pub name: String,
    /// Optional swatch for the Studio zone tree.
    pub color: Option<String>,
    /// Canvas dimensions the new zone's empty layout inherits.
    pub fallback_canvas: (u32, u32),
    /// The scene `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Patch a zone's presentation metadata.
#[derive(Debug, Clone)]
pub struct UpdateZone {
    /// Which zone to patch.
    pub zone_id: ZoneId,
    /// The fields to change; `None` fields keep their current values.
    pub patch: ZoneMetaPatch,
    /// The scene `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

/// Remove a custom zone from a scene.
#[derive(Debug, Clone)]
pub struct DeleteZone {
    /// Which zone to remove.
    pub zone_id: ZoneId,
    /// The scene `revision` the caller last saw, when it sent one.
    pub expected_revision: Option<u64>,
}

// ── Outcomes ─────────────────────────────────────────────────────────────

/// The outcome of a mutation that reports one zone.
#[derive(Debug)]
pub struct ZoneWritten {
    /// The zone as it now stands.
    pub zone: Zone,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of removing a zone.
#[derive(Debug)]
pub struct ZoneRemoved {
    /// The zone that was removed.
    pub zone: Zone,
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
/// [`DomainError::PreconditionFailed`] for a stale scene revision or
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

    let mut mutation = state.scene_manager.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("creating a zone")?;
    check_scene_revision(&mutation, command.expected_revision)?;

    let zone_id = mutation
        .create_zone(
            scene_id,
            command.name,
            command.color,
            command.fallback_canvas,
        )
        .map_err(|error| zone_error(error, scene_id, None, None))?;
    let zone = zone_in_scene(&mutation, scene_id, zone_id)?;

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;

    Ok(ZoneWritten { zone, commit })
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

    let mut mutation = state.scene_manager.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("updating a zone")?;
    check_scene_revision(&mutation, command.expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, command.zone_id)?;

    let zone = mutation
        .update_zone_meta(scene_id, command.zone_id, command.patch)
        .map_err(|error| zone_error(error, scene_id, Some(command.zone_id), None))?;

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;

    Ok(ZoneWritten { zone, commit })
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

    let mut mutation = state.scene_manager.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("deleting a zone")?;
    check_scene_revision(&mutation, command.expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, command.zone_id)?;

    let zone = zone_in_scene(&mutation, scene_id, command.zone_id)?;
    mutation
        .delete_zone(scene_id, command.zone_id)
        .map_err(|error| zone_error(error, scene_id, Some(command.zone_id), None))?;
    mutation.retire_zone_preview(scene_id, command.zone_id);

    let commit = commit_scene(state, mutation).await?;
    settle_zone_mutation(state).await;
    remove_zone_auto_exclusions(state, scene_id, command.zone_id).await;

    Ok(ZoneRemoved { zone, commit })
}

// ── Shared steps ─────────────────────────────────────────────────────────

/// Everything a structural zone change implies once it is committed:
/// the session snapshot records the new partition, and a device that
/// just entered or left the active scene reconnects or releases.
async fn settle_zone_mutation(state: &AppState) {
    crate::api::save_runtime_session_snapshot(state).await;
    crate::api::sync_connectivity(state).await;
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
        .zones
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
pub async fn reconcile_zone_auto_exclusions(
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
