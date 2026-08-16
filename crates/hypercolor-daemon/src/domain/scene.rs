//! Scene domain services: the owned-candidate mutation API, the commit
//! that admits it, and scene activation (Spec 76 §2.3).
//!
//! The mutation model matches the persistence layer's generation-based
//! convergence rather than fighting it. A [`SceneMutation`] is an owned
//! candidate: it is cloned out under a brief read lock, mutated with no
//! lock held at all, and either committed or dropped. Dropping one
//! discards a local candidate — there is nothing to roll back, because
//! nothing global ever changed.
//!
//! [`commit_scene`] is where the candidate becomes real. It takes the
//! scene write lock, compares the candidate's base revision against the
//! live one, installs the candidate, admits the snapshot bytes, and
//! releases the lock — all without an await inside the guard. Only then
//! does it persist and publish. `Err` therefore means one thing and one
//! thing only: the mutation was rejected *before* admission. Everything
//! that can happen after admission is a [`CommitDurability`] on the
//! returned [`SceneCommit`], because after admission the retry
//! supervisor owns the bytes and the mutation is going to land.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::{
    ControlsVersionMismatch, LayerMutationError, OutputPlacement, SceneGroupLayerInsert,
    SceneManager, ZoneMetaPatch, ZoneMutationError, default_primary_group,
};
use hypercolor_types::asset::AssetId;
use hypercolor_types::config::MediaConfig;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectId, EffectMetadata};
use hypercolor_types::event::{
    HypercolorEvent, SceneChangeReason, SceneLibraryChangeKind, ZoneChangeKind,
};
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceBlendMode, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, Zone, ZoneId,
};
use hypercolor_types::spatial::{Output, SpatialLayout};

use crate::api::AppState;
use crate::api::scenes::MediaAdmissionViolationDetails;
use crate::domain::commit::{CommitDurability, SceneCommit, SceneRevision};
use crate::domain::{DomainError, MutationContext, ResourceKind};
use crate::persistence::AtomicWriteOutcome;

// ── Owned candidate ──────────────────────────────────────────────────────

/// An owned candidate scene state, its base revision, and the events its
/// intent methods recorded.
///
/// Nothing here is shared. The candidate is a full [`SceneManager`]
/// clone, so intent methods are ordinary `&mut self` calls with no
/// locking, and abandoning the mutation costs a drop.
///
/// The pristine `base` is kept beside the candidate as the comparand
/// for the divergence check in [`commit_scene`]. It costs a second
/// clone per mutation, paid at commit rate rather than frame rate.
#[derive(Debug)]
pub struct SceneMutation {
    base: SceneManager,
    candidate: SceneManager,
    base_revision: SceneRevision,
    events: Vec<HypercolorEvent>,
    persists_scene_content: bool,
}

impl SceneMutation {
    /// The revision this candidate was snapshotted from.
    #[must_use]
    pub const fn base_revision(&self) -> SceneRevision {
        self.base_revision
    }

    /// Read the candidate. Every intent method's effect is visible here
    /// immediately, so callers compose reads and writes freely.
    #[must_use]
    pub const fn scenes(&self) -> &SceneManager {
        &self.candidate
    }

    /// Record an event for ordered publication at commit time.
    ///
    /// Events never publish from a mutation that is dropped, and never
    /// publish out of admission order.
    pub fn record(&mut self, event: HypercolorEvent) {
        self.events.push(event);
    }

    /// The active scene's id, refusing scenes that forbid runtime
    /// rewriting.
    ///
    /// Snapshot scenes are a deliberate user choice: runtime effect and
    /// face actions must not silently edit them.
    pub fn active_scene_for_runtime_mutation(&self, action: &str) -> Result<SceneId, DomainError> {
        let active = self
            .candidate
            .active_scene()
            .ok_or_else(|| DomainError::Internal(anyhow::anyhow!("No active scene available")))?;
        if active.blocks_runtime_mutation() {
            return Err(DomainError::conflict(format!(
                "Active scene '{}' is in snapshot mode; return to Default or deactivate it before {action}",
                active.name
            )));
        }
        Ok(active.id)
    }

    /// The active scene's primary zone id, when it has one.
    #[must_use]
    pub fn primary_zone_id(&self) -> Option<ZoneId> {
        self.candidate
            .active_scene()
            .and_then(Scene::primary_group)
            .map(|zone| zone.id)
    }

    /// The effect currently loaded in one of the active scene's zones.
    #[must_use]
    pub fn zone_effect(&self, zone_id: ZoneId) -> Option<hypercolor_types::effect::EffectId> {
        self.candidate
            .active_scene()?
            .groups
            .iter()
            .find(|zone| zone.id == zone_id)
            .and_then(|zone| zone.effect_id)
    }

    /// Load an effect into the active scene's primary zone, creating the
    /// zone when the scene has none.
    pub fn upsert_primary_zone(
        &mut self,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
        layout: SpatialLayout,
    ) -> Result<Zone, DomainError> {
        let zone = self
            .candidate
            .upsert_primary_group(metadata, controls, preset_id, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!(
                    "Failed to update active scene primary group: {error}"
                ))
            })?
            .clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Load an effect into a named zone, which keeps its own layout.
    pub fn apply_effect_to_zone(
        &mut self,
        zone_id: ZoneId,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
    ) -> Result<Zone, DomainError> {
        let zone = self
            .candidate
            .apply_effect_to_group(zone_id, metadata, controls, preset_id)
            .map_err(|error| {
                DomainError::validation(format!("Failed to apply effect to zone: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Make a scene the exclusive current one.
    ///
    /// Activation moves the priority stack and the transition state,
    /// neither of which is persisted scene content, so this intent does
    /// not arm a scene-store write.
    pub fn activate(
        &mut self,
        scene_id: SceneId,
        transition: Option<TransitionSpec>,
    ) -> Result<(), DomainError> {
        self.candidate
            .activate(&scene_id, transition)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to activate scene: {error}"))
            })
    }

    /// Return to the synthesized default scene.
    ///
    /// Like [`Self::activate`], this moves only the priority stack.
    pub fn deactivate_current(&mut self) {
        self.candidate.deactivate_current();
    }

    // ── Scene library ────────────────────────────────────────────────

    /// Add a scene to the library.
    pub fn create_scene(&mut self, scene: Scene) -> Result<(), DomainError> {
        self.candidate
            .create(scene)
            .map_err(|error| DomainError::conflict(format!("Failed to create scene: {error}")))?;
        self.persists_scene_content = true;
        Ok(())
    }

    /// Replace a scene's stored definition.
    pub fn update_scene(&mut self, scene: Scene) -> Result<(), DomainError> {
        self.candidate.update(scene).map_err(|error| {
            DomainError::Internal(anyhow::anyhow!("Failed to update scene: {error}"))
        })?;
        self.persists_scene_content = true;
        Ok(())
    }

    /// Remove a scene from the library.
    pub fn delete_scene(&mut self, scene_id: &SceneId) -> Result<Scene, DomainError> {
        let scene = self
            .candidate
            .delete(scene_id)
            .map_err(|error| DomainError::not_found(ResourceKind::Scene, error))?;
        self.persists_scene_content = true;
        Ok(scene)
    }

    // ── Zones ────────────────────────────────────────────────────────

    /// Add a custom zone to a scene.
    pub fn create_zone(
        &mut self,
        scene_id: SceneId,
        name: String,
        color: Option<String>,
        fallback_canvas: (u32, u32),
    ) -> Result<ZoneId, ZoneMutationError> {
        let zone_id =
            self.candidate
                .create_render_group(&scene_id, name, color, fallback_canvas)?;
        self.persists_scene_content = true;
        Ok(zone_id)
    }

    /// Patch a zone's presentation metadata.
    pub fn update_zone_meta(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        patch: ZoneMetaPatch,
    ) -> Result<Zone, ZoneMutationError> {
        let zone = self
            .candidate
            .update_render_group_meta(&scene_id, zone_id, patch)?;
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Remove a custom zone from a scene.
    pub fn delete_zone(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
    ) -> Result<(), ZoneMutationError> {
        self.candidate.delete_render_group(&scene_id, zone_id)?;
        self.persists_scene_content = true;
        Ok(())
    }

    /// Move one output into a zone.
    pub fn assign_output(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        output: Output,
        placement: OutputPlacement,
    ) -> Result<(), ZoneMutationError> {
        self.candidate
            .assign_device_zone(&scene_id, zone_id, output, placement)?;
        self.persists_scene_content = true;
        Ok(())
    }

    /// Drop one output out of whatever zone holds it.
    pub fn unassign_output(
        &mut self,
        scene_id: SceneId,
        output_id: &str,
    ) -> Result<(), ZoneMutationError> {
        self.candidate.unassign_device_zone(&scene_id, output_id)?;
        self.persists_scene_content = true;
        Ok(())
    }

    /// Reposition a zone's outputs without changing which it owns.
    pub fn set_zone_layout(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layout: SpatialLayout,
    ) -> Result<Zone, ZoneMutationError> {
        let zone = self
            .candidate
            .update_zone_layout(&scene_id, zone_id, layout)?;
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Choose what a scene does with outputs no zone claims.
    pub fn set_unassigned_behavior(
        &mut self,
        scene_id: SceneId,
        behavior: UnassignedBehavior,
    ) -> Result<UnassignedBehavior, ZoneMutationError> {
        let behavior = self
            .candidate
            .set_unassigned_behavior(&scene_id, behavior)?;
        self.persists_scene_content = true;
        Ok(behavior)
    }

    // ── Effect slots and controls ────────────────────────────────────

    /// Unload whatever effect a zone runs.
    pub fn clear_zone_effect(&mut self, zone_id: ZoneId) -> Option<Zone> {
        let zone = self.candidate.clear_group_effect(zone_id).cloned()?;
        self.persists_scene_content = true;
        Some(zone)
    }

    /// Merge control overrides into a zone, refusing a stale version and
    /// a zone that stopped running the expected effect.
    pub fn patch_effect_controls(
        &mut self,
        zone_id: ZoneId,
        expected_effect_id: Option<EffectId>,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(Zone, u64), ControlsVersionMismatch> {
        let (zone, version) = self.candidate.patch_effect_controls_with_precondition(
            zone_id,
            expected_effect_id,
            updates,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok((zone, version))
    }

    /// Merge control overrides into a zone with no effect precondition.
    pub fn patch_zone_controls(
        &mut self,
        zone_id: ZoneId,
        updates: HashMap<String, ControlValue>,
    ) -> Option<Zone> {
        let zone = self
            .candidate
            .patch_group_controls(zone_id, updates)?
            .clone();
        self.persists_scene_content = true;
        Some(zone)
    }

    /// Replace a zone's control values with the effect's defaults.
    pub fn reset_zone_controls(
        &mut self,
        zone_id: ZoneId,
        defaults: HashMap<String, ControlValue>,
    ) -> Option<Zone> {
        let zone = self
            .candidate
            .reset_group_controls(zone_id, defaults)?
            .clone();
        self.persists_scene_content = true;
        Some(zone)
    }

    /// Attach a live sensor binding to one of a zone's controls.
    pub fn set_zone_control_binding(
        &mut self,
        zone_id: ZoneId,
        control_id: String,
        binding: ControlBinding,
    ) -> Option<Zone> {
        let zone = self
            .candidate
            .set_group_control_binding(zone_id, control_id, binding)?
            .clone();
        self.persists_scene_content = true;
        Some(zone)
    }

    /// Record which preset a zone's control values came from.
    pub fn set_zone_preset_id(&mut self, zone_id: ZoneId, preset_id: Option<PresetId>) -> bool {
        let changed = self
            .candidate
            .set_group_preset_id(zone_id, preset_id)
            .is_some();
        if changed {
            self.persists_scene_content = true;
        }
        changed
    }

    /// Force the active scene's resolved zones to be recomputed.
    ///
    /// The resolved zones are derived state, so this bumps the
    /// render-group revision without touching persisted scene content.
    pub fn invalidate_active_zones(&mut self) {
        self.candidate.invalidate_active_render_groups();
    }

    // ── Layer stacks ─────────────────────────────────────────────────

    /// Insert a layer into a zone's stack.
    pub fn insert_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer: SceneLayer,
        index: Option<usize>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.insert_scene_group_layer(
            scene_id,
            zone_id,
            layer,
            index,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Insert one layer into each of several zones, all or nothing.
    pub fn insert_layers(
        &mut self,
        scene_id: SceneId,
        inserts: Vec<SceneGroupLayerInsert>,
    ) -> Result<Vec<Zone>, LayerMutationError> {
        let zones = self
            .candidate
            .insert_scene_group_layers_batch(scene_id, inserts)?;
        self.persists_scene_content = true;
        Ok(zones)
    }

    /// Replace one layer's definition in place.
    pub fn update_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.update_scene_group_layer(
            scene_id,
            zone_id,
            layer_id,
            layer,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Drop one layer out of a zone's stack.
    pub fn remove_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.remove_scene_group_layer(
            scene_id,
            zone_id,
            layer_id,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Rewrite a zone's layer order.
    pub fn reorder_layers(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_ids: Vec<SceneLayerId>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.reorder_scene_group_layers(
            scene_id,
            zone_id,
            layer_ids,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Merge control overrides into one effect layer.
    pub fn patch_layer_controls(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.patch_scene_layer_effect_controls(
            scene_id,
            zone_id,
            layer_id,
            updates,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    // ── Display zones ────────────────────────────────────────────────

    /// Assign a face to a display in the active scene, creating the
    /// display zone when the scene has none for that device.
    pub fn upsert_display_zone(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        layout: SpatialLayout,
    ) -> Result<Zone, DomainError> {
        let zone = self
            .candidate
            .upsert_display_group(device_id, device_name, effect, controls, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to update active scene: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Keep a display zone's surface aligned with the device's geometry.
    ///
    /// Returns whether the zone actually moved, so callers can skip a
    /// commit that would change nothing.
    pub fn ensure_display_surface(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<bool, DomainError> {
        let before = self.active_zones_revision();
        self.candidate
            .ensure_display_group_surface(device_id, device_name, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!(
                    "Failed to sync display screen surface: {error}"
                ))
            })?;
        let changed = self.active_zones_revision() != before;
        if changed {
            self.persists_scene_content = true;
        }
        Ok(changed)
    }

    /// Update how a display zone's face composes over the effect layer.
    pub fn patch_display_target(
        &mut self,
        zone_id: ZoneId,
        blend_mode: Option<DisplayFaceBlendMode>,
        opacity: Option<f32>,
    ) -> Option<Zone> {
        let zone = self
            .candidate
            .patch_display_group_target(zone_id, blend_mode, opacity)?
            .clone();
        self.persists_scene_content = true;
        Some(zone)
    }

    /// Strip the face assignment off a display zone, keeping the zone.
    pub fn clear_display_assignment(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<Zone, DomainError> {
        let zone = self
            .candidate
            .clear_display_group_assignment(device_id, device_name, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to update active scene: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        Ok(zone)
    }

    /// Drop every scene's display zone for one device.
    pub fn remove_display_zones_for_device(&mut self, device_id: DeviceId) -> Vec<(SceneId, Zone)> {
        let removed = self.candidate.remove_display_groups_for_device(device_id);
        if !removed.is_empty() {
            self.persists_scene_content = true;
        }
        removed
    }

    /// Install the runtime overlay zone a display preference resolves to.
    ///
    /// Default display zones are materialized from the preference store
    /// on every run, so they are runtime state rather than persisted
    /// scene content.
    pub fn set_default_display_zone(&mut self, zone: Zone) {
        self.candidate.set_default_display_group(zone);
    }

    /// Remove a display's runtime default overlay zone.
    pub fn remove_default_display_zone(&mut self, device_id: DeviceId) -> Option<Zone> {
        let existing = self.candidate.default_display_group_for(device_id).cloned();
        self.candidate.remove_default_display_group(device_id);
        existing
    }

    fn active_zones_revision(&self) -> u64 {
        self.candidate
            .active_scene()
            .map_or(0, |scene| scene.groups_revision)
    }
}

impl AppState {
    /// Snapshot the live scene state into an owned candidate.
    ///
    /// The read lock is held for exactly one clone. Everything the
    /// caller does afterwards happens on its own copy.
    pub async fn begin_scene_mutation(&self) -> SceneMutation {
        let manager = self.scene_manager.read().await;
        let base_revision = self.scene_commits.revision();
        SceneMutation {
            base: manager.clone(),
            candidate: manager.clone(),
            base_revision,
            events: Vec::new(),
            persists_scene_content: false,
        }
    }
}

// ── Commit ───────────────────────────────────────────────────────────────

/// Install a candidate, admit its snapshot, then persist and publish.
///
/// The compare-and-swap on the base revision refuses a candidate built
/// from a revision that no longer exists, rather than letting it
/// silently overwrite whatever landed in between.
///
/// **The swap only sees commits.** The revision advances in
/// [`SceneCommitSequencer::admit`], which nothing but this function
/// calls, so a candidate is protected against another `commit_scene`
/// and against nothing else. Roughly fifty other sites still take
/// `scene_manager.write()` directly and mutate live scene state without
/// touching the revision — the zone, layer, display, preset, profile,
/// and layout-transaction paths that wave 2.3b migrates. Because the
/// install below replaces the whole manager, a direct write that lands
/// inside a candidate's window is discarded with no error to either
/// caller. Every such site must route through here before the swap is
/// a general guarantee rather than a guarantee between commits.
///
/// # Errors
///
/// Only pre-admission rejections: a stale base revision
/// ([`DomainError::PreconditionFailed`]) or a snapshot that will not
/// serialize ([`DomainError::Internal`]). Once the bytes are admitted
/// the mutation is committed, and where they ended up is reported by
/// [`SceneCommit::durability`].
pub async fn commit_scene(
    state: &AppState,
    mutation: SceneMutation,
) -> Result<SceneCommit, DomainError> {
    let SceneMutation {
        base,
        candidate,
        base_revision,
        events,
        persists_scene_content,
    } = mutation;

    // Only a mutation that changes persisted scene content needs the
    // store handle, and cloning it is a deep copy plus a yield point
    // that would widen every activation's swap window for nothing.
    let coordinator = if persists_scene_content {
        Some(state.scene_store.read().await.clone())
    } else {
        None
    };

    let (ticket, pending) = {
        let mut manager = state.scene_manager.write().await;
        let current_revision = state.scene_commits.revision();
        if current_revision != base_revision {
            return Err(DomainError::PreconditionFailed {
                resource: ResourceKind::Scene,
                expected: base_revision,
                current: current_revision,
            });
        }

        // TEMPORARY BRIDGE — remove in wave 2.3b.
        //
        // The revision above only moves on commit, so it cannot see the
        // scene writers that still take `scene_manager.write()`
        // directly. Since the install below replaces the whole manager,
        // one of those landing inside this candidate's window would be
        // discarded with no error to anyone. Comparing the live state
        // against the pristine base turns that silent lost update into
        // a retryable conflict.
        //
        // Wave 2.3b routes the last direct writer through here, at
        // which point the revision is the only mutation path and this
        // check reduces to always-true. Delete it then.
        if !scene_state_matches(&manager, &base) {
            return Err(DomainError::conflict(
                "Scene state changed underneath this request; a concurrent writer \
                 modified it. Retry against current state.",
            ));
        }

        let previous = std::mem::replace(&mut *manager, candidate);
        let pending = if let Some(coordinator) = coordinator.as_ref() {
            match coordinator.reserve_save(manager.list().into_iter().cloned()) {
                Ok(pending) => Some(pending),
                Err(error) => {
                    // Serialization failed, so nothing was admitted and
                    // the candidate never happened.
                    *manager = previous;
                    return Err(DomainError::Internal(anyhow::anyhow!(
                        "Failed to persist scene: {error}"
                    )));
                }
            }
        } else {
            None
        };

        // The generation is assigned under the same guard that installed
        // the candidate, so admission order and revision order agree.
        let ticket = state.scene_commits.admit(Arc::clone(&state.event_bus));
        (ticket, pending)
    };

    let generation = ticket.generation();

    let Some(pending) = pending else {
        // Nothing persisted scene content, so there is nothing that
        // could be superseded or retried.
        ticket.release(events);
        return Ok(SceneCommit::new(
            generation,
            generation,
            CommitDurability::Written,
            None,
        ));
    };

    let outcome = state.scene_store.write().await.save_reserved(pending);
    match outcome {
        Ok(AtomicWriteOutcome::Written) => {
            ticket.release(events);
            Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Written,
                None,
            ))
        }
        Ok(AtomicWriteOutcome::Superseded) => {
            // A newer generation owns the destination. Its payload
            // already contains this commit's changes and its own
            // publication is what subscribers should see, so this
            // commit's announcement would only walk state backwards.
            ticket.discard();
            Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Superseded,
                None,
            ))
        }
        Err(error) => {
            // The bytes stay the destination's newest admitted intent
            // and the retry supervisor converges on them, so this
            // commit is authoritative and announces itself like any
            // other. Withholding the events here would leave every
            // client blind to a change that is going to land, and the
            // web UI is event-driven with no polling to fall back on.
            // `retry_error` carries the attempt's failure for logging.
            let message = format!("{error}");
            ticket.release(events);
            Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Retrying,
                Some(message),
            ))
        }
    }
}

/// Whether two scene managers hold the same scene state, for the
/// divergence bridge above.
///
/// Deliberately narrower than whole-manager equality, and not merely
/// for cost. The render thread takes `scene_manager.write()` on every
/// frame of a running transition to call `tick_transition`
/// (`render_thread/scene_snapshot.rs`), so any comparand including
/// transition progress would report divergence on nearly every commit
/// made during a transition — which is exactly when `activate_scene`
/// runs. Progress is render-local and no commit means to own it.
///
/// Everything else a candidate replaces is compared, and each member
/// exists because some direct writer can move it alone:
///
/// - the scene set, as a map, for scene content;
/// - the **whole** priority stack as an ordered `(scene, priority)`
///   sequence, not just its winner, because a mutation to a shadowed
///   entry leaves the active scene identical while still being work
///   this install would erase;
/// - the render-group revision as a **number**, because a direct
///   `invalidate_active_render_groups` bumps the counter without
///   necessarily changing the resolved zones, and comparing only the
///   zones would miss it;
/// - the resolved zones themselves, for content;
/// - the runtime default display groups as a full set, because a
///   default hidden behind an assigned display zone never appears in
///   the resolved zones at all.
///
/// `entered_at` is excluded from the stack comparison: it is an
/// `Instant` stamped at push time, so it carries no state a commit
/// could lose.
fn scene_state_matches(live: &SceneManager, base: &SceneManager) -> bool {
    if live.active_scene_id() != base.active_scene_id()
        || live.activation_history() != base.activation_history()
        || live.active_render_groups_revision() != base.active_render_groups_revision()
        || live.active_render_groups().as_ref() != base.active_render_groups().as_ref()
        || live.default_display_groups() != base.default_display_groups()
        || !priority_stacks_match(live, base)
    {
        return false;
    }

    // `list` walks a HashMap, so the order is not stable between calls
    // and the scenes are compared as a set keyed by id.
    let live_scenes = live.list();
    let base_scenes = base.list();
    if live_scenes.len() != base_scenes.len() {
        return false;
    }
    let base_by_id: HashMap<SceneId, &Scene> = base_scenes
        .into_iter()
        .map(|scene| (scene.id, scene))
        .collect();
    live_scenes
        .into_iter()
        .all(|scene| base_by_id.get(&scene.id) == Some(&scene))
}

/// The priority stack as the ordered identity sequence a commit can
/// lose, ignoring the push timestamps.
fn priority_stacks_match(live: &SceneManager, base: &SceneManager) -> bool {
    let live_entries = live.priority_stack().entries();
    let base_entries = base.priority_stack().entries();
    live_entries.len() == base_entries.len()
        && live_entries
            .iter()
            .zip(base_entries)
            .all(|(live, base)| live.scene_id == base.scene_id && live.priority == base.priority)
}

// ── Scene media admission ────────────────────────────────────────────────

/// What activating a scene would cost the compositor, and whether it
/// exceeds the hard producer caps.
#[derive(Debug)]
pub struct SceneMediaAdmission {
    /// Estimated per-frame producer cost in microseconds.
    pub estimated_cost_us: u64,
    /// The hard-cap violation, when the scene has one.
    pub violation: Option<MediaAdmissionViolationDetails>,
}

impl SceneMediaAdmission {
    /// The violation message, when the scene exceeds its caps.
    #[must_use]
    pub fn rejection_message(&self) -> Option<&str> {
        self.violation
            .as_ref()
            .map(|violation| violation.message.as_str())
    }
}

/// Evaluate a scene's media producer admission against live config.
///
/// Both transports render their own frozen shape for a violation, so
/// they call this for the details and then call
/// [`activate_scene`], which enforces the same rule again. The check
/// cannot be skipped by an adapter that forgets it.
pub fn evaluate_scene_media_admission(
    scene: &Scene,
    asset_mime_types: &HashMap<AssetId, String>,
    media_config: &MediaConfig,
) -> SceneMediaAdmission {
    let counts = crate::api::scenes::scene_media_admission_counts(scene, asset_mime_types);
    SceneMediaAdmission {
        estimated_cost_us: counts.estimated_cost_us(),
        violation: crate::api::scenes::scene_media_admission_violation_details(
            &counts,
            media_config,
        ),
    }
}

// ── activate_scene ───────────────────────────────────────────────────────

/// Make a scene the exclusive current one.
#[derive(Debug, Clone)]
pub struct ActivateScene {
    /// Which scene to activate. Adapters resolve names to ids.
    pub scene_id: SceneId,
    /// Overrides the scene's own transition spec when present.
    pub transition: Option<TransitionSpec>,
}

/// The outcome of a scene activation.
#[derive(Debug)]
pub struct SceneActivated {
    /// The scene that is now current.
    pub scene_id: SceneId,
    /// Its name, resolved before activation.
    pub scene_name: String,
    /// Which scene was current before, when one was.
    pub previous_scene_id: Option<SceneId>,
    /// The estimated producer cost that drove soft admission.
    pub estimated_cost_us: u64,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Activate a scene: validate its media admission, switch the exclusive
/// current scene, apply soft admission, then persist and reconcile
/// connectivity.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Validation`] when the scene exceeds its hard media
/// producer caps, and [`DomainError::PreconditionFailed`] when a
/// concurrent scene mutation lands first.
pub async fn activate_scene(
    state: &AppState,
    command: ActivateScene,
    meta: MutationContext,
) -> Result<SceneActivated, DomainError> {
    let _ = meta;

    let asset_mime_types = crate::api::scenes::asset_mime_types(state).await;
    let media_config = crate::api::scenes::current_media_config(state);

    let mut mutation = state.begin_scene_mutation().await;
    let previous_scene_id = mutation.scenes().active_scene_id().copied();

    let scene = mutation
        .scenes()
        .get(&command.scene_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, command.scene_id))?;
    let scene_name = scene.name.clone();
    let admission = evaluate_scene_media_admission(scene, &asset_mime_types, &media_config);
    if let Some(message) = admission.rejection_message() {
        return Err(DomainError::validation(message.to_owned()));
    }

    mutation.activate(command.scene_id, command.transition)?;

    let current_scene = mutation.scenes().active_scene().cloned();
    if previous_scene_id != current_scene.as_ref().map(|scene| scene.id)
        && let Some(current) = current_scene.as_ref()
    {
        mutation.record(active_scene_changed_event(
            previous_scene_id,
            current,
            SceneChangeReason::UserActivate,
        ));
    }

    let commit = commit_scene(state, mutation).await?;

    crate::api::scenes::apply_scene_media_soft_admission(
        state,
        command.scene_id,
        &scene_name,
        admission.estimated_cost_us,
    )
    .await;
    crate::api::save_runtime_session_snapshot(state).await;

    // Which scene is active decides which devices are worth connecting.
    crate::api::sync_connectivity(state).await;

    Ok(SceneActivated {
        scene_id: command.scene_id,
        scene_name,
        previous_scene_id,
        estimated_cost_us: admission.estimated_cost_us,
        commit,
    })
}

// ── Scene library CRUD ───────────────────────────────────────────────────

/// Add a scene to the library.
#[derive(Debug, Clone)]
pub struct CreateScene {
    /// Human-readable name.
    pub name: String,
    /// What the scene does.
    pub description: Option<String>,
    /// Whether the scene is selectable. Defaults to enabled.
    pub enabled: Option<bool>,
    /// Whether runtime effect and face actions may rewrite the scene.
    pub mutation_mode: Option<SceneMutationMode>,
    /// Free-form provenance the adapter wants recorded on the scene.
    pub metadata: HashMap<String, String>,
}

/// Replace a scene's stored definition.
#[derive(Debug, Clone)]
pub struct UpdateScene {
    /// Which scene to rewrite.
    pub scene_id: SceneId,
    /// Its new name.
    pub name: String,
    /// Its new description.
    pub description: Option<String>,
    /// Whether it stays selectable. `None` keeps the current value.
    pub enabled: Option<bool>,
    /// Whether runtime actions may rewrite it. `None` keeps the
    /// current value.
    pub mutation_mode: Option<SceneMutationMode>,
}

/// The outcome of a scene library mutation.
#[derive(Debug)]
pub struct SceneWritten {
    /// The scene as it now stands.
    pub scene: Scene,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of deleting a scene.
#[derive(Debug)]
pub struct SceneDeleted {
    /// The scene that was removed.
    pub scene: Scene,
    /// Which scene is current now, when the deletion changed it.
    pub current_scene: Option<Scene>,
    /// Which scene was current before.
    pub previous_scene_id: Option<SceneId>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of returning to the synthesized default scene.
#[derive(Debug)]
pub struct SceneDeactivated {
    /// The scene that was current, when one was.
    pub previous_scene: Option<Scene>,
    /// The scene that is current now.
    pub current_scene: Option<Scene>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Create a scene, seeded with a Default zone holding the current
/// device output roster.
///
/// Every scene is born with that zone so the Studio scene selector
/// always has one to select (Spec 65 §5.2); the user renames it freely.
///
/// # Errors
///
/// [`DomainError::Conflict`] when the scene cannot be added, and
/// [`DomainError::PreconditionFailed`] when a concurrent scene mutation
/// lands first.
pub async fn create_scene(
    state: &AppState,
    command: CreateScene,
    meta: MutationContext,
) -> Result<SceneWritten, DomainError> {
    let _ = meta;

    let default_layout = crate::api::effects::resolve_full_scope_layout(state).await;
    let scene = Scene {
        id: SceneId::new(),
        name: command.name,
        description: command.description,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: vec![default_primary_group(default_layout)],
        groups_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: command.enabled.unwrap_or(true),
        metadata: command.metadata,
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: command.mutation_mode.unwrap_or(SceneMutationMode::Live),
    };

    let mut mutation = state.begin_scene_mutation().await;
    mutation.create_scene(scene.clone())?;
    mutation.record(HypercolorEvent::SceneLibraryChanged {
        scene_id: scene.id,
        kind: SceneLibraryChangeKind::Created,
        name: Some(scene.name.clone()),
    });
    let commit = commit_scene(state, mutation).await?;

    Ok(SceneWritten { scene, commit })
}

/// Rewrite a scene's name, description, and mode flags.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene, and
/// [`DomainError::PreconditionFailed`] when a concurrent scene mutation
/// lands first.
pub async fn update_scene(
    state: &AppState,
    command: UpdateScene,
    meta: MutationContext,
) -> Result<SceneWritten, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let existing = mutation
        .scenes()
        .get(&command.scene_id)
        .cloned()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, command.scene_id))?;

    let updated = Scene {
        name: command.name,
        description: command.description,
        enabled: command.enabled.unwrap_or(existing.enabled),
        mutation_mode: command.mutation_mode.unwrap_or(existing.mutation_mode),
        ..existing
    };

    mutation.update_scene(updated.clone())?;
    mutation.record(HypercolorEvent::SceneLibraryChanged {
        scene_id: updated.id,
        kind: SceneLibraryChangeKind::Updated,
        name: Some(updated.name.clone()),
    });
    let commit = commit_scene(state, mutation).await?;

    Ok(SceneWritten {
        scene: updated,
        commit,
    })
}

/// Remove a scene from the library, deactivating it first when it is
/// the current one.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Conflict`] for the Default scene, and
/// [`DomainError::PreconditionFailed`] when a concurrent scene mutation
/// lands first.
pub async fn delete_scene(
    state: &AppState,
    scene_id: SceneId,
    meta: MutationContext,
) -> Result<SceneDeleted, DomainError> {
    let _ = meta;

    if scene_id.is_default() {
        return Err(DomainError::conflict("Default scene cannot be deleted"));
    }

    let mut mutation = state.begin_scene_mutation().await;
    let previous_scene_id = mutation.scenes().active_scene_id().copied();
    let scene = mutation.delete_scene(&scene_id)?;
    let current_scene = mutation.scenes().active_scene().cloned();

    if previous_scene_id != current_scene.as_ref().map(|scene| scene.id)
        && let Some(current) = current_scene.as_ref()
    {
        mutation.record(active_scene_changed_event(
            previous_scene_id,
            current,
            SceneChangeReason::UserDeactivate,
        ));
    }
    mutation.record(HypercolorEvent::SceneLibraryChanged {
        scene_id,
        kind: SceneLibraryChangeKind::Deleted,
        name: None,
    });
    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;

    Ok(SceneDeleted {
        scene,
        current_scene,
        previous_scene_id,
        commit,
    })
}

/// Return to the synthesized default scene.
///
/// # Errors
///
/// [`DomainError::PreconditionFailed`] when a concurrent scene mutation
/// lands first.
pub async fn deactivate_scene(
    state: &AppState,
    meta: MutationContext,
) -> Result<SceneDeactivated, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let previous_scene = mutation.scenes().active_scene().cloned();
    mutation.deactivate_current();
    let current_scene = mutation.scenes().active_scene().cloned();

    if previous_scene.as_ref().map(|scene| scene.id) != current_scene.as_ref().map(|scene| scene.id)
        && let Some(current) = current_scene.as_ref()
    {
        mutation.record(active_scene_changed_event(
            previous_scene.as_ref().map(|scene| scene.id),
            current,
            SceneChangeReason::UserDeactivate,
        ));
    }
    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;

    // Which scene is active decides which devices are worth connecting.
    crate::api::sync_connectivity(state).await;

    Ok(SceneDeactivated {
        previous_scene,
        current_scene,
        commit,
    })
}

// ── Shared event helpers ─────────────────────────────────────────────────

/// The active-scene-changed event every activation path records.
#[must_use]
pub fn active_scene_changed_event(
    previous: Option<SceneId>,
    current: &Scene,
    reason: SceneChangeReason,
) -> HypercolorEvent {
    HypercolorEvent::ActiveSceneChanged {
        previous,
        current: current.id,
        current_name: current.name.clone(),
        current_kind: current.kind,
        current_mutation_mode: current.mutation_mode,
        current_snapshot_locked: current.blocks_runtime_mutation(),
        reason,
    }
}

/// The zone-changed event both effect-apply paths record.
#[must_use]
pub fn zone_changed_event(scene_id: SceneId, zone: &Zone, kind: ZoneChangeKind) -> HypercolorEvent {
    HypercolorEvent::RenderGroupChanged {
        scene_id,
        group_id: zone.id,
        role: zone.role,
        kind,
    }
}
