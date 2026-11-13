//! Scene engine — scene lifecycle, transition planning, priority management,
//! and automation rule evaluation.
//!
//! This module is the orchestration layer that sits between the effect
//! pipeline and the event-driven automation system. It manages:
//!
//! - **Scene CRUD** — create, read, update, delete scenes.
//! - **Activation** — activate a scene with a transition, track the active scene.
//! - **Deactivation** — deactivate the current scene, restoring the previous one.
//! - **Transitions** — immutable activation plans via [`TransitionPlan`].
//! - **Priority stacking** — conflict resolution via [`PriorityStack`].
//! - **Automation** — rule evaluation via [`AutomationEngine`].

pub mod automation;
mod display_collisions;
mod layer_scope;
pub mod priority;
pub mod screens;
pub mod transition;

pub use automation::AutomationEngine;
pub use layer_scope::{
    OutputMove, check_introduced_scope, first_lighting_effect, plan_layer_carry,
};
pub use priority::{PriorityStack, StackEntry};
pub use transition::{
    TransitionIdentity, TransitionPlan, interpolate_color, interpolate_oklab, interpolate_srgb,
};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};

use hypercolor_types::api::scene::LayerMove;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlBinding, EffectId, EffectMetadata};
use hypercolor_types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{NormalizedPosition, Output, SpatialLayout};

/// The name a scene's primary zone is born with.
pub const DEFAULT_ZONE_NAME: &str = "Main";
/// The description a scene's primary zone is born with.
pub const DEFAULT_ZONE_DESCRIPTION: &str = "The scene's main zone.";

/// Error variants for precondition-checked layer mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerMutationError {
    /// No scene exists with the requested id.
    SceneMissing,
    /// The active scene exists but no zone with the given id.
    ZoneMissing,
    /// The zone exists but no layer with the given id.
    LayerMissing { layer_id: SceneLayerId },
    /// The requested layer id already exists in the zone.
    DuplicateLayer { layer_id: SceneLayerId },
    /// The internal layer-stack generation is stale.
    Stale { expected: u64, current: u64 },
    /// The supplied layer payload violates layer-stack invariants.
    InvalidLayer { errors: Vec<String> },
    /// The requested insertion index is outside the current layer stack.
    InvalidIndex { index: usize, len: usize },
    /// The supplied order is not an exact permutation of current layer ids.
    InvalidOrder,
    /// The patch writes control keys an input binding already drives.
    ///
    /// A manual write the next sensor resolution would silently
    /// overwrite is an error, not a race. The caller either drops the
    /// key or clears its binding in the same request.
    ControlBound { keys: Vec<String> },
    /// The layer's scope introduces an output its zone does not hold.
    ///
    /// Only ids a request brings in are checked. An id carried over from
    /// the layer being replaced may name an output that has left the zone.
    ScopeOutputMissing {
        layer_id: SceneLayerId,
        output: String,
    },
    /// A replayed layer move no longer matches the stack: the layer is
    /// not at the slot the move names, or its scope has changed since.
    LayerMoveStale { layer_id: SceneLayerId },
}

/// Error variants for structural render-zone mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneMutationError {
    /// No scene exists with the requested id.
    SceneMissing,
    /// No zone exists with the requested id.
    ZoneMissing,
    /// No device zone exists with the requested id.
    OutputMissing,
    /// The scene is snapshot-locked and cannot be structurally edited.
    SnapshotLocked,
    /// The requested mutation is invalid for the zone's role.
    InvalidRole { role: ZoneRole },
    /// A placement update carried an output set that does not match the
    /// zone's stored outputs. Adds and drops route through the device
    /// assignment endpoints, not the layout endpoint.
    LayoutOutputMismatch,
}

/// Presentation fields that can be patched without touching effects,
/// layers, or device assignment.
#[derive(Debug, Clone, Default)]
pub struct ZoneMetaPatch {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub color: Option<Option<String>>,
    pub brightness: Option<f32>,
    pub enabled: Option<bool>,
    pub make_primary: Option<bool>,
    /// Move the zone to this position in the scene's zone order, clamped
    /// to the last slot.
    pub index: Option<usize>,
}

// ── SceneManager ────────────────────────────────────────────────────────

/// Immutable resolved scene plan consumed by the render thread.
///
/// The generation and every field in this value describe one admitted
/// control-plane state. Per-frame clocks and transition progress belong
/// to the renderer and are deliberately absent.
#[derive(Debug, Clone)]
pub struct ScenePlanSnapshot {
    pub generation: u64,
    pub active_scene_id: Option<SceneId>,
    pub active_scene_name: Option<String>,
    pub transition: Option<TransitionPlan>,
    pub zones: Arc<[Zone]>,
    pub zones_revision: u64,
    pub unassigned_behavior: hypercolor_types::scene::UnassignedBehavior,
}

/// Central scene lifecycle manager.
///
/// Owns the scene store, the priority stack, and immutable transition plans.
/// Render-local frame state owns clocks and transition progress.
#[derive(Debug, Clone)]
pub struct SceneManager {
    /// All registered scenes, keyed by [`SceneId`].
    scenes: HashMap<SceneId, Scene>,

    /// Priority stack for active scene arbitration.
    priority_stack: PriorityStack,

    /// Most recently admitted transition plan (if any).
    transition_plan: Option<TransitionPlan>,

    /// Identity source for immutable transition plans.
    transition_epoch: u64,

    /// History of previously active scene IDs, most recent first.
    /// Used for restore-previous semantics.
    activation_history: Vec<SceneId>,

    /// Cached active zones for cheap frame snapshot reads.
    resolved_zones: Arc<[Zone]>,

    /// Monotonic revision for the active render-zone cache.
    resolved_zones_revision: u64,

    /// Runtime-only default face zones, keyed by display device. Merged
    /// into the active render zones whenever the active scene has no
    /// assigned display zone for that device; never written into scenes.
    default_display_zones: Vec<Zone>,

    /// Every known screen, by the layout device id its output carries.
    /// Runtime state: it says which outputs are screens the engine can
    /// drive and at what size, and is never written into a scene.
    screen_surfaces: screens::ScreenSurfaces,
}

impl SceneManager {
    /// Create a new scene manager with no scenes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scenes: HashMap::new(),
            priority_stack: PriorityStack::new(),
            transition_plan: None,
            transition_epoch: 0,
            activation_history: Vec::new(),
            resolved_zones: Arc::default(),
            resolved_zones_revision: 0,
            default_display_zones: Vec::new(),
            screen_surfaces: screens::ScreenSurfaces::new(),
        }
    }

    #[must_use]
    pub fn with_default() -> Self {
        Self::with_default_layout(empty_default_spatial_layout())
    }

    #[must_use]
    pub fn with_default_layout(layout: SpatialLayout) -> Self {
        let mut manager = Self::new();
        manager.install_default_scene(layout);
        manager
    }

    fn install_default_scene(&mut self, layout: SpatialLayout) {
        if self.scenes.contains_key(&SceneId::DEFAULT) {
            return;
        }

        let default = Scene {
            id: SceneId::DEFAULT,
            name: "Default".to_owned(),
            description: Some("Auto-managed default scene.".to_owned()),
            zones: vec![default_primary_zone(layout)],
            zones_revision: 0,
            transition: TransitionSpec {
                duration_ms: 1_000,
                easing: EasingFunction::Linear,
                color_interpolation: ColorInterpolation::Oklab,
            },
            priority: ScenePriority::AMBIENT,
            enabled: true,
            metadata: HashMap::new(),
            unassigned_behavior: hypercolor_types::scene::UnassignedBehavior::Off,
            layout_id: None,
            activation_brightness: None,
            kind: SceneKind::Ephemeral,
            mutation_mode: SceneMutationMode::Live,
        };
        self.scenes.insert(default.id, default);
        self.priority_stack
            .push(SceneId::DEFAULT, ScenePriority::AMBIENT);
        self.refresh_resolved_zones();
    }

    // ── CRUD ────────────────────────────────────────────────────────

    /// Register a new scene. Returns an error if a scene with the same
    /// ID already exists.
    pub fn create(&mut self, scene: Scene) -> Result<()> {
        if self.scenes.contains_key(&scene.id) {
            bail!("scene already exists: {}", scene.id);
        }
        if let Err(errors) = scene.validate() {
            bail!("scene '{}' is invalid: {}", scene.name, errors.join("; "));
        }
        self.scenes.insert(scene.id, scene);
        Ok(())
    }

    /// Retrieve a scene by ID.
    #[must_use]
    pub fn get(&self, id: &SceneId) -> Option<&Scene> {
        self.scenes.get(id)
    }

    /// List all registered scenes.
    #[must_use]
    pub fn list(&self) -> Vec<&Scene> {
        self.scenes.values().collect()
    }

    /// Update an existing scene in-place. Returns an error if the scene
    /// does not exist.
    pub fn update(&mut self, scene: Scene) -> Result<()> {
        let Some(existing) = self.scenes.get(&scene.id) else {
            bail!("scene not found: {}", scene.id);
        };
        if existing.kind != scene.kind {
            bail!("scene kind cannot be changed");
        }
        if scene.id.is_default() && scene.name != existing.name {
            bail!("default scene cannot be renamed");
        }
        if let Err(errors) = scene.validate() {
            bail!("scene '{}' is invalid: {}", scene.name, errors.join("; "));
        }
        let scene_id = scene.id;
        let active_scene_id = self.active_scene_id().copied();
        self.scenes.insert(scene_id, scene);
        if active_scene_id == Some(scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(())
    }

    /// Delete a scene by ID. Also removes it from the priority stack
    /// if it was active. Returns an error if the scene does not exist.
    pub fn delete(&mut self, id: &SceneId) -> Result<Scene> {
        if id.is_default() {
            bail!("cannot delete default scene");
        }

        let scene = self
            .scenes
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("scene not found: {id}"))?;

        self.priority_stack.remove(id);
        self.activation_history.retain(|sid| sid != id);
        self.refresh_resolved_zones();

        Ok(scene)
    }

    /// Number of registered scenes.
    #[must_use]
    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    // ── Activation ──────────────────────────────────────────────────

    /// Activate a scene, pushing it onto the priority stack.
    ///
    /// If a transition spec is provided it overrides the scene's default.
    /// If another scene is currently active, a transition is started
    /// between them.
    pub fn activate(
        &mut self,
        id: &SceneId,
        transition_override: Option<TransitionSpec>,
    ) -> Result<()> {
        let scene = self
            .scenes
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("scene not found: {id}"))?;

        let spec = transition_override.unwrap_or_else(|| scene.transition.clone());
        let priority = scene.priority;
        let to_id = scene.id;

        // Capture from-state before pushing.
        let from_state = self.active_scene_id().copied();
        // Record history.
        if let Some(prev_id) = from_state {
            self.activation_history.insert(0, prev_id);
        }

        self.priority_stack.push(to_id, priority);

        // Start transition if there's a from-scene.
        if let Some(from_id) = from_state {
            if spec.duration_ms > 0 {
                self.transition_epoch = self.transition_epoch.saturating_add(1);
                self.transition_plan = Some(TransitionPlan::new(
                    self.transition_epoch,
                    from_id,
                    to_id,
                    spec,
                ));
            } else {
                // Instant activation — no transition.
                self.transition_plan = None;
            }
        } else {
            self.transition_plan = None;
        }

        self.refresh_resolved_zones();

        Ok(())
    }

    /// Deactivate the currently active scene, restoring the previous one.
    ///
    /// If there is no active scene, this is a no-op.
    pub fn deactivate_current(&mut self) {
        if self.priority_stack.len() == 1 && self.active_scene_id().is_some_and(SceneId::is_default)
        {
            return;
        }

        let popped = self.priority_stack.pop();
        if let Some(entry) = popped {
            // If there was a previous scene in history, try to restore it.
            // The priority stack already exposes the next entry via peek().
            // We also clear the transition since we're switching instantly.
            self.transition_plan = None;

            // Remove from history if present.
            self.activation_history.retain(|sid| *sid != entry.scene_id);
        }

        self.refresh_resolved_zones();
    }

    /// Get the currently active scene ID (top of the priority stack).
    #[must_use]
    pub fn active_scene_id(&self) -> Option<&SceneId> {
        self.priority_stack.peek().map(|e| &e.scene_id)
    }

    /// Get the currently active scene.
    #[must_use]
    pub fn active_scene(&self) -> Option<&Scene> {
        self.active_scene_id().and_then(|id| self.scenes.get(id))
    }

    /// Get the cached active zones for cheap frame snapshots.
    #[must_use]
    pub fn resolved_zones(&self) -> Arc<[Zone]> {
        Arc::clone(&self.resolved_zones)
    }

    /// Monotonic revision of the cached active zones.
    #[must_use]
    pub fn resolved_zones_revision(&self) -> u64 {
        self.resolved_zones_revision
    }

    /// Capture one commit-stable resolved plan for lock-free frame work.
    #[must_use]
    pub fn plan_snapshot(&self, generation: u64) -> ScenePlanSnapshot {
        ScenePlanSnapshot {
            generation,
            active_scene_id: self.active_scene_id().copied(),
            active_scene_name: self.active_scene().map(|scene| scene.name.clone()),
            transition: self.transition_plan.clone(),
            zones: self.resolved_zones(),
            zones_revision: self.resolved_zones_revision,
            unassigned_behavior: self
                .active_scene()
                .map(|scene| scene.unassigned_behavior.clone())
                .unwrap_or_default(),
        }
    }

    /// Invalidate caches derived from the active zones when an
    /// external dependency changes without mutating the scene graph itself.
    pub fn invalidate_resolved_zones(&mut self) {
        self.resolved_zones_revision = self.resolved_zones_revision.saturating_add(1);
    }

    /// Replace effect identities throughout authored and runtime-only scene state.
    ///
    /// The render-zone revision advances even when only an inactive scene
    /// changed, fencing prepared layout publications captured before the
    /// identity rewrite.
    pub fn remap_effect_ids(&mut self, migrations: &HashMap<EffectId, EffectId>) -> usize {
        let mut migrated = 0;
        for scene in self.scenes.values_mut() {
            let mut scene_changed = false;
            for zone in &mut scene.zones {
                let zone_migrated = remap_zone_effect_ids(zone, migrations);
                if zone_migrated > 0 {
                    zone.layers_version = zone.layers_version.saturating_add(1);
                    scene_changed = true;
                    migrated += zone_migrated;
                }
            }
            if scene_changed {
                bump_zones_revision(scene);
            }
        }
        for zone in &mut self.default_display_zones {
            let zone_migrated = remap_zone_effect_ids(zone, migrations);
            if zone_migrated > 0 {
                zone.layers_version = zone.layers_version.saturating_add(1);
                migrated += zone_migrated;
            }
        }
        self.refresh_resolved_zones();
        self.invalidate_resolved_zones();
        migrated
    }

    /// Replace persisted layout and physical device identities throughout
    /// authored and runtime-only scene state.
    ///
    /// The render-zone revision advances even when only an inactive scene
    /// changed, fencing prepared layout publications captured before the
    /// identity rewrite.
    ///
    /// When a retargeted display zone lands on a display that already has
    /// a display zone, the pair collapses to one: the zone carrying layers
    /// survives, then the one whose layers were assigned most recently,
    /// then the retargeted zone.
    pub fn remap_device_bindings(
        &mut self,
        layout_device_ids: &HashMap<String, String>,
        physical_device_ids: &HashMap<DeviceId, DeviceId>,
    ) -> usize {
        let mut migrated = 0;
        for scene in self.scenes.values_mut() {
            let retargeted =
                display_collisions::retargeted_display_zones(&scene.zones, physical_device_ids);
            let mut scene_migrated = scene
                .zones
                .iter_mut()
                .map(|zone| {
                    remap_zone_device_bindings(zone, layout_device_ids, physical_device_ids)
                })
                .sum::<usize>();
            scene_migrated +=
                display_collisions::collapse_retargeted_display_zones(scene, &retargeted);
            if scene_migrated > 0 {
                bump_zones_revision(scene);
                migrated += scene_migrated;
            }
        }
        let retargeted_defaults = display_collisions::retargeted_display_zones(
            &self.default_display_zones,
            physical_device_ids,
        );
        migrated += self
            .default_display_zones
            .iter_mut()
            .map(|zone| remap_zone_device_bindings(zone, layout_device_ids, physical_device_ids))
            .sum::<usize>();
        migrated += display_collisions::drop_retargeted_default_duplicates(
            &mut self.default_display_zones,
            &retargeted_defaults,
        );
        if migrated > 0 {
            self.refresh_resolved_zones();
            self.invalidate_resolved_zones();
        }
        migrated
    }

    // ── Transition ──────────────────────────────────────────────────

    /// Get the latest immutable transition plan, if activation requested one.
    #[must_use]
    pub fn transition_plan(&self) -> Option<&TransitionPlan> {
        self.transition_plan.as_ref()
    }

    // ── Priority Stack Access ───────────────────────────────────────

    /// Get a reference to the priority stack.
    #[must_use]
    pub fn priority_stack(&self) -> &PriorityStack {
        &self.priority_stack
    }

    /// Get a mutable reference to the priority stack.
    pub fn priority_stack_mut(&mut self) -> &mut PriorityStack {
        &mut self.priority_stack
    }

    // ── History ─────────────────────────────────────────────────────

    /// Get the activation history (most recent first).
    #[must_use]
    pub fn activation_history(&self) -> &[SceneId] {
        &self.activation_history
    }

    pub fn upsert_primary_zone(
        &mut self,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        active_preset_id: Option<PresetId>,
        full_scope_layout: SpatialLayout,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;
        let custom_zones_present = scene_has_custom_led_zones(scene);
        let next_primary_layout = if custom_zones_present {
            scene
                .primary_zone()
                .map(|zone| zone.layout.clone())
                .unwrap_or_else(|| unclaimed_primary_layout(scene, full_scope_layout))
        } else {
            full_scope_layout
        };

        let mut structural_changed = false;
        if let Some(zone) = scene.primary_zone_mut() {
            let control_bindings = carried_control_bindings(zone, effect.id);
            replace_effect_layer_stack(
                zone,
                effect.id,
                controls,
                control_bindings,
                active_preset_id,
            );
            if zone.layout != next_primary_layout {
                zone.layout = next_primary_layout;
                structural_changed = true;
            }
            zone.enabled = true;
            zone.display_target = None;
            zone.role = ZoneRole::Primary;
            // Preserve the internal control generation for compatibility
            // observers. Wire clients are fenced by the replacement's new
            // immutable layer id and the scene document revision.
            zone.controls_version = zone.controls_version.saturating_add(1);
        } else {
            let mut zone = Zone {
                id: ZoneId::new(),
                name: DEFAULT_ZONE_NAME.to_owned(),
                description: Some(DEFAULT_ZONE_DESCRIPTION.to_owned()),
                layers: Vec::new(),
                layout: next_primary_layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: None,
                role: ZoneRole::Primary,
                controls_version: 0,
                layers_version: 0,
            };
            replace_effect_layer_stack(
                &mut zone,
                effect.id,
                controls,
                HashMap::new(),
                active_preset_id,
            );
            scene.zones.push(zone);
            structural_changed = true;
        }

        if structural_changed {
            bump_zones_revision(scene);
        }

        self.refresh_resolved_zones();
        Ok(self
            .active_scene()
            .and_then(Scene::primary_zone)
            .expect("primary zone should exist after upsert"))
    }

    /// Create an empty `Custom` LED zone in the target scene.
    ///
    /// The zone takes the canvas its LED siblings share, which every layout
    /// publication keeps on the active layout's canvas. Reading it from the
    /// scene, rather than from a size the caller sampled earlier, keeps a
    /// resize that lands between the two from leaving the new zone behind.
    /// `fallback_canvas` is used only when the scene has no LED zone yet.
    pub fn create_zone(
        &mut self,
        scene_id: &SceneId,
        name: String,
        color: Option<String>,
        fallback_canvas: (u32, u32),
    ) -> Result<ZoneId, ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }

        let (canvas_width, canvas_height) = scene
            .zones
            .iter()
            .find(|zone| zone.display_target.is_none())
            .map_or(fallback_canvas, |zone| {
                (zone.layout.canvas_width, zone.layout.canvas_height)
            });
        let id = ZoneId::new();
        scene.zones.push(Zone {
            id,
            name,
            description: None,
            layers: Vec::new(),
            layout: empty_scene_zone_layout(id, canvas_width, canvas_height),
            brightness: 1.0,
            enabled: true,
            color,
            display_target: None,
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        });
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(id)
    }

    pub fn delete_zone(
        &mut self,
        scene_id: &SceneId,
        zone_id: ZoneId,
    ) -> Result<(), ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let Some(index) = scene.zones.iter().position(|zone| zone.id == zone_id) else {
            return Err(ZoneMutationError::ZoneMissing);
        };
        let role = scene.zones[index].role;
        if role != ZoneRole::Custom {
            return Err(ZoneMutationError::InvalidRole { role });
        }

        scene.zones.remove(index);
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(())
    }

    /// Patch a zone's metadata, role, and position in one step.
    ///
    /// A promotion to primary and a move to a different slot are
    /// structural, so they bump `zones_revision` and are refused on a
    /// snapshot scene. A move to the slot the zone already holds changes
    /// nothing.
    pub fn update_zone_meta(
        &mut self,
        scene_id: &SceneId,
        zone_id: ZoneId,
        patch: ZoneMetaPatch,
    ) -> Result<Zone, ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        let role_change = patch.make_primary == Some(true);
        if role_change && scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let Some(index) = scene.zones.iter().position(|zone| zone.id == zone_id) else {
            return Err(ZoneMutationError::ZoneMissing);
        };
        let last_slot = scene.zones.len().saturating_sub(1);
        let destination = patch
            .index
            .map(|requested| requested.min(last_slot))
            .filter(|&destination| destination != index);
        if destination.is_some() && scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }

        if role_change {
            for zone in &mut scene.zones {
                if zone.role == ZoneRole::Primary {
                    zone.role = ZoneRole::Custom;
                }
            }
            let zone = &mut scene.zones[index];
            zone.role = ZoneRole::Primary;
            zone.display_target = None;
        }
        let index = match destination {
            Some(destination) => {
                let zone = scene.zones.remove(index);
                scene.zones.insert(destination, zone);
                destination
            }
            None => index,
        };
        if role_change || destination.is_some() {
            bump_zones_revision(scene);
        }

        let zone = &mut scene.zones[index];
        if let Some(name) = patch.name {
            zone.name = name;
        }
        if let Some(description) = patch.description {
            zone.description = description;
        }
        if let Some(color) = patch.color {
            zone.color = color;
        }
        if let Some(brightness) = patch.brightness {
            zone.brightness = brightness.clamp(0.0, 1.0);
        }
        if let Some(enabled) = patch.enabled {
            zone.enabled = enabled;
        }
        let zone = zone.clone();
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(zone)
    }

    /// Move one output into a zone. See [`Self::assign_outputs_to_zone`].
    pub fn assign_output_to_zone(
        &mut self,
        scene_id: &SceneId,
        zone_id: ZoneId,
        device_zone: Output,
        placement: OutputPlacement,
    ) -> Result<Vec<LayerMove>, ZoneMutationError> {
        self.assign_outputs_to_zone(scene_id, zone_id, vec![device_zone], placement)
    }

    /// Move a set of outputs into a zone as one membership edit.
    ///
    /// Scoped layers follow their outputs: every layer of a zone that
    /// loses outputs is handled against the whole set leaving it, as
    /// [`plan_layer_carry`] describes, so a device moved with all its
    /// outputs keeps a layer scoped to them. Returns each layer that
    /// moved or was narrowed.
    pub fn assign_outputs_to_zone(
        &mut self,
        scene_id: &SceneId,
        zone_id: ZoneId,
        outputs: Vec<Output>,
        placement: OutputPlacement,
    ) -> Result<Vec<LayerMove>, ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let target_index = scene
            .zones
            .iter()
            .position(|zone| zone.id == zone_id)
            .ok_or(ZoneMutationError::ZoneMissing)?;

        let owner_of = |scene: &Scene, output_id: &str| {
            scene.zones.iter().position(|zone| {
                zone.layout
                    .zones
                    .iter()
                    .any(|output| output.id == output_id)
            })
        };
        let moves = outputs
            .iter()
            .filter_map(|output| {
                let owner = owner_of(scene, &output.id)?;
                (owner != target_index).then(|| OutputMove {
                    output_id: output.id.clone(),
                    from: scene.zones[owner].id,
                    to: zone_id,
                })
            })
            .collect::<Vec<_>>();
        let layer_moves = plan_layer_carry(scene, &moves);

        for device_zone in outputs {
            if owner_of(scene, &device_zone.id) == Some(target_index) {
                if let Some(zone) = scene.zones[target_index]
                    .layout
                    .zones
                    .iter_mut()
                    .find(|zone| zone.id == device_zone.id)
                {
                    *zone = device_zone;
                }
            } else {
                for zone in &mut scene.zones {
                    zone.layout.zones.retain(|zone| zone.id != device_zone.id);
                }
                let slot = scene.zones[target_index].layout.zones.len();
                let mut moved = device_zone;
                match placement {
                    OutputPlacement::AutoGrid => reset_device_zone_placement(&mut moved, slot),
                    OutputPlacement::Preserve => {
                        moved.display_order = i32::try_from(slot).unwrap_or(0);
                    }
                }
                scene.zones[target_index].layout.zones.push(moved);
            }
        }
        layer_scope::execute_layer_moves(scene, &layer_moves);

        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(layer_moves)
    }

    pub fn unassign_output(
        &mut self,
        scene_id: &SceneId,
        device_zone_id: &str,
    ) -> Result<(), ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let mut removed = false;
        for zone in &mut scene.zones {
            let previous_len = zone.layout.zones.len();
            zone.layout.zones.retain(|zone| zone.id != device_zone_id);
            removed |= zone.layout.zones.len() != previous_len;
        }
        if !removed {
            return Err(ZoneMutationError::OutputMissing);
        }
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(())
    }

    pub fn set_unassigned_behavior(
        &mut self,
        scene_id: &SceneId,
        behavior: UnassignedBehavior,
    ) -> Result<UnassignedBehavior, ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        if let UnassignedBehavior::Fallback(zone_id) = behavior
            && !scene
                .zones
                .iter()
                .any(|zone| zone.id == zone_id && zone.display_target.is_none())
        {
            return Err(ZoneMutationError::ZoneMissing);
        }
        scene.unassigned_behavior = behavior;
        bump_zones_revision(scene);
        let behavior = scene.unassigned_behavior.clone();
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
            self.invalidate_resolved_zones();
        }
        Ok(behavior)
    }

    /// Apply a placement-only update to a zone's [`SpatialLayout`].
    ///
    /// This is a placement *merge*, never a replace. The request may
    /// move, resize, rotate, restyle, or reorder the outputs the zone
    /// already owns and may retune the zone's canvas dimensions and
    /// sampling defaults — but it can neither add nor drop an output nor
    /// re-bind one to different hardware. Adds and drops route through
    /// the device assignment endpoints (§8); topology and component
    /// binding route through device and component config. A request
    /// whose output-id set differs from the zone's stored set is
    /// rejected with [`ZoneMutationError::LayoutOutputMismatch`].
    pub fn update_zone_layout(
        &mut self,
        scene_id: &SceneId,
        zone_id: ZoneId,
        layout: SpatialLayout,
    ) -> Result<Zone, ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let index = scene
            .zones
            .iter()
            .position(|zone| zone.id == zone_id)
            .ok_or(ZoneMutationError::ZoneMissing)?;

        // The request must carry exactly the outputs the zone owns. Adds
        // and drops are not placement edits — they route through the
        // device endpoints, which keep scene-wide exclusivity intact.
        let stored_ids = scene.zones[index]
            .layout
            .zones
            .iter()
            .map(|zone| zone.id.as_str())
            .collect::<HashSet<_>>();
        let request_ids = layout
            .zones
            .iter()
            .map(|zone| zone.id.as_str())
            .collect::<HashSet<_>>();
        if request_ids.len() != layout.zones.len() || stored_ids != request_ids {
            return Err(ZoneMutationError::LayoutOutputMismatch);
        }

        // Placement and visual fields come from the request, keyed by
        // output id; identity and hardware-binding fields are preserved
        // from the stored output so no request can re-bind hardware or
        // rewrite LED topology. The request's output order is adopted —
        // vector order is the canvas tie-breaker for equal `display_order`
        // and drives ordered routing, so a reorder is a real placement
        // edit, not a no-op.
        let zone = &mut scene.zones[index];
        let mut stored = zone
            .layout
            .zones
            .drain(..)
            .map(|zone| (zone.id.clone(), zone))
            .collect::<HashMap<_, _>>();
        zone.layout.zones = layout
            .zones
            .into_iter()
            .filter_map(|incoming| {
                let mut merged = stored.remove(&incoming.id)?;
                merged.name = incoming.name;
                merged.position = incoming.position;
                merged.size = incoming.size;
                merged.rotation = incoming.rotation;
                merged.scale = incoming.scale;
                merged.display_order = incoming.display_order;
                merged.orientation = incoming.orientation;
                merged.shape = incoming.shape;
                merged.shape_preset = incoming.shape_preset;
                merged.sampling_mode = incoming.sampling_mode;
                merged.edge_behavior = incoming.edge_behavior;
                merged.brightness = incoming.brightness;
                Some(merged)
            })
            .collect();
        // Sampling defaults are mutable; the layout's own identity (id,
        // name, description, version) is preserved from the stored layout.
        // An LED zone's canvas follows the active layout's, so only a
        // display zone takes the request's canvas.
        if zone.display_target.is_some() {
            zone.layout.canvas_width = layout.canvas_width;
            zone.layout.canvas_height = layout.canvas_height;
        }
        zone.layout.default_sampling_mode = layout.default_sampling_mode;
        zone.layout.default_edge_behavior = layout.default_edge_behavior;

        let updated = zone.clone();
        // Exclusivity holds by construction: the output-id set is
        // unchanged and no other zone is touched.
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(updated)
    }

    pub fn patch_display_zone_target(
        &mut self,
        zone_id: ZoneId,
        blend_mode: Option<BlendMode>,
        opacity: Option<f32>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let zone = scene.zones.iter_mut().find(|zone| zone.id == zone_id)?;
        let current_target = zone.display_target.clone()?;
        let mut next_target = DisplayFaceTarget {
            blend_mode: blend_mode.unwrap_or(current_target.blend_mode),
            device_id: current_target.device_id,
            opacity: opacity.unwrap_or(current_target.opacity),
        }
        .normalized();
        if !next_target.clone().blends_with_effect() {
            next_target.opacity = 1.0;
        }
        zone.display_target = Some(next_target);
        self.refresh_resolved_zones();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|zone| zone.id == zone_id))
    }

    /// Insert a new layer into a zone's stack.
    ///
    /// # Errors
    ///
    /// Beside the stack's own refusals,
    /// [`LayerMutationError::ScopeOutputMissing`] when the layer's scope
    /// names an output the zone does not currently hold.
    pub fn insert_zone_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer: SceneLayer,
        index: Option<usize>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            if zone.layers.iter().any(|existing| existing.id == layer.id) {
                return Err(LayerMutationError::DuplicateLayer { layer_id: layer.id });
            }
            let layer = layer.normalized();
            if let Err(errors) = layer.validate() {
                return Err(LayerMutationError::InvalidLayer { errors });
            }
            layer_scope::check_introduced_scope(zone, &layer, None)?;
            if let Some(index) = index {
                if index > zone.layers.len() {
                    return Err(LayerMutationError::InvalidIndex {
                        index,
                        len: zone.layers.len(),
                    });
                }
                zone.layers.insert(index, layer);
            } else {
                zone.layers.push(layer);
            }
            Ok(())
        })
    }

    /// Replace one layer in place, keeping its slot in the stack.
    ///
    /// The replacement's scope is taken as given. Ids the replaced layer's
    /// scope already held are carried without a membership check, so a
    /// layer whose output has left the zone can still be edited; any other
    /// id must be a current output of the zone.
    ///
    /// # Errors
    ///
    /// [`LayerMutationError::LayerMissing`] for an unknown layer,
    /// [`LayerMutationError::ScopeOutputMissing`] for a newly named output
    /// the zone does not hold, and the stack's usual refusals.
    pub fn replace_zone_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            let Some(index) = zone.layers.iter().position(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            if layer.id != layer_id && zone.layers.iter().any(|existing| existing.id == layer.id) {
                return Err(LayerMutationError::DuplicateLayer { layer_id: layer.id });
            }
            let layer = layer.normalized();
            if let Err(errors) = layer.validate() {
                return Err(LayerMutationError::InvalidLayer { errors });
            }
            layer_scope::check_introduced_scope(zone, &layer, zone.layers[index].scope.as_ref())?;
            zone.layers[index] = layer;
            Ok(())
        })
    }

    /// Replace a zone's whole-zone layers with one layer, keeping every
    /// scoped layer above it: the same rule an effect apply follows, for
    /// callers that bring their own base layer.
    ///
    /// # Errors
    ///
    /// Refuses a missing target, an invalid or scoped base layer, and an
    /// id a kept layer already uses, before changing the stack.
    pub fn replace_zone_base_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        base: SceneLayer,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        if base.scope.is_some() {
            return Err(LayerMutationError::InvalidLayer {
                errors: vec!["a zone's base layer applies to the whole zone".to_owned()],
            });
        }
        base.validate()
            .map_err(|errors| LayerMutationError::InvalidLayer { errors })?;
        self.mutate_zone_layers(scene_id, zone_id, None, |zone| {
            if zone
                .layers
                .iter()
                .any(|kept| kept.scope.is_some() && kept.id == base.id)
            {
                return Err(LayerMutationError::DuplicateLayer { layer_id: base.id });
            }
            layer_scope::replace_unscoped_layers(zone, base);
            Ok(())
        })
    }

    /// Apply caller-supplied layer moves, as a membership edit replays
    /// them from a receipt, after that edit's outputs have moved.
    ///
    /// # Errors
    ///
    /// [`LayerMutationError::LayerMoveStale`] when a layer is not at the
    /// slot its move names with the scope it names,
    /// [`LayerMutationError::ScopeOutputMissing`] when a move introduces a
    /// scope id its destination does not hold, and the stack's refusals
    /// for an unknown zone, a repeated layer, or an index out of range.
    /// Nothing changes when any move is refused.
    pub fn apply_layer_moves(
        &mut self,
        scene_id: SceneId,
        moves: &[LayerMove],
    ) -> Result<Vec<ZoneId>, LayerMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(&scene_id)
            .ok_or(LayerMutationError::SceneMissing)?;
        layer_scope::validate_layer_moves(scene, moves)?;
        let touched = layer_scope::execute_layer_moves(scene, moves);
        if !touched.is_empty() && active_scene_id == Some(scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(touched)
    }

    pub fn remove_zone_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            let Some(index) = zone.layers.iter().position(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            zone.layers.remove(index);
            Ok(())
        })
    }

    pub fn reorder_zone_layers(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_ids: Vec<SceneLayerId>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            let current_ids = zone
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<HashSet<_>>();
            let requested_ids = layer_ids.iter().copied().collect::<HashSet<_>>();
            if current_ids.len() != zone.layers.len()
                || requested_ids.len() != layer_ids.len()
                || current_ids != requested_ids
            {
                return Err(LayerMutationError::InvalidOrder);
            }

            let mut layers_by_id = zone
                .layers
                .drain(..)
                .map(|layer| (layer.id, layer))
                .collect::<HashMap<_, _>>();
            zone.layers = layer_ids
                .into_iter()
                .map(|layer_id| {
                    layers_by_id
                        .remove(&layer_id)
                        .expect("layer order was validated as an exact permutation")
                })
                .collect();
            Ok(())
        })
    }

    pub fn patch_zone_layer_effect_controls(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            let Some(layer) = zone.layers.iter_mut().find(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            let LayerSource::Effect { controls, .. } = &mut layer.source else {
                return Err(LayerMutationError::InvalidLayer {
                    errors: vec![format!("layer {layer_id} is not an effect layer")],
                });
            };
            controls.extend(updates);
            Ok(())
        })
    }

    /// Write control values and remove named input bindings in one
    /// mutation (Spec 78 §1.6).
    ///
    /// The two halves are inseparable: a value write to a bound key is
    /// refused with [`LayerMutationError::ControlBound`] unless the same
    /// request clears that binding, so a caller recovering from the
    /// refusal never leaves the layer half-written.
    ///
    /// # Errors
    ///
    /// [`LayerMutationError::ControlBound`] when a written key keeps a
    /// binding this request does not clear, plus the usual missing
    /// scene, zone, layer, and stale-version refusals.
    pub fn patch_zone_layer_controls_and_bindings(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        clear_bindings: &[String],
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_zone_layers(scene_id, zone_id, expected_version, |zone| {
            let Some(layer) = zone.layers.iter_mut().find(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            let LayerSource::Effect {
                controls,
                control_bindings,
                ..
            } = &mut layer.source
            else {
                return Err(LayerMutationError::InvalidLayer {
                    errors: vec![format!("layer {layer_id} is not an effect layer")],
                });
            };

            let mut bound: Vec<String> = updates
                .keys()
                .filter(|key| {
                    control_bindings.contains_key(*key)
                        && !clear_bindings.iter().any(|cleared| cleared == *key)
                })
                .cloned()
                .collect();
            if !bound.is_empty() {
                bound.sort();
                return Err(LayerMutationError::ControlBound { keys: bound });
            }

            for key in clear_bindings {
                control_bindings.remove(key);
            }
            controls.extend(updates);
            Ok(())
        })
    }

    #[must_use]
    pub fn remove_display_zones_for_device(&mut self, device_id: DeviceId) -> Vec<(SceneId, Zone)> {
        let active_scene_id = self.active_scene_id().copied();
        let mut removed_zones = Vec::new();

        for scene in self.scenes.values_mut() {
            let mut index = 0;
            while index < scene.zones.len() {
                let matches_device = scene.zones[index].role == ZoneRole::Display
                    && scene.zones[index]
                        .display_target
                        .as_ref()
                        .is_some_and(|target| target.device_id == device_id);
                if matches_device {
                    removed_zones.push((scene.id, scene.zones.remove(index)));
                } else {
                    index += 1;
                }
            }
        }

        if active_scene_id.is_some_and(|scene_id| {
            removed_zones
                .iter()
                .any(|(removed_scene_id, _)| *removed_scene_id == scene_id)
        }) {
            self.refresh_resolved_zones();
        }

        removed_zones
    }

    pub fn patch_zone_controls(
        &mut self,
        zone_id: ZoneId,
        updates: HashMap<String, ControlValue>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let zone = scene.zones.iter_mut().find(|zone| zone.id == zone_id)?;
        let mut effect_layers = zone.layers.iter().enumerate().filter_map(|(index, layer)| {
            matches!(layer.source, LayerSource::Effect { .. }).then_some(index)
        });
        let index = effect_layers.next()?;
        if effect_layers.next().is_some() {
            return None;
        }
        let LayerSource::Effect { controls, .. } = &mut zone.layers[index].source else {
            unreachable!("selected layer must be an effect layer");
        };
        controls.extend(updates);
        zone.layers_version = zone.layers_version.saturating_add(1);
        zone.controls_version = zone.controls_version.saturating_add(1);
        self.refresh_resolved_zones();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|zone| zone.id == zone_id))
    }

    /// Apply an effect to a named (non-Primary) zone — the
    /// zone-targeted counterpart of [`Self::upsert_primary_zone`]. Sets
    /// the zone's effect, controls, and preset; the zone's layout,
    /// role, and device assignment are left untouched. The zone must
    /// already exist — an effect apply never creates a zone.
    pub fn apply_effect_to_zone(
        &mut self,
        zone_id: ZoneId,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        active_preset_id: Option<PresetId>,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;
        let zone = scene
            .zones
            .iter_mut()
            .find(|zone| zone.id == zone_id)
            .ok_or_else(|| anyhow::anyhow!("zone {zone_id:?} is not in the active scene"))?;
        if zone.role == ZoneRole::Display {
            anyhow::bail!("zone {zone_id:?} is a display face, not an LED zone");
        }
        let control_bindings = carried_control_bindings(zone, effect.id);
        replace_effect_layer_stack(
            zone,
            effect.id,
            controls,
            control_bindings,
            active_preset_id,
        );
        zone.enabled = true;
        // Keep compatibility observers aligned with the source replacement.
        zone.controls_version = zone.controls_version.saturating_add(1);
        self.refresh_resolved_zones();
        self.active_scene()
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .ok_or_else(|| anyhow::anyhow!("zone vanished after effect apply"))
    }

    /// Stop a zone's lighting: remove every layer except those whose
    /// scope names only screens the zone currently holds, so a screen's
    /// face survives the stop gesture the way a display zone's does. A
    /// scope that also names an LED output, or an output that has left the
    /// zone, marks the layer as lighting and it is removed.
    pub fn clear_zone_effect(&mut self, zone_id: ZoneId) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let zone = scene.zones.iter_mut().find(|zone| zone.id == zone_id)?;
        if layer_scope::clear_lighting_layers(zone) {
            zone.layers_version = zone.layers_version.saturating_add(1);
        }
        // Keep the internal generation aligned with the cleared source.
        zone.controls_version = zone.controls_version.saturating_add(1);
        self.refresh_resolved_zones();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|zone| zone.id == zone_id))
    }

    /// Align the active scene's LED zones with the supplied layout.
    ///
    /// The primary zone's layout is a snapshot taken when an effect is
    /// applied. When the active spatial layout changes, that snapshot goes
    /// stale and the render pipeline stops seeing the real device zones. Call
    /// this after applying a new active layout to keep the primary zone in
    /// sync. In a scene with custom LED zones every LED zone keeps its own
    /// placements, but its canvas follows the layout's, so a resolution
    /// change reaches every zone and no zone keeps a shape of its own.
    /// Display zones are left alone: they render at their face's size.
    ///
    /// Returns `true` if any zone's layout changed.
    pub fn sync_primary_zone_layout(&mut self, layout: &SpatialLayout) -> bool {
        let Some(scene) = self.active_scene_mut() else {
            return false;
        };
        let custom = scene_has_custom_led_zones(scene);
        let mut changed = false;
        for zone in &mut scene.zones {
            changed |= align_led_zone(zone, layout, custom);
        }
        if changed {
            bump_zones_revision(scene);
            self.refresh_resolved_zones();
        }
        changed
    }

    /// Build the active render zones that would result from synchronizing the
    /// primary layout, without changing scene state.
    #[must_use]
    pub fn resolved_zones_for_primary_layout(&self, layout: &SpatialLayout) -> (Arc<[Zone]>, u64) {
        let Some(scene) = self.active_scene() else {
            return (
                Arc::clone(&self.resolved_zones),
                self.resolved_zones_revision,
            );
        };
        let custom = scene_has_custom_led_zones(scene);

        let mut changed = false;
        let zones = self
            .resolved_zones
            .iter()
            .cloned()
            .map(|mut zone| {
                changed |= align_led_zone(&mut zone, layout, custom);
                zone
            })
            .collect::<Vec<_>>();
        if changed {
            (zones.into(), self.resolved_zones_revision.saturating_add(1))
        } else {
            (
                Arc::clone(&self.resolved_zones),
                self.resolved_zones_revision,
            )
        }
    }

    fn active_scene_mut(&mut self) -> Option<&mut Scene> {
        let scene_id = *self.active_scene_id()?;
        self.scenes.get_mut(&scene_id)
    }

    fn mutate_zone_layers<F>(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        expected_version: Option<u64>,
        mutate: F,
    ) -> Result<(&Zone, u64), LayerMutationError>
    where
        F: FnOnce(&mut Zone) -> Result<(), LayerMutationError>,
    {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(&scene_id)
            .ok_or(LayerMutationError::SceneMissing)?;
        let zone = scene
            .zones
            .iter_mut()
            .find(|zone| zone.id == zone_id)
            .ok_or(LayerMutationError::ZoneMissing)?;
        if let Some(expected) = expected_version
            && expected != zone.layers_version
        {
            return Err(LayerMutationError::Stale {
                expected,
                current: zone.layers_version,
            });
        }

        mutate(zone)?;
        zone.layers_version = zone.layers_version.saturating_add(1);
        let new_version = zone.layers_version;

        if active_scene_id == Some(scene_id) {
            self.refresh_resolved_zones();
        }
        let current = self
            .scenes
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .ok_or(LayerMutationError::ZoneMissing)?;
        Ok((current, new_version))
    }

    /// Rebuild the zone list the render thread consumes: the active
    /// scene lowered (see [`screens`]), then each kept face whose screen
    /// the scene leaves alone.
    fn refresh_resolved_zones(&mut self) {
        let mut next_zones: Vec<Zone> = self
            .active_scene()
            .map(|scene| screens::lower_zones(scene, &self.screen_surfaces))
            .unwrap_or_default();
        for default_zone in &self.default_display_zones {
            let Some(target) = default_zone.display_target.as_ref() else {
                continue;
            };
            let covered = self.active_scene().is_some_and(|scene| {
                let stack = screens::screen_stack(scene, &self.screen_surfaces, target.device_id);
                stack.assigned() || stack.rendering()
            });
            if !covered {
                next_zones.push(default_zone.clone());
            }
        }
        let next_zones = Arc::<[Zone]>::from(next_zones);
        if self.resolved_zones.as_ref() != next_zones.as_ref() {
            self.resolved_zones_revision = self.resolved_zones_revision.saturating_add(1);
        }
        self.resolved_zones = next_zones;
    }

    // ── Screens ─────────────────────────────────────────────────────

    /// Replace the table of known screens. Returns whether it changed.
    pub fn set_screen_surfaces(&mut self, surfaces: screens::ScreenSurfaces) -> bool {
        if self.screen_surfaces == surfaces {
            return false;
        }
        self.screen_surfaces = surfaces;
        self.refresh_resolved_zones();
        true
    }

    /// Every known screen, by the layout device id its output carries.
    #[must_use]
    pub fn screen_surfaces(&self) -> &screens::ScreenSurfaces {
        &self.screen_surfaces
    }

    /// Where a screen's stack lives in the active scene.
    #[must_use]
    pub fn active_screen_stack(&self, device_id: DeviceId) -> Option<screens::ScreenStack<'_>> {
        self.active_scene()
            .map(|scene| screens::screen_stack(scene, &self.screen_surfaces, device_id))
    }

    /// The view of one known screen in a scene.
    #[must_use]
    pub fn screen_view(
        &self,
        scene_id: &SceneId,
        device_id: DeviceId,
    ) -> Option<screens::ScreenView> {
        let scene = self.scenes.get(scene_id)?;
        screens::screen_view(scene, &self.screen_surfaces, device_id)
    }

    /// The views a scene lists. See [`screens::screen_views`].
    #[must_use]
    pub fn screen_views(&self, scene_id: &SceneId) -> Vec<screens::ScreenView> {
        self.scenes.get(scene_id).map_or_else(Vec::new, |scene| {
            screens::screen_views(scene, &self.screen_surfaces)
        })
    }

    /// The screen a zone id is the view of, when it is one.
    #[must_use]
    pub fn screen_of_view(&self, scene_id: &SceneId, zone_id: ZoneId) -> Option<DeviceId> {
        let scene = self.scenes.get(scene_id)?;
        screens::known_screens(&self.screen_surfaces)
            .into_iter()
            .map(|surface| surface.device_id)
            .filter(|device_id| scene.display_zone_for(*device_id).is_none())
            .find(|device_id| screens::screen_zone_id(*scene_id, *device_id) == zone_id)
    }

    /// Rewrite a scene's stored display zones into screen layers where the
    /// result is exact. See [`screens::convert_legacy_display_zones`].
    pub fn convert_legacy_display_zones(
        &mut self,
        scene_id: &SceneId,
    ) -> screens::ScreenConversion {
        let active_scene_id = self.active_scene_id().copied();
        let Some(scene) = self.scenes.get_mut(scene_id) else {
            return screens::ScreenConversion::default();
        };
        let report = screens::convert_legacy_display_zones(scene, &self.screen_surfaces);
        if report.changed() && active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        report
    }

    /// Make sure a scene stores a display zone for a known screen, and
    /// return its id.
    ///
    /// This is the stack of a screen that no zone places: with no output
    /// to scope a layer to, its layers are stored as a display zone, the
    /// spelling every build has read. The zone takes the screen's view id,
    /// so a client that addressed the empty view keeps the same zone, and
    /// conversion later keeps the same renderer.
    ///
    /// # Errors
    ///
    /// Refuses a missing or snapshot-locked scene, and
    /// [`ZoneMutationError::OutputMissing`] for a device that is not a
    /// known screen.
    pub fn ensure_display_zone(
        &mut self,
        scene_id: &SceneId,
        device_id: DeviceId,
    ) -> Result<ZoneId, ZoneMutationError> {
        let surface = self
            .screen_surfaces
            .values()
            .find(|surface| surface.device_id == device_id)
            .cloned()
            .ok_or(ZoneMutationError::OutputMissing)?;
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        if let Some(zone) = scene.display_zone_for(device_id) {
            return Ok(zone.id);
        }
        let zone_id = screens::screen_zone_id(*scene_id, device_id);
        scene.zones.push(Zone {
            id: zone_id,
            name: format!("{} Face", surface.name),
            description: Some(format!("Display face for {}", surface.name)),
            layers: Vec::new(),
            layout: screens::screen_face_layout(
                device_id,
                &surface.name,
                surface.width,
                surface.height,
            ),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget::new(device_id)),
            role: ZoneRole::Display,
            controls_version: 0,
            layers_version: 0,
        });
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Ok(zone_id)
    }

    /// Make one face the whole stack of a scene's stored display zone,
    /// creating the zone when the scene has none for the screen. Input
    /// bindings carry over when the face is the one already there.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure_display_zone`].
    pub fn write_display_zone_face(
        &mut self,
        scene_id: &SceneId,
        device_id: DeviceId,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        target: DisplayFaceTarget,
    ) -> Result<&Zone, ZoneMutationError> {
        let zone_id = self.ensure_display_zone(scene_id, device_id)?;
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        let zone = scene
            .zones
            .iter_mut()
            .find(|zone| zone.id == zone_id)
            .ok_or(ZoneMutationError::ZoneMissing)?;
        let control_bindings = carried_control_bindings(zone, effect.id);
        replace_effect_layer_stack(zone, effect.id, controls, control_bindings, None);
        zone.display_target = Some(screens::normalize_face_target(target));
        zone.enabled = true;
        zone.controls_version = zone.controls_version.saturating_add(1);
        zone.layers_version = zone.layers_version.saturating_add(1);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        self.scenes
            .get(scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .ok_or(ZoneMutationError::ZoneMissing)
    }

    /// Remove a scene's stored display zone for one device.
    pub fn remove_legacy_display_zone(
        &mut self,
        scene_id: &SceneId,
        device_id: DeviceId,
    ) -> Option<Zone> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self.scenes.get_mut(scene_id)?;
        let index = scene.zones.iter().position(|zone| {
            zone.role == ZoneRole::Display
                && zone
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
        })?;
        let removed = scene.zones.remove(index);
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_resolved_zones();
        }
        Some(removed)
    }

    /// Rewrite one zone's layer stack in place.
    ///
    /// This is the door for writes that are about one screen and have to
    /// touch the zone that places it: the closure edits the stack, and the
    /// result is checked for duplicate ids and invalid layers before it
    /// stands. Scope ids are not re-checked; the screen adapters only
    /// write ids the zone already holds.
    ///
    /// # Errors
    ///
    /// The closure's own refusal, or [`LayerMutationError::InvalidLayer`]
    /// when the edited stack is not valid. Nothing changes on a refusal.
    pub fn edit_zone_layers<T>(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        edit: impl FnOnce(&mut Zone) -> Result<T, LayerMutationError>,
    ) -> Result<T, LayerMutationError> {
        let mut outcome = None;
        self.mutate_zone_layers(scene_id, zone_id, None, |zone| {
            let mut candidate = zone.clone();
            let value = edit(&mut candidate)?;
            for layer in &mut candidate.layers {
                *layer = layer.normalized();
            }
            candidate
                .validate_layers()
                .map_err(|errors| LayerMutationError::InvalidLayer { errors })?;
            zone.layers = candidate.layers;
            outcome = Some(value);
            Ok(())
        })?;
        outcome.ok_or(LayerMutationError::ZoneMissing)
    }

    // ── Default display faces (spec 69 §3.6) ───────────────────────

    /// Install or update the runtime default face zone for a display.
    ///
    /// The zone's identity is keyed by its display target's device; updates
    /// keep the existing [`ZoneId`] so effect slots stay stable. The zone
    /// only reaches the active render zones while the active scene has no
    /// assigned display zone for the same device.
    pub fn set_default_display_zone(&mut self, mut zone: Zone) {
        let Some(device_id) = zone.display_target.as_ref().map(|target| target.device_id) else {
            return;
        };
        if let Some(existing) = self.default_display_zones.iter_mut().find(|zone| {
            zone.display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        }) {
            zone.id = existing.id;
            *existing = zone;
        } else {
            self.default_display_zones.push(zone);
        }
        self.refresh_resolved_zones();
    }

    /// Remove the runtime default face zone for a display, if present.
    pub fn remove_default_display_zone(&mut self, device_id: DeviceId) -> bool {
        let before = self.default_display_zones.len();
        self.default_display_zones.retain(|zone| {
            zone.display_target
                .as_ref()
                .is_none_or(|target| target.device_id != device_id)
        });
        let removed = self.default_display_zones.len() != before;
        if removed {
            self.refresh_resolved_zones();
        }
        removed
    }

    /// Every runtime default face zone, in insertion order.
    ///
    /// A default hidden behind an assigned display zone never reaches
    /// the resolved render zones, so a caller comparing scene state
    /// for concurrent modification has to read the set directly.
    #[must_use]
    pub fn default_display_zones(&self) -> &[Zone] {
        &self.default_display_zones
    }

    /// The runtime default face zone registered for a display, if any.
    #[must_use]
    pub fn default_display_zone_for(&self, device_id: DeviceId) -> Option<&Zone> {
        self.default_display_zones.iter().find(|zone| {
            zone.display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        })
    }
}

#[must_use]
pub fn default_primary_zone(layout: SpatialLayout) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: DEFAULT_ZONE_NAME.to_owned(),
        description: Some(DEFAULT_ZONE_DESCRIPTION.to_owned()),
        layers: Vec::new(),
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }
}

/// Whether the scene's primary zone covers the full scope and so follows
/// the active spatial layout.
///
/// Until a custom LED zone exists, effect applies and layout publications
/// rewrite the primary zone's layout from the active layout, so a placement
/// that should last belongs in that layout. Once a custom LED zone claims
/// outputs, the primary zone's layout is scene-local.
#[must_use]
pub fn primary_zone_follows_active_layout(scene: &Scene) -> bool {
    !scene_has_custom_led_zones(scene)
}

fn bump_zones_revision(scene: &mut Scene) {
    scene.zones_revision = scene.zones_revision.saturating_add(1);
}

fn remap_zone_effect_ids(zone: &mut Zone, migrations: &HashMap<EffectId, EffectId>) -> usize {
    zone.layers
        .iter_mut()
        .map(|layer| match &mut layer.source {
            LayerSource::Effect { effect_id, .. } => {
                let Some(canonical_id) = migrations.get(effect_id).copied() else {
                    return 0;
                };
                *effect_id = canonical_id;
                1
            }
            LayerSource::Media { .. }
            | LayerSource::ScreenRegion { .. }
            | LayerSource::WebViewport { .. }
            | LayerSource::ColorFill { .. } => 0,
        })
        .sum()
}

fn remap_zone_device_bindings(
    zone: &mut Zone,
    layout_device_ids: &HashMap<String, String>,
    physical_device_ids: &HashMap<DeviceId, DeviceId>,
) -> usize {
    let mut migrated = 0;
    for output in &mut zone.layout.zones {
        if let Some(canonical) = layout_device_ids.get(&output.device_id) {
            output.device_id.clone_from(canonical);
            migrated += 1;
        }
    }
    if let Some(target) = zone.display_target.as_mut()
        && let Some(canonical) = physical_device_ids.get(&target.device_id)
    {
        target.device_id = *canonical;
        migrated += 1;
    }
    migrated
}

/// Align one zone with the active layout: a primary zone in a scene without
/// custom LED zones takes the layout whole; any other LED zone keeps its
/// placements and takes the layout's canvas. Display zones are untouched.
fn align_led_zone(zone: &mut Zone, layout: &SpatialLayout, custom_scene: bool) -> bool {
    if zone.display_target.is_some() {
        return false;
    }
    if !custom_scene {
        if zone.role == ZoneRole::Primary && zone.layout != *layout {
            zone.layout = layout.clone();
            return true;
        }
        return false;
    }
    if zone.layout.canvas_width == layout.canvas_width
        && zone.layout.canvas_height == layout.canvas_height
    {
        return false;
    }
    zone.layout.canvas_width = layout.canvas_width;
    zone.layout.canvas_height = layout.canvas_height;
    true
}

fn scene_has_custom_led_zones(scene: &Scene) -> bool {
    scene
        .zones
        .iter()
        .any(|zone| zone.role == ZoneRole::Custom && zone.display_target.is_none())
}

fn unclaimed_primary_layout(scene: &Scene, mut full_scope_layout: SpatialLayout) -> SpatialLayout {
    let claimed = scene
        .zones
        .iter()
        .filter(|zone| zone.role == ZoneRole::Custom && zone.display_target.is_none())
        .flat_map(|zone| zone.layout.zones.iter().map(|zone| zone.id.as_str()))
        .collect::<HashSet<_>>();
    full_scope_layout
        .zones
        .retain(|zone| !claimed.contains(zone.id.as_str()));
    full_scope_layout
}

fn empty_scene_zone_layout(
    zone_id: ZoneId,
    canvas_width: u32,
    canvas_height: u32,
) -> SpatialLayout {
    SpatialLayout {
        id: format!("zone-{zone_id}"),
        name: "Zone Layout".to_owned(),
        description: None,
        canvas_width,
        canvas_height,
        zones: Vec::new(),
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        version: 1,
    }
}

fn empty_default_spatial_layout() -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 640,
        canvas_height: 480,
        zones: Vec::new(),
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        version: 1,
    }
}

/// How [`SceneManager::assign_output_to_zone`] places an output that lands in a
/// zone it did not previously belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPlacement {
    /// Drop the output into the target zone's next grid slot, discarding
    /// whatever geometry it arrived with.
    AutoGrid,

    /// Keep the caller's position, size, rotation, and scale. Only
    /// `display_order` is reassigned, so the output stacks predictably
    /// against whatever the zone already holds.
    Preserve,
}

/// Place a freshly assigned output at a modest default size, cascaded by
/// its slot in the target zone so successive adds neither stack on one
/// spot nor blanket the whole canvas. `size` is a normalized extent and
/// `position` is the box center, so a 0.2 x 0.15 box centered inside the
/// canvas stays small and movable; the user repositions from there.
fn reset_device_zone_placement(zone: &mut Output, slot: usize) {
    const COLS: usize = 5;
    let col = (slot % COLS) as f32;
    let row = (slot / COLS) as f32;
    let x = (0.2 + col * 0.15).min(0.9);
    let y = (0.2 + row * 0.2).clamp(0.1, 0.9);
    zone.position = NormalizedPosition::new(x, y);
    zone.size = NormalizedPosition::new(0.2, 0.15);
    zone.rotation = 0.0;
    zone.scale = 1.0;
    zone.display_order = i32::try_from(slot).unwrap_or(0);
}

/// Replace a zone's whole-zone layers with a freshly identified effect
/// layer. Scoped layers stay above it: a keyboard's own layer or a
/// screen's face survives a change of base effect.
fn replace_effect_layer_stack(
    zone: &mut Zone,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    control_bindings: HashMap<String, ControlBinding>,
    preset_id: Option<PresetId>,
) {
    layer_scope::replace_unscoped_layers(
        zone,
        SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            controls,
            control_bindings,
            preset_id,
        ),
    );
    zone.layers_version = zone.layers_version.saturating_add(1);
}

/// The input bindings a re-applied effect keeps: those of the whole-zone
/// effect layer it replaces, when that layer runs the same effect.
fn carried_control_bindings(zone: &Zone, effect_id: EffectId) -> HashMap<String, ControlBinding> {
    match layer_scope::base_effect_layer(zone).map(|layer| &layer.source) {
        Some(LayerSource::Effect {
            effect_id: current,
            control_bindings,
            ..
        }) if *current == effect_id => control_bindings.clone(),
        _ => HashMap::new(),
    }
}

impl Default for SceneManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Scene Builder Helpers ───────────────────────────────────────────────

/// Create a minimal scene for testing and prototyping.
///
/// This is not part of the public API — it's a convenience for tests
/// and internal use.
#[must_use]
pub fn make_scene(name: &str) -> Scene {
    use hypercolor_types::scene::{ColorInterpolation, EasingFunction, TransitionSpec};

    Scene {
        id: SceneId::new(),
        name: name.to_string(),
        description: None,
        zones: Vec::new(),
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: hypercolor_types::scene::UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}
