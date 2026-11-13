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
//! Device selector resolution and response shaping stay with the adapters.
//! Effect identity and controls are admitted here under the catalog guard
//! before either layer can mutate authoritative state.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::scene::screens::{self, ScreenStack, ScreenSurfaces, ScreenView};
use hypercolor_core::scene::{LayerMutationError, SceneManager, ZoneMutationError};
use hypercolor_types::api::displays::DisplayFaceRendering;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::event::{HypercolorEvent, LayerStackChangeKind, ZoneChangeKind};
use hypercolor_types::layer::{BlendMode, LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::RwLock;

use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::{DisplayPreference, DisplayPreferencesStore};
use crate::domain::commit::SceneCommit;
use crate::domain::context::{DeviceContext, SceneContext};
use crate::domain::effect::{
    AdmittedEffectControls, EffectContext, EffectMutationAdmission, ResolvedEffect,
};
use crate::domain::scene::{COMMIT_ATTEMPTS, SceneMutation};
use crate::domain::scene_tree::layer_error;
use crate::domain::{DomainError, ResourceKind};

/// Native display surface geometry resolved from one tracked device.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySurfaceInfo {
    pub width: u32,
    pub height: u32,
    pub circular: bool,
}

/// Resolve native display geometry from the device's display segment.
#[must_use]
pub fn display_surface_info(info: &DeviceInfo) -> Option<DisplaySurfaceInfo> {
    info.display_surface().map(|surface| DisplaySurfaceInfo {
        width: surface.width,
        height: surface.height,
        circular: surface.circular,
    })
}

/// Build a native-resolution canvas for one display face.
#[must_use]
pub fn display_face_layout(
    device_id: DeviceId,
    device_name: &str,
    surface: DisplaySurfaceInfo,
) -> SpatialLayout {
    screens::screen_face_layout(device_id, device_name, surface.width, surface.height)
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Make a face the whole of a screen's stack in the active scene.
#[derive(Debug, Clone)]
pub struct SetDisplayFace {
    /// Which display gets the face.
    pub device_id: DeviceId,
    /// The face resolved against one catalog generation.
    pub effect: ResolvedEffect,
    /// Control overrides to store on the layer.
    pub controls: HashMap<String, ControlValue>,
    /// How the face composes over the screen's backdrop.
    pub target: DisplayFaceTarget,
}

/// Update how a screen's stack composes over its backdrop.
#[derive(Debug, Clone)]
pub struct PatchDisplayComposition {
    /// Which display to patch.
    pub device_id: DeviceId,
    /// The new blend mode, when the caller named one.
    pub blend_mode: Option<BlendMode>,
    /// The new opacity, when the caller named one.
    pub opacity: Option<f32>,
}

/// Merge control overrides into a screen's one face layer.
#[derive(Debug, Clone)]
pub struct PatchDisplayFaceControls {
    /// Which display to patch.
    pub device_id: DeviceId,
    /// Canonical control values to validate against the assigned face.
    pub controls: HashMap<String, ControlValue>,
}

// ── Outcomes ─────────────────────────────────────────────────────────────

/// The outcome of a write to a screen's stack.
#[derive(Debug)]
pub struct DisplayZoneWritten {
    /// The scene that owns the stack.
    pub scene_id: SceneId,
    /// The screen as it stands after the write: its view, or its stored
    /// display zone while one exists.
    pub zone: Zone,
    /// The zone that places the screen and the screen's output in it.
    pub owner: Option<(ZoneId, String)>,
    /// Whether the screen had a stack before.
    pub change: ZoneChangeKind,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of clearing a screen's stack.
#[derive(Debug)]
pub struct DisplayFaceCleared {
    /// The scene the stack was cleared from.
    pub scene_id: SceneId,
    /// The screen's now empty view, when the screen is connected.
    pub view: Option<ScreenView>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// A display control mutation plus the schema that admitted it.
#[derive(Debug)]
pub struct DisplayControlsWritten {
    pub effect: EffectMetadata,
    pub written: DisplayZoneWritten,
}

fn validate_display_face(metadata: &EffectMetadata) -> Result<(), DomainError> {
    if metadata.category != EffectCategory::Display {
        return Err(DomainError::validation(format!(
            "Effect '{}' is not a display face",
            metadata.name
        )));
    }
    if !matches!(metadata.source, EffectSource::Html { .. }) {
        return Err(DomainError::validation(format!(
            "Effect '{}' is not an HTML display face",
            metadata.name
        )));
    }
    Ok(())
}

pub(crate) async fn admit_display_face_controls(
    ctx: &EffectContext,
    effect: ResolvedEffect,
    controls: &HashMap<String, ControlValue>,
) -> Result<AdmittedEffectControls, DomainError> {
    validate_display_face(&effect)?;
    ctx.admit_resolved_controls(effect, controls).await
}

pub(crate) async fn admit_current_display_face_controls(
    ctx: &EffectContext,
    effect_id: hypercolor_types::effect::EffectId,
    controls: &HashMap<String, ControlValue>,
) -> Result<AdmittedEffectControls, DomainError> {
    let admitted = ctx.admit_current_controls(effect_id, controls).await?;
    validate_display_face(admitted.metadata())?;
    Ok(admitted)
}

pub(crate) async fn resolve_display_face_controls_under_admission(
    ctx: &EffectContext,
    admission: &EffectMutationAdmission,
    effect_id: hypercolor_types::effect::EffectId,
    controls: &HashMap<String, ControlValue>,
) -> Result<(EffectMetadata, HashMap<String, ControlValue>), DomainError> {
    let (metadata, controls) = ctx
        .resolve_controls_under_admission(admission, effect_id, controls)
        .await?;
    validate_display_face(&metadata)?;
    Ok((metadata, controls))
}

// ── Scene-layer services ─────────────────────────────────────────────────

/// The screen as a write left it: its view, or its stored display zone.
fn screen_zone(
    mutation: &SceneMutation,
    device_id: DeviceId,
) -> Option<(Zone, Option<(ZoneId, String)>)> {
    if let Some(view) = mutation.screen_view(device_id) {
        return Some((view.zone, view.owner));
    }
    mutation
        .scenes()
        .active_scene()?
        .display_zone_for(device_id)
        .cloned()
        .map(|zone| (zone, None))
}

fn display_zone_error(error: &ZoneMutationError, device_id: DeviceId) -> DomainError {
    match error {
        ZoneMutationError::SnapshotLocked => {
            DomainError::conflict("Snapshot scene cannot be structurally edited")
        }
        ZoneMutationError::SceneMissing => DomainError::not_found(ResourceKind::Scene, "active"),
        _ => DomainError::not_found(ResourceKind::Device, device_id),
    }
}

/// The lowest face layer of a screen's stack: its id and effect.
fn face_layer(stack: ScreenStack<'_>) -> Option<&SceneLayer> {
    stack
        .layers()
        .into_iter()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
}

/// Make a face the whole of a screen's stack in the active scene.
///
/// For a screen a zone places, the screen's own layers are removed, it
/// leaves the scope of layers it shares, and a stored display zone for it
/// is replaced. A screen no zone places has no output to scope a layer
/// to, so its face is written to the scene's stored display zone for it.
/// Input bindings carry over when the face is the one already assigned.
///
/// # Errors
///
/// [`DomainError::Conflict`] when the active scene is snapshot-locked or
/// a concurrent scene mutation lands first, and
/// [`DomainError::NotFound`] for a device that is not a known screen.
pub async fn set_display_face(
    ctx: &EffectContext,
    command: SetDisplayFace,
) -> Result<DisplayZoneWritten, DomainError> {
    let device_id = command.device_id;
    let target = normalize_display_face_target(command.target);
    let admitted = admit_display_face_controls(ctx, command.effect, &command.controls).await?;
    let (effect, controls, _admission) = admitted.into_parts();
    let scene = ctx.scene_context();
    let mut mutation = scene.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("assigning a display face")?;
    // A face may be the first thing written for a display the surface
    // table has not seen yet, so the write reads the registry itself. It
    // reads after the mutation begins: the commit lands only if nothing
    // else committed since, so this table cannot take a newer one back.
    let surfaces = scene.known_screen_surfaces().await;
    mutation.set_screen_surfaces(surfaces);
    let stack = mutation.scenes().active_screen_stack(device_id);
    let change = if stack.is_some_and(|stack| !stack.layers().is_empty()) {
        ZoneChangeKind::Updated
    } else {
        ZoneChangeKind::Created
    };

    if let Some((owner, output_id)) = mutation.screen_placement(device_id) {
        let bindings = stack
            .and_then(face_layer)
            .and_then(|layer| match &layer.source {
                LayerSource::Effect {
                    effect_id,
                    control_bindings,
                    ..
                } if *effect_id == effect.id => Some(control_bindings.clone()),
                _ => None,
            })
            .unwrap_or_default();
        mutation.remove_legacy_display_zone(scene_id, device_id);
        let mut layer =
            SceneLayer::from_effect(SceneLayerId::new(), effect.id, controls, bindings, None);
        layer.blend = target.blend_mode;
        layer.opacity = target.opacity;
        layer.scope = Some(LayerScope {
            outputs: vec![output_id.clone()],
        });
        mutation
            .edit_screen_layers(
                scene_id,
                owner,
                device_id,
                LayerStackChangeKind::Updated,
                |zone| {
                    screens::clear_screen_layers(zone, &output_id);
                    zone.layers.push(layer);
                    Ok(())
                },
            )
            .map_err(|error| layer_error(error, owner, None))?;
    } else {
        // No zone places the screen, so there is no output to scope a
        // layer to: its face is stored as a display zone.
        mutation
            .write_display_zone_face(scene_id, device_id, &effect, controls, target)
            .map_err(|error| display_zone_error(&error, device_id))?;
    }
    let (zone, owner) = screen_zone(&mutation, device_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Device, device_id))?;
    let commit = scene.commit(mutation).await?;
    scene.save_runtime_session().await;

    Ok(DisplayZoneWritten {
        scene_id,
        zone,
        owner,
        change,
        commit,
    })
}

/// Remove a screen's stack from the active scene: its own layers are
/// deleted, it leaves the scope of layers it shares, and a stored display
/// zone for it is removed.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn clear_display_face(
    ctx: &SceneContext,
    device_id: DeviceId,
) -> Result<DisplayFaceCleared, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("removing a display face")?;
    mutation.remove_legacy_display_zone(scene_id, device_id);
    let placed = mutation
        .scenes()
        .active_screen_stack(device_id)
        .and_then(|stack| match stack {
            ScreenStack::Placed { zone, output } if !stack.layers().is_empty() => {
                Some((zone.id, output.id.clone()))
            }
            _ => None,
        });
    if let Some((owner, output_id)) = placed {
        mutation
            .edit_screen_layers(
                scene_id,
                owner,
                device_id,
                LayerStackChangeKind::Removed,
                |zone| {
                    screens::clear_screen_layers(zone, &output_id);
                    Ok(())
                },
            )
            .map_err(|error| layer_error(error, owner, None))?;
    }
    let view = mutation.screen_view(device_id);
    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(DisplayFaceCleared {
        scene_id,
        view,
        commit,
    })
}

/// Update how a screen's stack composes over its backdrop: the stored
/// target of a legacy zone, or the blend and opacity of the stack's
/// bottom enabled layer. A layer other screens share is split first.
///
/// `Ok(None)` means the screen has no stack to patch, which each
/// transport renders as its own not-found.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn patch_display_composition(
    ctx: &SceneContext,
    command: PatchDisplayComposition,
) -> Result<Option<DisplayZoneWritten>, DomainError> {
    let device_id = command.device_id;
    let mut mutation = ctx.begin_mutation().await;
    let scene_id =
        mutation.active_scene_for_runtime_mutation("updating display face composition")?;
    let Some(stack) = mutation.scenes().active_screen_stack(device_id) else {
        return Ok(None);
    };
    match stack {
        ScreenStack::Legacy(zone) => {
            let zone_id = zone.id;
            if mutation
                .patch_display_target(zone_id, command.blend_mode, command.opacity)
                .is_none()
            {
                return Ok(None);
            }
        }
        ScreenStack::Placed { zone, output } => {
            let layers = stack.layers();
            let Some(bottom) = layers.iter().find(|layer| layer.enabled).or(layers.first()) else {
                return Ok(None);
            };
            let (owner, output_id, layer_id) = (zone.id, output.id.clone(), bottom.id);
            mutation
                .edit_screen_layers(
                    scene_id,
                    owner,
                    device_id,
                    LayerStackChangeKind::Updated,
                    |zone| {
                        let index = screens::split_for_screen(zone, layer_id, &output_id)
                            .ok_or(LayerMutationError::LayerMissing { layer_id })?;
                        let layer = &mut zone.layers[index];
                        let target = normalize_display_face_target(DisplayFaceTarget {
                            device_id,
                            blend_mode: command.blend_mode.unwrap_or(layer.blend),
                            opacity: command.opacity.unwrap_or(layer.opacity),
                        });
                        layer.blend = target.blend_mode;
                        layer.opacity = target.opacity;
                        Ok(())
                    },
                )
                .map_err(|error| layer_error(error, owner, Some(layer_id)))?;
        }
        ScreenStack::Unplaced => return Ok(None),
    }
    let Some((zone, owner)) = screen_zone(&mutation, device_id) else {
        return Ok(None);
    };
    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(Some(DisplayZoneWritten {
        scene_id,
        zone,
        owner,
        change: ZoneChangeKind::Updated,
        commit,
    }))
}

/// Merge control overrides into a screen's one face layer. A layer other
/// screens share is split first, so only this screen's copy changes.
///
/// `Ok(None)` means the screen's stack holds no face layer, or more than
/// one, so there is no single face to patch.
///
/// # Errors
///
/// As [`set_display_face`].
pub async fn patch_display_face_controls(
    ctx: &EffectContext,
    command: PatchDisplayFaceControls,
) -> Result<Option<DisplayControlsWritten>, DomainError> {
    let device_id = command.device_id;
    let scene = ctx.scene_context();
    let mut mutation = scene.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("updating display face controls")?;
    let Some(stack) = mutation.scenes().active_screen_stack(device_id) else {
        return Ok(None);
    };
    let faces = stack
        .layers()
        .into_iter()
        .filter_map(|layer| match &layer.source {
            LayerSource::Effect { effect_id, .. } => Some((layer.id, *effect_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(layer_id, effect_id)] = faces[..] else {
        return Ok(None);
    };
    let admitted = admit_current_display_face_controls(ctx, effect_id, &command.controls).await?;
    let (effect, controls, _admission) = admitted.into_parts();
    match stack {
        ScreenStack::Legacy(zone) => {
            let zone_id = zone.id;
            if mutation.patch_zone_controls(zone_id, controls).is_none() {
                return Ok(None);
            }
        }
        ScreenStack::Placed { zone, output } => {
            let (owner, output_id) = (zone.id, output.id.clone());
            mutation
                .edit_screen_layers(
                    scene_id,
                    owner,
                    device_id,
                    LayerStackChangeKind::ControlsPatched,
                    |zone| {
                        let index = screens::split_for_screen(zone, layer_id, &output_id)
                            .ok_or(LayerMutationError::LayerMissing { layer_id })?;
                        if let LayerSource::Effect {
                            controls: stored, ..
                        } = &mut zone.layers[index].source
                        {
                            stored.extend(controls);
                        }
                        Ok(())
                    },
                )
                .map_err(|error| layer_error(error, owner, Some(layer_id)))?;
        }
        ScreenStack::Unplaced => return Ok(None),
    }
    let Some((zone, owner)) = screen_zone(&mutation, device_id) else {
        return Ok(None);
    };
    let commit = scene.commit(mutation).await?;
    scene.save_runtime_session().await;

    Ok(Some(DisplayControlsWritten {
        effect,
        written: DisplayZoneWritten {
            scene_id,
            zone,
            owner,
            change: ZoneChangeKind::ControlsPatched,
            commit,
        },
    }))
}

/// Install a table of known screens, and convert the active scene's
/// stored display zones now that their screens are known.
///
/// The surface table is runtime state, so a change to it alone commits
/// without a scene-store write. Conversion skips a scene that forbids
/// runtime rewriting, so this is a no-op there rather than an error.
///
/// The caller read `surfaces` before this began, so a commit that lands in
/// between can be overwritten by it. The daemon's own sync is
/// [`sync_known_screens`], which reads inside each attempt.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn sync_screen_surfaces(
    ctx: &SceneContext,
    surfaces: ScreenSurfaces,
) -> Result<bool, DomainError> {
    let outcome = ctx
        .commit_retrying(|mutation| Ok(install_screen_surfaces(mutation, surfaces.clone())))
        .await?;

    if let Some(((), commit)) = outcome.as_ref() {
        commit.log_if_retrying("Failed to persist converted screen layers");
        // The default scene lives in the runtime session, so a conversion
        // of it is saved here, as every other write to it is.
        ctx.save_runtime_session().await;
    }
    Ok(outcome.is_some())
}

fn install_screen_surfaces(mutation: &mut SceneMutation, surfaces: ScreenSurfaces) -> Option<()> {
    let mut changed = mutation.set_screen_surfaces(surfaces);
    if let Some(scene_id) = mutation.scenes().active_scene_id().copied() {
        changed |= mutation.convert_legacy_display_zones(scene_id).changed();
    }
    changed.then_some(())
}

/// Read the device registry and install its screens as the table of known
/// screens, converting the active scene's stored display zones whose
/// screens are now known.
///
/// Each attempt reads the registry after its mutation begins, and the
/// commit lands only if nothing else committed since. So the table an
/// attempt installs was read later than any table already installed, and
/// two syncs racing each other, or one racing an activation, cannot leave
/// the older read in place.
///
/// # Errors
///
/// [`DomainError::Conflict`] when every attempt loses to a concurrent
/// scene mutation.
pub async fn sync_known_screens(ctx: &SceneContext) -> Result<bool, DomainError> {
    let mut last_conflict = None;
    for _ in 0..COMMIT_ATTEMPTS {
        let mut mutation = ctx.begin_mutation().await;
        let surfaces = ctx.known_screen_surfaces().await;
        if install_screen_surfaces(&mut mutation, surfaces).is_none() {
            return Ok(false);
        }
        match ctx.commit(mutation).await {
            Ok(commit) => {
                commit.log_if_retrying("Failed to persist converted screen layers");
                ctx.save_runtime_session().await;
                return Ok(true);
            }
            Err(conflict @ DomainError::Conflict { .. }) => last_conflict = Some(conflict),
            Err(error) => return Err(error),
        }
    }
    Err(last_conflict.unwrap_or_else(|| {
        DomainError::conflict("the screen sync did not converge after repeated concurrent writes")
    }))
}

/// Drop every scene's display zone for a device, its runtime default
/// overlay, and its entry in the table of known screens.
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
            // The device is gone, so it is no longer a screen anyone can
            // address: its view leaves with it. Layers scoped to its
            // output stay where they are and wait, like any layer whose
            // output was forgotten.
            let mut surfaces = mutation.scenes().screen_surfaces().clone();
            surfaces.retain(|_, surface| surface.device_id != device_id);
            let forgot_screen = mutation.set_screen_surfaces(surfaces);
            if removed_zones.is_empty() && removed_default.is_none() && !forgot_screen {
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
                    .default_display_zone_for(device_id)
                    .cloned();
                return Ok(None);
            }
            Ok(Some(
                mutation
                    .scenes()
                    .default_display_zone_for(device_id)
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

// ── Default-face context ─────────────────────────────────────────────────

/// Which layers currently carry a face for one display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayFaceLayers {
    /// The active scene holds a face for the screen: a layer with an
    /// effect source in its stack, enabled or not, in a zone enabled or
    /// not.
    pub scene_assigned: bool,
    /// The active scene's stack draws on the screen: at least one enabled
    /// layer in an enabled zone.
    pub scene_rendering: bool,
    /// The preference store carries a default face.
    pub default_assigned: bool,
}

impl DisplayFaceLayers {
    /// What is on the panel over its backdrop. The kept face shows only
    /// when the scene's stack is neither assigned nor rendering.
    #[must_use]
    pub const fn rendering(self) -> DisplayFaceRendering {
        if self.scene_rendering {
            DisplayFaceRendering::Scene
        } else if !self.scene_assigned && self.default_assigned {
            DisplayFaceRendering::Default
        } else {
            DisplayFaceRendering::Nothing
        }
    }
}

/// Assign the default face a display keeps across scene switches.
#[derive(Debug, Clone)]
pub struct SetDefaultDisplayFace {
    /// Which display gets the face.
    pub device_id: DeviceId,
    /// The face resolved against one catalog generation.
    pub effect: ResolvedEffect,
    /// Control overrides to store with the preference.
    pub controls: HashMap<String, ControlValue>,
    /// How the face composes over the effect layer beneath it.
    pub target: DisplayFaceTarget,
}

/// The outcome of writing a display's default face.
#[derive(Debug)]
pub struct DefaultDisplayFaceWritten {
    /// The face as the catalog admitted it.
    pub effect: EffectMetadata,
    /// The runtime overlay zone the preference materialized into.
    pub zone: Zone,
    /// The scene the overlay renders alongside.
    pub scene_id: SceneId,
    /// Whether the active scene's own assignment still wins the display.
    pub scene_assigned: bool,
}

/// The outcome of dropping a display's default face.
#[derive(Debug)]
pub struct DefaultDisplayFaceCleared {
    /// Whether a stored preference was actually removed.
    pub removed: bool,
    /// The retracted overlay zone, present only when the display now shows
    /// nothing and render observers have to be told.
    pub retracted: Option<Zone>,
    /// The scene the retraction applies to.
    pub scene_id: SceneId,
}

/// Clamp a face's composition to what the renderer can honor.
///
/// A face that does not blend covers the layer beneath it outright, so a
/// partial opacity would describe a composite that never happens.
#[must_use]
pub fn normalize_display_face_target(target: DisplayFaceTarget) -> DisplayFaceTarget {
    screens::normalize_face_target(target)
}

/// Default-face authority for displays.
///
/// The default layer lives in the preference store and materializes each
/// run into a runtime overlay zone. Every transport enters here, so an
/// assignment reaches the store, the catalog guard, and the scene in one
/// order no adapter can reorder.
struct DisplayAuthorities {
    preferences: Arc<RwLock<DisplayPreferencesStore>>,
    frames: Arc<RwLock<DisplayFrameRuntime>>,
    scene: SceneContext,
    effects: EffectContext,
    devices: DeviceContext,
    event_bus: Arc<HypercolorBus>,
}

#[derive(Clone)]
pub struct DisplayContext {
    authorities: Arc<DisplayAuthorities>,
}

impl DisplayContext {
    pub(crate) fn new(
        preferences: Arc<RwLock<DisplayPreferencesStore>>,
        frames: Arc<RwLock<DisplayFrameRuntime>>,
        scene: SceneContext,
        effects: EffectContext,
        devices: DeviceContext,
        event_bus: Arc<HypercolorBus>,
    ) -> Self {
        Self {
            authorities: Arc::new(DisplayAuthorities {
                preferences,
                frames,
                scene,
                effects,
                devices,
                event_bus,
            }),
        }
    }

    /// Tell render observers that a default-face overlay zone changed.
    ///
    /// The scene commit announces the structural install or removal; this
    /// explicit event carries the overlay's content change (face, controls,
    /// composition), and the domain publishes it at every write so the two
    /// transports cannot diverge on when observers hear about an overlay.
    fn publish_overlay_change(&self, scene_id: SceneId, zone: &Zone, kind: ZoneChangeKind) {
        self.authorities
            .event_bus
            .publish(HypercolorEvent::ZoneChanged {
                scene_id,
                zone_id: zone.id,
                role: zone.role,
                kind,
            });
    }

    /// Latest composited frame per display, for preview surfaces.
    #[must_use]
    pub fn frames(&self) -> &Arc<RwLock<DisplayFrameRuntime>> {
        &self.authorities.frames
    }

    /// Persisted per-display default face preferences.
    #[must_use]
    pub fn preferences(&self) -> &Arc<RwLock<DisplayPreferencesStore>> {
        &self.authorities.preferences
    }

    /// Whether a display carries a stored default face.
    pub async fn has_default_face(&self, device_id: DeviceId) -> bool {
        self.authorities
            .preferences
            .read()
            .await
            .get(device_id)
            .is_some()
    }

    /// Resolve both assignment layers for a display.
    pub async fn face_layers(&self, device_id: DeviceId) -> DisplayFaceLayers {
        let (scene_assigned, scene_rendering) = {
            let scenes = self.authorities.scene.snapshot().await;
            scenes
                .active_screen_stack(device_id)
                .map_or((false, false), |stack| {
                    (stack.assigned(), stack.rendering())
                })
        };
        DisplayFaceLayers {
            scene_assigned,
            scene_rendering,
            default_assigned: self.has_default_face(device_id).await,
        }
    }

    async fn active_scene_id(&self) -> SceneId {
        let scenes = self.authorities.scene.snapshot().await;
        scenes
            .active_scene()
            .map_or(SceneId::DEFAULT, |scene| scene.id)
    }

    /// Write a display's default face and materialize its overlay.
    ///
    /// # Errors
    ///
    /// [`DomainError::Validation`] when the effect is not an HTML display
    /// face or a control value fails admission, and
    /// [`DomainError::Internal`] when the preference cannot be persisted
    /// or the overlay refuses to install.
    pub async fn set_default_face(
        &self,
        command: SetDefaultDisplayFace,
    ) -> Result<DefaultDisplayFaceWritten, DomainError> {
        let SetDefaultDisplayFace {
            device_id,
            effect,
            controls,
            target,
        } = command;
        let target = normalize_display_face_target(target);
        let admitted =
            admit_display_face_controls(&self.authorities.effects, effect, &controls).await?;
        let (effect, controls, admission) = admitted.into_parts();
        {
            let mut store = self.authorities.preferences.write().await;
            store
                .set(
                    device_id,
                    DisplayPreference {
                        blend_mode: target.blend_mode,
                        controls,
                        effect_id: effect.id,
                        opacity: target.opacity,
                    },
                )
                .map_err(|error| {
                    DomainError::Internal(anyhow::anyhow!(
                        "Failed to prepare display preference persistence: {error}"
                    ))
                })?;
        }
        drop(admission);

        let Some(zone) = self.apply_preference_overlay(device_id).await else {
            return Err(DomainError::Internal(anyhow::anyhow!(
                "Failed to install the default face overlay"
            )));
        };
        let layers = self.face_layers(device_id).await;
        let scene_id = self.active_scene_id().await;
        if !layers.scene_assigned {
            self.publish_overlay_change(scene_id, &zone, ZoneChangeKind::Updated);
        }

        Ok(DefaultDisplayFaceWritten {
            effect,
            zone,
            scene_id,
            scene_assigned: layers.scene_assigned,
        })
    }

    /// Drop a display's default face and retract its overlay.
    ///
    /// # Errors
    ///
    /// [`DomainError::Internal`] when the preference store cannot be
    /// written, and [`DomainError::Conflict`] when a concurrent scene
    /// mutation lands first.
    pub async fn clear_default_face(
        &self,
        device_id: DeviceId,
    ) -> Result<DefaultDisplayFaceCleared, DomainError> {
        let removed = {
            let mut store = self.authorities.preferences.write().await;
            store
                .remove(device_id)
                .map_err(|error| {
                    DomainError::Internal(anyhow::anyhow!(
                        "Failed to prepare display preference persistence: {error}"
                    ))
                })?
                .is_some()
        };
        let layers = self.face_layers(device_id).await;
        let cleared = remove_default_display_overlay(&self.authorities.scene, device_id).await?;
        let retracted = if removed && !layers.scene_assigned {
            cleared.map(|mut zone| {
                zone.layers.clear();
                zone
            })
        } else {
            None
        };

        let scene_id = self.active_scene_id().await;
        if let Some(zone) = retracted.as_ref() {
            self.publish_overlay_change(scene_id, zone, ZoneChangeKind::Updated);
        }

        Ok(DefaultDisplayFaceCleared {
            removed,
            retracted,
            scene_id,
        })
    }

    /// Update how a display's default face composes with the layer below.
    ///
    /// # Errors
    ///
    /// [`DomainError::NotFound`] when the display carries no default face,
    /// and [`DomainError::Internal`] when the preference cannot be
    /// persisted.
    pub async fn patch_default_composition(
        &self,
        device_id: DeviceId,
        blend_mode: Option<BlendMode>,
        opacity: Option<f32>,
    ) -> Result<(), DomainError> {
        let mut store = self.authorities.preferences.write().await;
        let Some(mut updated) = store.get(device_id).cloned() else {
            return Err(DomainError::not_found(
                ResourceKind::Zone,
                format!("default-face:{device_id}"),
            ));
        };
        let target = normalize_display_face_target(DisplayFaceTarget {
            blend_mode: blend_mode.unwrap_or(updated.blend_mode),
            device_id,
            opacity: opacity.unwrap_or(updated.opacity),
        });
        updated.blend_mode = target.blend_mode;
        updated.opacity = target.opacity;
        store.set(device_id, updated).map_err(|error| {
            DomainError::Internal(anyhow::anyhow!(
                "Failed to prepare display preference persistence: {error}"
            ))
        })?;
        drop(store);

        if let Some(zone) = self.apply_preference_overlay(device_id).await {
            let scene_id = self.active_scene_id().await;
            self.publish_overlay_change(scene_id, &zone, ZoneChangeKind::Updated);
        }
        Ok(())
    }

    /// Merge control overrides into a display's stored default face.
    ///
    /// # Errors
    ///
    /// [`DomainError::NotFound`] when the display carries no default face,
    /// [`DomainError::Validation`] when a control value fails admission,
    /// and [`DomainError::Internal`] when the preference cannot be
    /// persisted.
    pub async fn merge_default_controls(
        &self,
        device_id: DeviceId,
        controls: &HashMap<String, ControlValue>,
    ) -> Result<(), DomainError> {
        loop {
            let preference = {
                let store = self.authorities.preferences.read().await;
                let Some(preference) = store.get(device_id).cloned() else {
                    return Err(DomainError::not_found(
                        ResourceKind::Zone,
                        format!("default-face:{device_id}"),
                    ));
                };
                preference
            };
            let admitted = admit_current_display_face_controls(
                &self.authorities.effects,
                preference.effect_id,
                controls,
            )
            .await?;
            let (_effect, normalized_controls, admission) = admitted.into_parts();
            let updated = {
                let mut store = self.authorities.preferences.write().await;
                store.merge_controls_if_unchanged(device_id, &preference, &normalized_controls)
            };
            drop(admission);
            match updated {
                Ok(true) => {
                    if let Some(zone) = self.apply_preference_overlay(device_id).await {
                        let scene_id = self.active_scene_id().await;
                        self.publish_overlay_change(
                            scene_id,
                            &zone,
                            ZoneChangeKind::ControlsPatched,
                        );
                    }
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) => {
                    return Err(DomainError::Internal(anyhow::anyhow!(
                        "Failed to prepare display preference persistence: {error}"
                    )));
                }
            }
        }
    }

    /// Install (or refresh) the runtime default zone for one display from
    /// its stored preference.
    ///
    /// Removes the overlay when the preference is gone or its effect no
    /// longer resolves. Returns the installed zone, if any.
    pub async fn apply_preference_overlay(&self, device_id: DeviceId) -> Option<Zone> {
        loop {
            let admission = self.authorities.effects.admit_current().await;
            let candidate = {
                let store = self.authorities.preferences.read().await;
                store.get(device_id).cloned()
            };
            let Some(mut preference) = candidate else {
                let store = self.authorities.preferences.read().await;
                if store.get(device_id).is_some() {
                    continue;
                }
                let retracted = self.retract_preference_overlay(device_id).await;
                drop(store);
                drop(admission);
                return retracted;
            };

            let registry = self.authorities.devices.device_registry();
            let tracked = registry.get(&device_id).await;
            let surface = tracked
                .as_ref()
                .and_then(|tracked| display_surface_info(&tracked.info));
            let resolved = resolve_display_face_controls_under_admission(
                &self.authorities.effects,
                &admission,
                preference.effect_id,
                &preference.controls,
            )
            .await;
            let store = self.authorities.preferences.read().await;
            if store.get(device_id) != Some(&preference) {
                drop(store);
                drop(admission);
                continue;
            }
            let (Some(tracked), Some(surface)) = (tracked, surface) else {
                let retracted = self.retract_preference_overlay(device_id).await;
                drop(store);
                drop(admission);
                return retracted;
            };
            let (effect, controls) = match resolved {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        %device_id,
                        effect_id = %preference.effect_id,
                        "Default display face is no longer admissible; skipping overlay"
                    );
                    let retracted = self.retract_preference_overlay(device_id).await;
                    drop(store);
                    drop(admission);
                    return retracted;
                }
            };
            preference.controls = controls;

            let zone = build_default_display_zone(
                device_id,
                tracked.info.name.as_str(),
                effect.id,
                &preference,
                display_face_layout(device_id, tracked.info.name.as_str(), surface),
            );
            let installed =
                set_default_display_overlay(&self.authorities.scene, device_id, zone).await;
            drop(store);
            drop(admission);
            return match installed {
                Ok(installed) => installed,
                Err(error) => {
                    tracing::warn!(%error, %device_id, "Failed to install the default face overlay");
                    None
                }
            };
        }
    }

    /// Drop a display's runtime default overlay, reporting `None` either
    /// way so the caller reads it as "no overlay is installed".
    async fn retract_preference_overlay(&self, device_id: DeviceId) -> Option<Zone> {
        if let Err(error) = remove_default_display_overlay(&self.authorities.scene, device_id).await
        {
            tracing::warn!(%error, %device_id, "Failed to retract the default face overlay");
        }
        None
    }

    /// Reconcile every stored default-face overlay with the preference
    /// store, so defaults follow devices as they appear.
    pub async fn sync_preference_overlays(&self) {
        let device_ids = {
            let store = self.authorities.preferences.read().await;
            store
                .iter()
                .map(|(device_id, _)| device_id)
                .collect::<Vec<_>>()
        };
        for device_id in device_ids {
            self.apply_preference_overlay(device_id).await;
        }
    }

    /// Tell the scene manager which screens the rig knows, and convert the
    /// active scene's stored display zones whose screens are now known.
    pub async fn sync_connected_surfaces(&self) {
        if let Err(error) = sync_known_screens(&self.authorities.scene).await {
            tracing::warn!(%error, "Failed to sync known screens");
        }
    }
}

/// Materialize restored preferences into a prepared scene candidate.
///
/// The restore coordinator holds the scene, device, preference, and effect
/// authorities through publication. Use those prepared values without
/// reacquiring live authority or publishing overlays ahead of the transaction.
pub(crate) fn restore_preference_overlays(
    scenes: &mut SceneManager,
    preferences: &DisplayPreferencesStore,
    devices: &HashMap<DeviceId, DeviceInfo>,
) {
    for (id, info) in devices {
        let Some(preference) = preferences.get(*id) else {
            scenes.remove_default_display_zone(*id);
            continue;
        };
        let Some(surface) = display_surface_info(info) else {
            continue;
        };
        scenes.set_default_display_zone(build_default_display_zone(
            *id,
            &info.name,
            preference.effect_id,
            preference,
            display_face_layout(*id, &info.name, surface),
        ));
    }
}

/// Build the runtime-only default zone a preference materializes into.
fn build_default_display_zone(
    device_id: DeviceId,
    device_name: &str,
    effect_id: EffectId,
    preference: &DisplayPreference,
    layout: SpatialLayout,
) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: format!("{device_name} Face"),
        description: Some(format!("Default face for {device_name}")),
        layers: vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            preference.controls.clone(),
            HashMap::new(),
            None,
        )],
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(
            DisplayFaceTarget {
                blend_mode: preference.blend_mode,
                device_id,
                opacity: preference.opacity,
            }
            .normalized(),
        ),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}
