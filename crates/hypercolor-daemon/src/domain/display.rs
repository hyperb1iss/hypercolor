//! Display-face domain services (Spec 76 §2.2, §2.3).
//!
//! A display carries its face on two layers. The **scene** layer lives
//! in the active scene's display zone and is persisted scene content;
//! the **default** layer lives in the preference store and materializes
//! each run into a runtime overlay zone that no save ever touches. Both
//! layers mutate scene state, so both commit — the difference is only
//! whether the commit arms a scene-store write, which the intent methods
//! decide.
//!
//! Device resolution, effect validation, and response shaping stay with
//! the adapters. A REST path segment naming a display by id or name, an
//! MCP argument doing the same, and the compaction that hides a
//! redundant blend mode from the response are all transport concerns.

use std::collections::HashMap;

use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint};
use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::event::ZoneChangeKind;
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use crate::domain::DomainError;
use crate::domain::commit::SceneCommit;
use crate::domain::context::SceneContext;

/// Native display surface geometry resolved from one tracked device.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySurfaceInfo {
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

/// Resolve native display geometry from segment topology or capabilities.
#[must_use]
pub fn display_surface_info(info: &DeviceInfo) -> Option<DisplaySurfaceInfo> {
    for segment in &info.segments {
        if let DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } = &segment.topology
        {
            return Some(DisplaySurfaceInfo {
                width: *width,
                height: *height,
                circular: *circular,
            });
        }
    }

    info.capabilities
        .display_resolution
        .filter(|_| info.capabilities.has_display)
        .map(|(width, height)| DisplaySurfaceInfo {
            width,
            height,
            circular: false,
        })
}

/// Build a native-resolution canvas for one display face.
#[must_use]
pub fn display_face_layout(
    device_id: DeviceId,
    device_name: &str,
    surface: DisplaySurfaceInfo,
) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: format!("{device_name} Display Face"),
        description: Some(format!("Native-resolution face canvas for {device_name}")),
        canvas_width: surface.width,
        canvas_height: surface.height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Assign a face to a display in the active scene.
#[derive(Debug, Clone)]
pub struct SetDisplayFace {
    /// Which display gets the face.
    pub device_id: DeviceId,
    /// The display's name, for the zone the assignment creates.
    pub device_name: String,
    /// The face, already validated as an HTML display effect.
    pub effect: EffectMetadata,
    /// Control overrides to store on the zone.
    pub controls: HashMap<String, ControlValue>,
    /// The display's native-resolution face canvas.
    pub layout: SpatialLayout,
    /// How the face composes over the effect layer beneath it.
    pub target: DisplayFaceTarget,
}

/// Strip the face assignment off a display's scene zone.
#[derive(Debug, Clone)]
pub struct ClearDisplayFace {
    /// Which display loses its face.
    pub device_id: DeviceId,
    /// The display's name, for the zone that survives the clear.
    pub device_name: String,
    /// The display's native-resolution face canvas.
    pub layout: SpatialLayout,
}

/// Update how a display's face composes over the effect layer.
#[derive(Debug, Clone)]
pub struct PatchDisplayComposition {
    /// Which display zone to patch.
    pub zone_id: ZoneId,
    /// The new blend mode, when the caller named one.
    pub blend_mode: Option<DisplayFaceBlendMode>,
    /// The new opacity, when the caller named one.
    pub opacity: Option<f32>,
}

/// Merge control overrides into a display's face zone.
#[derive(Debug, Clone)]
pub struct PatchDisplayFaceControls {
    /// Which display zone to patch.
    pub zone_id: ZoneId,
    /// Control values, already normalized against the face's schema.
    pub controls: HashMap<String, ControlValue>,
}

// ── Outcomes ─────────────────────────────────────────────────────────────

/// The outcome of a display-zone mutation.
#[derive(Debug)]
pub struct DisplayZoneWritten {
    /// The scene that owns the zone.
    pub scene_id: SceneId,
    /// The zone as it stands after the mutation.
    pub zone: Zone,
    /// Whether the zone was created or updated.
    pub change: ZoneChangeKind,
    /// The commit receipt.
    pub commit: SceneCommit,
}

// ── Scene-layer services ─────────────────────────────────────────────────

/// Assign a face to a display in the active scene.
///
/// # Errors
///
/// [`DomainError::Conflict`] when the active scene is snapshot-locked,
/// [`DomainError::Internal`] when the scene refuses the display zone,
/// and [`DomainError::Conflict`] when a concurrent scene
/// mutation lands first.
pub async fn set_display_face(
    ctx: &SceneContext,
    command: SetDisplayFace,
) -> Result<DisplayZoneWritten, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("assigning a display face")?;
    let change = if scene_display_zone(&mutation, command.device_id).is_some() {
        ZoneChangeKind::Updated
    } else {
        ZoneChangeKind::Created
    };

    let zone = mutation.upsert_display_zone(
        command.device_id,
        &command.device_name,
        &command.effect,
        command.controls,
        command.layout,
        command.target,
    )?;
    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(DisplayZoneWritten {
        scene_id,
        zone,
        change,
        commit,
    })
}

/// Strip the face assignment off a display's scene zone, keeping the
/// zone so the display keeps a surface to render into.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn clear_display_face(
    ctx: &SceneContext,
    command: ClearDisplayFace,
) -> Result<DisplayZoneWritten, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("removing a display face")?;
    let change = if scene_display_zone(&mutation, command.device_id).is_some() {
        ZoneChangeKind::Updated
    } else {
        ZoneChangeKind::Created
    };

    let zone = mutation.clear_display_assignment(
        command.device_id,
        &command.device_name,
        command.layout,
    )?;

    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(DisplayZoneWritten {
        scene_id,
        zone,
        change,
        commit,
    })
}

/// Update how a display's face composes over the effect layer.
///
/// `Ok(None)` means no display zone with that id, which each transport
/// renders as its own not-found.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn patch_display_composition(
    ctx: &SceneContext,
    command: PatchDisplayComposition,
) -> Result<Option<DisplayZoneWritten>, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id =
        mutation.active_scene_for_runtime_mutation("updating display face composition")?;
    let Some(zone) =
        mutation.patch_display_target(command.zone_id, command.blend_mode, command.opacity)
    else {
        return Ok(None);
    };

    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(Some(DisplayZoneWritten {
        scene_id,
        zone,
        change: ZoneChangeKind::Updated,
        commit,
    }))
}

/// Merge control overrides into a display's face zone.
///
/// `Ok(None)` means no display zone with that id.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn patch_display_face_controls(
    ctx: &SceneContext,
    command: PatchDisplayFaceControls,
) -> Result<Option<DisplayZoneWritten>, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("updating display face controls")?;
    let Some(zone) = mutation.patch_zone_controls(command.zone_id, command.controls) else {
        return Ok(None);
    };

    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(Some(DisplayZoneWritten {
        scene_id,
        zone,
        change: ZoneChangeKind::ControlsPatched,
        commit,
    }))
}

/// Keep every connected display's scene zone aligned with the device's
/// geometry, and report whether anything moved.
///
/// A scene that forbids runtime rewriting is left alone rather than
/// refused: surface sync runs on scene activation and on every display
/// listing, so it must be a no-op on a snapshot scene rather than an
/// error the caller has to handle.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn sync_display_surfaces(
    ctx: &SceneContext,
    displays: Vec<(DeviceId, String, SpatialLayout)>,
) -> Result<bool, DomainError> {
    let outcome = ctx
        .commit_retrying(|mutation| {
            let Some(active) = mutation.scenes().active_scene() else {
                return Ok(None);
            };
            if active.blocks_runtime_mutation() {
                return Ok(None);
            }

            let mut changed = false;
            for (device_id, device_name, layout) in &displays {
                match mutation.ensure_display_surface(*device_id, device_name, layout.clone()) {
                    Ok(moved) => changed |= moved,
                    Err(error) => {
                        tracing::warn!(%error, %device_id, "Failed to sync display screen surface");
                    }
                }
            }
            Ok(changed.then_some(()))
        })
        .await?;

    if let Some(((), commit)) = outcome.as_ref() {
        commit.log_if_retrying("Failed to persist display surfaces");
    }
    Ok(outcome.is_some())
}

/// Refresh native geometry for display zones that already exist in the
/// active scene, including snapshot scenes.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation lands first.
pub async fn hydrate_existing_display_surfaces(
    ctx: &SceneContext,
    displays: Vec<(DeviceId, String, SpatialLayout)>,
) -> Result<bool, DomainError> {
    let outcome = ctx
        .commit_retrying(|mutation| {
            let Some(scene_id) = mutation.scenes().active_scene_id().copied() else {
                return Ok(None);
            };
            let changed = mutation.hydrate_existing_display_surfaces(scene_id, &displays)?;
            Ok(changed.then_some(()))
        })
        .await?;

    if let Some(((), commit)) = outcome.as_ref() {
        commit.log_if_retrying("Failed to persist display surface geometry");
    }
    Ok(outcome.is_some())
}

/// Drop every scene's display zone for a device, and its runtime default
/// overlay with them.
///
/// Returns the removed scene zones and the removed default overlay. The
/// default zone goes even when the scene zones do not, because a deleted
/// device must never keep a live zone demanding face frames and it can
/// no longer be addressed through the displays API to clear one.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn prune_display_zones_for_device(
    ctx: &SceneContext,
    device_id: DeviceId,
) -> Result<PrunedDisplayZones, DomainError> {
    let outcome = ctx
        .commit_retrying(|mutation| {
            let removed_zones = mutation.remove_display_zones_for_device(device_id);
            let removed_default = mutation.remove_default_display_zone(device_id);
            if removed_zones.is_empty() && removed_default.is_none() {
                return Ok(None);
            }
            Ok(Some((removed_zones, removed_default)))
        })
        .await?;

    let Some(((removed_zones, removed_default), commit)) = outcome else {
        return Ok(PrunedDisplayZones::empty());
    };

    Ok(PrunedDisplayZones {
        removed_zones,
        removed_default,
        commit: Some(commit),
    })
}

/// What pruning a device's display zones removed.
#[derive(Debug)]
pub struct PrunedDisplayZones {
    /// Every scene zone that was bound to the device.
    pub removed_zones: Vec<(SceneId, Zone)>,
    /// The runtime default overlay, when one existed.
    pub removed_default: Option<Zone>,
    /// The commit receipt, absent when the device owned no display zone
    /// on either layer and nothing was committed.
    pub commit: Option<SceneCommit>,
}

impl PrunedDisplayZones {
    /// The outcome of a prune that removed nothing.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            removed_zones: Vec::new(),
            removed_default: None,
            commit: None,
        }
    }
}

// ── Default-layer services ───────────────────────────────────────────────

/// Install the runtime overlay zone a display preference resolves to.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn set_default_display_overlay(
    ctx: &SceneContext,
    device_id: DeviceId,
    zone: Zone,
) -> Result<Option<Zone>, DomainError> {
    let mut already_installed = None;
    let outcome = ctx
        .commit_retrying(|mutation| {
            already_installed = None;
            if !mutation.set_default_display_zone(zone.clone()) {
                // The preference already resolves to exactly this overlay.
                // Re-installing it would commit, and a commit invalidates
                // every in-flight candidate — which is how a read path ends
                // up failing a user's write.
                already_installed = mutation
                    .scenes()
                    .default_display_group_for(device_id)
                    .cloned();
                return Ok(None);
            }
            Ok(Some(
                mutation
                    .scenes()
                    .default_display_group_for(device_id)
                    .cloned(),
            ))
        })
        .await?;

    Ok(outcome.map_or(already_installed, |(installed, _commit)| installed))
}

/// Remove a display's runtime default overlay zone.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn remove_default_display_overlay(
    ctx: &SceneContext,
    device_id: DeviceId,
) -> Result<Option<Zone>, DomainError> {
    let outcome = ctx
        .commit_retrying(|mutation| Ok(mutation.remove_default_display_zone(device_id)))
        .await?;

    Ok(outcome.map(|(removed, _commit)| removed))
}

/// Whether the active scene already carries a display zone for a device.
fn scene_display_zone(
    mutation: &crate::domain::scene::SceneMutation,
    device_id: DeviceId,
) -> Option<Zone> {
    mutation
        .scenes()
        .active_scene()?
        .display_zone_for(device_id)
        .cloned()
}
