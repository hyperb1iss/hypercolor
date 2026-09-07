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
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::layer::{BlendMode, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tokio::sync::RwLock;

use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::{DisplayPreference, DisplayPreferencesStore};
use crate::domain::commit::SceneCommit;
use crate::domain::context::{DeviceContext, SceneContext};
use crate::domain::effect::{
    AdmittedEffectControls, EffectContext, EffectMutationAdmission, ResolvedEffect,
};
use crate::domain::layout::LayoutContext;
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
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: format!("{device_name} Display Face"),
        description: Some(format!("Native-resolution face canvas for {device_name}")),
        canvas_width: surface.width,
        canvas_height: surface.height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
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
    /// The face resolved against one catalog generation.
    pub effect: ResolvedEffect,
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
    pub blend_mode: Option<BlendMode>,
    /// The new opacity, when the caller named one.
    pub opacity: Option<f32>,
}

/// Merge control overrides into a display's face zone.
#[derive(Debug, Clone)]
pub struct PatchDisplayFaceControls {
    /// Which display zone to patch.
    pub zone_id: ZoneId,
    /// Canonical control values to validate against the assigned face.
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

/// Assign a face to a display in the active scene.
///
/// # Errors
///
/// [`DomainError::Conflict`] when the active scene is snapshot-locked,
/// [`DomainError::Internal`] when the scene refuses the display zone,
/// and [`DomainError::Conflict`] when a concurrent scene
/// mutation lands first.
pub async fn set_display_face(
    ctx: &EffectContext,
    command: SetDisplayFace,
) -> Result<DisplayZoneWritten, DomainError> {
    let admitted = admit_display_face_controls(ctx, command.effect, &command.controls).await?;
    let (effect, controls, _admission) = admitted.into_parts();
    let scene = ctx.scene_context();
    let mut mutation = scene.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("assigning a display face")?;
    let change = if scene_display_zone(&mutation, command.device_id).is_some() {
        ZoneChangeKind::Updated
    } else {
        ZoneChangeKind::Created
    };

    let zone = mutation.upsert_display_zone(
        command.device_id,
        &command.device_name,
        &effect,
        controls,
        command.layout,
        command.target,
    )?;
    let commit = scene.commit(mutation).await?;
    scene.save_runtime_session().await;

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
    ctx: &EffectContext,
    command: PatchDisplayFaceControls,
) -> Result<Option<DisplayControlsWritten>, DomainError> {
    let scene = ctx.scene_context();
    let mut mutation = scene.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("updating display face controls")?;
    let Some(effect_id) = mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.zones.iter().find(|zone| zone.id == command.zone_id))
        .and_then(|zone| zone.effect_ids().next())
    else {
        return Ok(None);
    };
    let admitted = admit_current_display_face_controls(ctx, effect_id, &command.controls).await?;
    let (effect, controls, _admission) = admitted.into_parts();
    let Some(zone) = mutation.patch_zone_controls(command.zone_id, controls) else {
        return Ok(None);
    };

    let commit = scene.commit(mutation).await?;
    scene.save_runtime_session().await;

    Ok(Some(DisplayControlsWritten {
        effect,
        written: DisplayZoneWritten {
            scene_id,
            zone,
            change: ZoneChangeKind::ControlsPatched,
            commit,
        },
    }))
}

/// Keep every connected display's scene zone aligned with the device's
/// geometry, and report whether anything moved.
///
/// A scene that forbids runtime rewriting is left alone rather than
/// refused: surface sync runs on startup, device connection, and scene
/// activation, so it must be a no-op on a snapshot scene rather than an
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
            let changed = mutation.sync_active_display_surfaces(&displays);
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

// ── Default-face context ─────────────────────────────────────────────────

/// Which layers currently carry a face for one display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayFaceLayers {
    /// The active scene's display zone carries an assignment.
    pub scene_assigned: bool,
    /// The preference store carries a default face.
    pub default_assigned: bool,
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
    let mut target = target.normalized();
    if !target.clone().blends_with_effect() {
        target.opacity = 1.0;
    }
    target
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
    layout: LayoutContext,
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
        layout: LayoutContext,
        devices: DeviceContext,
        event_bus: Arc<HypercolorBus>,
    ) -> Self {
        Self {
            authorities: Arc::new(DisplayAuthorities {
                preferences,
                frames,
                scene,
                effects,
                layout,
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
        let scene_assigned = {
            let scenes = self.authorities.scene.snapshot().await;
            scenes
                .active_scene()
                .and_then(|scene| scene.display_zone_for(device_id))
                .is_some_and(display_zone_has_face_assignment)
        };
        DisplayFaceLayers {
            scene_assigned,
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

    /// Materialize every connected display's editable scene surface and keep
    /// its native geometry current.
    pub async fn sync_connected_surfaces(&self) {
        let displays = self
            .authorities
            .layout
            .connected_display_surface_layouts(&self.authorities.devices.layout_runtime())
            .await;
        if let Err(error) =
            hydrate_existing_display_surfaces(&self.authorities.scene, displays.clone()).await
        {
            tracing::warn!(%error, "Failed to hydrate connected display surfaces");
        }
        if let Err(error) = sync_display_surfaces(&self.authorities.scene, displays).await {
            tracing::warn!(%error, "Failed to sync connected display surfaces");
        }
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

/// Whether a display zone carries a face assignment at all.
fn display_zone_has_face_assignment(zone: &Zone) -> bool {
    zone.effect_ids().next().is_some()
}
