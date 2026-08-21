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
pub mod priority;
pub mod transition;

pub use automation::AutomationEngine;
pub use priority::{PriorityStack, StackEntry};
pub use transition::{
    TransitionIdentity, TransitionPlan, interpolate_color, interpolate_oklab, interpolate_srgb,
};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::types::device::DeviceId;
use crate::types::effect::{ControlBinding, ControlValue, EffectId, EffectMetadata};
use crate::types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use crate::types::library::PresetId;
use crate::types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use crate::types::spatial::{NormalizedPosition, Output, SpatialLayout};

const DEFAULT_ZONE_NAME: &str = "Default zone";

/// Error variants for precondition-checked control patches.
///
/// `NoActiveScene` and `GroupMissing` are plumbed separately from
/// `Stale` so API callers can map each to a distinct HTTP status
/// (404 vs 412) without reflecting on strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlsVersionMismatch {
    /// No scene is currently active — nothing to patch.
    NoActiveScene,
    /// The active scene exists but no group with the given id.
    GroupMissing,
    /// The group exists and the `If-Match` precondition did not
    /// match. `current` is the server-side version the client should
    /// rebase against if they choose to retry.
    Stale { current: u64 },
    /// More than one effect layer could receive the patch.
    AmbiguousLayerStack,
}

/// Error variants for precondition-checked layer mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerMutationError {
    /// No scene is currently active.
    NoActiveScene,
    /// No scene exists with the requested id.
    SceneMissing,
    /// The active scene exists but no group with the given id.
    GroupMissing,
    /// The group exists but no layer with the given id.
    LayerMissing { layer_id: SceneLayerId },
    /// The requested layer id already exists in the group.
    DuplicateLayer { layer_id: SceneLayerId },
    /// The supplied `layers_version` precondition is stale. `expected`
    /// is the version the request carried, `current` the one the zone
    /// holds.
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
}

/// Error variants for structural render-group mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneMutationError {
    /// No scene exists with the requested id.
    SceneMissing,
    /// No zone exists with the requested id.
    GroupMissing,
    /// No device zone exists with the requested id.
    OutputMissing,
    /// The scene is snapshot-locked and cannot be structurally edited.
    SnapshotLocked,
    /// The requested mutation is invalid for the group's role.
    InvalidRole { role: ZoneRole },
    /// A placement update carried an output set that does not match the
    /// zone's stored outputs. Adds and drops route through the device
    /// assignment endpoints, not the layout endpoint.
    LayoutOutputMismatch,
}

/// One precondition-checked layer insertion in a multi-group scene mutation.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneGroupLayerInsert {
    /// Target zone.
    pub group_id: ZoneId,
    /// Layer to insert into the target group's authored stack.
    pub layer: SceneLayer,
    /// Optional bottom-to-top insertion index. `None` appends on top.
    pub index: Option<usize>,
    /// Optional expected `layers_version` for optimistic concurrency.
    pub expected_version: Option<u64>,
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
}

// ── SceneManager ────────────────────────────────────────────────────────

/// Immutable authored scene plan consumed by the render thread.
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
    pub unassigned_behavior: crate::types::scene::UnassignedBehavior,
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
    active_render_groups: Arc<[Zone]>,

    /// Monotonic revision for the active render-group cache.
    active_render_groups_revision: u64,

    /// Runtime-only default face zones, keyed by display device. Merged
    /// into the active render groups whenever the active scene has no
    /// assigned display zone for that device; never written into scenes.
    default_display_groups: Vec<Zone>,
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
            active_render_groups: Arc::default(),
            active_render_groups_revision: 0,
            default_display_groups: Vec::new(),
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
            zones: vec![default_primary_group(layout)],
            zones_revision: 0,
            transition: TransitionSpec {
                duration_ms: 1_000,
                easing: EasingFunction::Linear,
                color_interpolation: ColorInterpolation::Oklab,
            },
            priority: ScenePriority::AMBIENT,
            enabled: true,
            metadata: HashMap::new(),
            unassigned_behavior: crate::types::scene::UnassignedBehavior::Off,
            layout_id: None,
            activation_brightness: None,
            kind: SceneKind::Ephemeral,
            mutation_mode: SceneMutationMode::Live,
        };
        self.scenes.insert(default.id, default);
        self.priority_stack
            .push(SceneId::DEFAULT, ScenePriority::AMBIENT);
        self.refresh_active_render_groups();
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
            self.refresh_active_render_groups();
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
        self.refresh_active_render_groups();

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

        self.refresh_active_render_groups();

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

        self.refresh_active_render_groups();
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
    pub fn active_render_groups(&self) -> Arc<[Zone]> {
        Arc::clone(&self.active_render_groups)
    }

    /// Monotonic revision of the cached active zones.
    #[must_use]
    pub fn active_render_groups_revision(&self) -> u64 {
        self.active_render_groups_revision
    }

    /// Capture one commit-stable authored plan for lock-free frame work.
    #[must_use]
    pub fn plan_snapshot(&self, generation: u64) -> ScenePlanSnapshot {
        ScenePlanSnapshot {
            generation,
            active_scene_id: self.active_scene_id().copied(),
            active_scene_name: self.active_scene().map(|scene| scene.name.clone()),
            transition: self.transition_plan.clone(),
            zones: self.active_render_groups(),
            zones_revision: self.active_render_groups_revision,
            unassigned_behavior: self
                .active_scene()
                .map(|scene| scene.unassigned_behavior.clone())
                .unwrap_or_default(),
        }
    }

    /// Invalidate caches derived from the active zones when an
    /// external dependency changes without mutating the scene graph itself.
    pub fn invalidate_active_render_groups(&mut self) {
        self.active_render_groups_revision = self.active_render_groups_revision.saturating_add(1);
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

    pub fn upsert_primary_group(
        &mut self,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        active_preset_id: Option<PresetId>,
        full_scope_layout: SpatialLayout,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;
        let custom_zones_present = scene_has_custom_led_groups(scene);
        let next_primary_layout = if custom_zones_present {
            scene
                .primary_zone()
                .map(|group| group.layout.clone())
                .unwrap_or_else(|| unclaimed_primary_layout(scene, full_scope_layout))
        } else {
            full_scope_layout
        };

        let mut structural_changed = false;
        if let Some(group) = scene.primary_zone_mut() {
            let effect_changed = effect_layer_id(group) != Some(effect.id);
            let control_bindings = if effect_changed {
                HashMap::new()
            } else {
                effect_control_bindings(group)
            };
            replace_effect_layer_stack(
                group,
                effect.id,
                controls,
                control_bindings,
                active_preset_id,
            );
            if group.layout != next_primary_layout {
                group.layout = next_primary_layout;
                structural_changed = true;
            }
            group.enabled = true;
            group.display_target = None;
            group.role = ZoneRole::Primary;
            // An effect swap is the classic trigger for the modal's
            // TOCTOU race: the group id stays the same but `effect_id`
            // has changed out from under an open modal. Bumping the
            // version makes that modal's Apply fail its `If-Match`,
            // forcing it to re-seed against the new effect instead of
            // quietly overwriting controls for the wrong effect.
            group.controls_version = group.controls_version.saturating_add(1);
        } else {
            let mut group = Zone {
                id: ZoneId::new(),
                name: DEFAULT_ZONE_NAME.to_owned(),
                description: Some("Default zone.".to_owned()),
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
                &mut group,
                effect.id,
                controls,
                HashMap::new(),
                active_preset_id,
            );
            scene.zones.push(group);
            structural_changed = true;
        }

        if structural_changed {
            bump_zones_revision(scene);
        }

        self.refresh_active_render_groups();
        Ok(self
            .active_scene()
            .and_then(Scene::primary_zone)
            .expect("primary group should exist after upsert"))
    }

    pub fn upsert_display_group(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        layout: SpatialLayout,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;

        if let Some(group) = scene.display_zone_for_mut(device_id) {
            let effect_changed = effect_layer_id(group) != Some(effect.id);
            let control_bindings = if effect_changed {
                HashMap::new()
            } else {
                effect_control_bindings(group)
            };
            replace_effect_layer_stack(group, effect.id, controls, control_bindings, None);
            group.layout = layout;
            group.display_target = Some(DisplayFaceTarget::new(device_id));
            group.enabled = true;
            group.role = ZoneRole::Display;
            if group.name.trim().is_empty() {
                group.name = format!("{device_name} Face");
            }
        } else {
            let mut group = Zone {
                id: ZoneId::new(),
                name: format!("{device_name} Face"),
                description: Some(format!("Display face for {device_name}")),
                layers: Vec::new(),
                layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: Some(DisplayFaceTarget::new(device_id)),
                role: ZoneRole::Display,
                controls_version: 0,
                layers_version: 0,
            };
            replace_effect_layer_stack(&mut group, effect.id, controls, HashMap::new(), None);
            scene.zones.push(group);
        }

        self.refresh_active_render_groups();
        Ok(self
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .expect("display group should exist after upsert"))
    }

    pub fn ensure_display_group_surface(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;

        let mut structural_changed = false;
        if let Some(group) = scene.display_zone_for_mut(device_id) {
            if group.role != ZoneRole::Display {
                group.role = ZoneRole::Display;
                structural_changed = true;
            }
            if group.display_target.is_none() {
                group.display_target = Some(DisplayFaceTarget::new(device_id));
                structural_changed = true;
            }
            // Repair pass: earlier builds seeded face-less screen groups
            // with Replace, which drops the scene frame the moment a face
            // arrives via the layer path. A face-less group cannot carry a
            // deliberate composition choice (the composition endpoint
            // rejects targets with no face), so normalizing to the blended
            // default is always safe here.
            if !display_group_has_face(group)
                && let Some(target) = group.display_target.as_mut()
                && target.blend_mode == BlendMode::Replace
            {
                target.blend_mode = BlendMode::default();
                structural_changed = true;
            }
            if group.layout != layout {
                group.layout = layout;
                structural_changed = true;
            }
            if group.name.trim().is_empty() {
                device_name.clone_into(&mut group.name);
                structural_changed = true;
            }
        } else {
            scene.zones.push(Zone {
                id: ZoneId::new(),
                name: device_name.to_owned(),
                description: Some(format!("Screen surface for {device_name}")),
                layers: Vec::new(),
                layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: Some(DisplayFaceTarget::new(device_id)),
                role: ZoneRole::Display,
                controls_version: 0,
                layers_version: 0,
            });
            structural_changed = true;
        }

        if structural_changed {
            bump_zones_revision(scene);
            self.refresh_active_render_groups();
        }

        Ok(self
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .expect("display group should exist after sync"))
    }

    pub fn remove_display_group(&mut self, device_id: DeviceId) -> Result<bool> {
        let Some(scene) = self.active_scene_mut() else {
            bail!("no active scene");
        };
        let previous_len = scene.zones.len();
        scene.zones.retain(|group| {
            group.role != ZoneRole::Display
                || group
                    .display_target
                    .as_ref()
                    .is_none_or(|target| target.device_id != device_id)
        });
        let removed = scene.zones.len() != previous_len;
        if removed {
            self.refresh_active_render_groups();
        }
        Ok(removed)
    }

    pub fn clear_display_group_assignment(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;

        let structural_changed = if let Some(group) = scene.display_zone_for_mut(device_id) {
            let changed = !group.layers.is_empty()
                || group.layout != layout
                || !group.enabled
                || group.display_target != Some(DisplayFaceTarget::new(device_id))
                || group.role != ZoneRole::Display
                || group.name.trim().is_empty();
            group.layers.clear();
            group.layout = layout;
            group.enabled = true;
            group.display_target = Some(DisplayFaceTarget::new(device_id));
            group.role = ZoneRole::Display;
            if group.name.trim().is_empty() {
                device_name.clone_into(&mut group.name);
            }
            if changed {
                group.controls_version = group.controls_version.saturating_add(1);
                group.layers_version = group.layers_version.saturating_add(1);
            }
            changed
        } else {
            scene.zones.push(Zone {
                id: ZoneId::new(),
                name: device_name.to_owned(),
                description: Some(format!("Screen surface for {device_name}")),
                layers: Vec::new(),
                layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: Some(DisplayFaceTarget::new(device_id)),
                role: ZoneRole::Display,
                controls_version: 0,
                layers_version: 0,
            });
            true
        };

        if structural_changed {
            bump_zones_revision(scene);
            self.refresh_active_render_groups();
        }

        Ok(self
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .expect("display group should exist after clearing assignment"))
    }

    /// Create an empty `Custom` LED zone in the target scene.
    ///
    /// The zone inherits its canvas from an existing LED group so it stays
    /// consistent with its siblings; `fallback_canvas` is used only when the
    /// scene has no LED group to inherit from.
    pub fn create_render_group(
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
            .find(|group| group.display_target.is_none())
            .map_or(fallback_canvas, |group| {
                (group.layout.canvas_width, group.layout.canvas_height)
            });
        let id = ZoneId::new();
        scene.zones.push(Zone {
            id,
            name,
            description: None,
            layers: Vec::new(),
            layout: empty_scene_group_layout(id, canvas_width, canvas_height),
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
            self.refresh_active_render_groups();
        }
        Ok(id)
    }

    pub fn delete_render_group(
        &mut self,
        scene_id: &SceneId,
        group_id: ZoneId,
    ) -> Result<(), ZoneMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(scene_id)
            .ok_or(ZoneMutationError::SceneMissing)?;
        if scene.blocks_runtime_mutation() {
            return Err(ZoneMutationError::SnapshotLocked);
        }
        let Some(index) = scene.zones.iter().position(|group| group.id == group_id) else {
            return Err(ZoneMutationError::GroupMissing);
        };
        let role = scene.zones[index].role;
        if role != ZoneRole::Custom {
            return Err(ZoneMutationError::InvalidRole { role });
        }

        scene.zones.remove(index);
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
        }
        Ok(())
    }

    pub fn update_render_group_meta(
        &mut self,
        scene_id: &SceneId,
        group_id: ZoneId,
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
        let Some(index) = scene.zones.iter().position(|group| group.id == group_id) else {
            return Err(ZoneMutationError::GroupMissing);
        };

        if role_change {
            for group in &mut scene.zones {
                if group.role == ZoneRole::Primary {
                    group.role = ZoneRole::Custom;
                }
            }
            let group = &mut scene.zones[index];
            group.role = ZoneRole::Primary;
            group.display_target = None;
            bump_zones_revision(scene);
        }

        let group = &mut scene.zones[index];
        if let Some(name) = patch.name {
            group.name = name;
        }
        if let Some(description) = patch.description {
            group.description = description;
        }
        if let Some(color) = patch.color {
            group.color = color;
        }
        if let Some(brightness) = patch.brightness {
            group.brightness = brightness.clamp(0.0, 1.0);
        }
        if let Some(enabled) = patch.enabled {
            group.enabled = enabled;
        }
        let group = group.clone();
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
        }
        Ok(group)
    }

    pub fn assign_device_zone(
        &mut self,
        scene_id: &SceneId,
        group_id: ZoneId,
        device_zone: Output,
        placement: OutputPlacement,
    ) -> Result<(), ZoneMutationError> {
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
            .position(|group| group.id == group_id)
            .ok_or(ZoneMutationError::GroupMissing)?;

        let current_owner = scene.zones.iter().position(|group| {
            group
                .layout
                .zones
                .iter()
                .any(|zone| zone.id == device_zone.id)
        });

        if current_owner == Some(target_index) {
            if let Some(zone) = scene.zones[target_index]
                .layout
                .zones
                .iter_mut()
                .find(|zone| zone.id == device_zone.id)
            {
                *zone = device_zone;
            }
        } else {
            for group in &mut scene.zones {
                group.layout.zones.retain(|zone| zone.id != device_zone.id);
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

        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
        }
        Ok(())
    }

    pub fn unassign_device_zone(
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
        for group in &mut scene.zones {
            let previous_len = group.layout.zones.len();
            group.layout.zones.retain(|zone| zone.id != device_zone_id);
            removed |= group.layout.zones.len() != previous_len;
        }
        if !removed {
            return Err(ZoneMutationError::OutputMissing);
        }
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
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
        if let UnassignedBehavior::Fallback(group_id) = behavior
            && !scene
                .zones
                .iter()
                .any(|group| group.id == group_id && group.display_target.is_none())
        {
            return Err(ZoneMutationError::GroupMissing);
        }
        scene.unassigned_behavior = behavior;
        bump_zones_revision(scene);
        let behavior = scene.unassigned_behavior.clone();
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
            self.invalidate_active_render_groups();
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
            .position(|group| group.id == zone_id)
            .ok_or(ZoneMutationError::GroupMissing)?;

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
        let group = &mut scene.zones[index];
        let mut stored = group
            .layout
            .zones
            .drain(..)
            .map(|zone| (zone.id.clone(), zone))
            .collect::<HashMap<_, _>>();
        group.layout.zones = layout
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
        // Canvas dimensions and sampling defaults are mutable; the
        // layout's own identity (id, name, description, version, spaces)
        // is preserved from the stored layout.
        group.layout.canvas_width = layout.canvas_width;
        group.layout.canvas_height = layout.canvas_height;
        group.layout.default_sampling_mode = layout.default_sampling_mode;
        group.layout.default_edge_behavior = layout.default_edge_behavior;

        let updated = group.clone();
        // Exclusivity holds by construction: the output-id set is
        // unchanged and no other zone is touched.
        bump_zones_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
        }
        Ok(updated)
    }

    pub fn patch_display_group_target(
        &mut self,
        group_id: ZoneId,
        blend_mode: Option<BlendMode>,
        opacity: Option<f32>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.zones.iter_mut().find(|group| group.id == group_id)?;
        let current_target = group.display_target.clone()?;
        let mut next_target = DisplayFaceTarget {
            blend_mode: blend_mode.unwrap_or(current_target.blend_mode),
            device_id: current_target.device_id,
            opacity: opacity.unwrap_or(current_target.opacity),
        }
        .normalized();
        if !next_target.clone().blends_with_effect() {
            next_target.opacity = 1.0;
        }
        group.display_target = Some(next_target);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
    }

    pub fn add_group_layer(
        &mut self,
        group_id: ZoneId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        let scene_id = self
            .active_scene_id()
            .copied()
            .ok_or(LayerMutationError::NoActiveScene)?;
        self.add_scene_group_layer(scene_id, group_id, layer, expected_version)
    }

    pub fn add_scene_group_layer(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.insert_scene_group_layer(scene_id, group_id, layer, None, expected_version)
    }

    pub fn insert_scene_group_layer(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer: SceneLayer,
        index: Option<usize>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            if group.layers.iter().any(|existing| existing.id == layer.id) {
                return Err(LayerMutationError::DuplicateLayer { layer_id: layer.id });
            }
            let layer = layer.normalized();
            if let Err(errors) = layer.validate() {
                return Err(LayerMutationError::InvalidLayer { errors });
            }
            if let Some(index) = index {
                if index > group.layers.len() {
                    return Err(LayerMutationError::InvalidIndex {
                        index,
                        len: group.layers.len(),
                    });
                }
                group.layers.insert(index, layer);
            } else {
                group.layers.push(layer);
            }
            Ok(())
        })
    }

    pub fn insert_scene_group_layers_batch(
        &mut self,
        scene_id: SceneId,
        inserts: Vec<SceneGroupLayerInsert>,
    ) -> Result<Vec<Zone>, LayerMutationError> {
        let active_scene_id = self.active_scene_id().copied();
        let scene = self
            .scenes
            .get_mut(&scene_id)
            .ok_or(LayerMutationError::SceneMissing)?;
        let mut seen_targets = HashSet::with_capacity(inserts.len());
        let mut target_order = Vec::with_capacity(inserts.len());
        let mut normalized_inserts = Vec::with_capacity(inserts.len());

        for insert in inserts {
            if !seen_targets.insert(insert.group_id) {
                return Err(LayerMutationError::InvalidLayer {
                    errors: vec![format!(
                        "target group {} appears more than once",
                        insert.group_id
                    )],
                });
            }
            target_order.push(insert.group_id);
            let group = scene
                .zones
                .iter()
                .find(|group| group.id == insert.group_id)
                .ok_or(LayerMutationError::GroupMissing)?;
            if let Some(expected) = insert.expected_version
                && expected != group.layers_version
            {
                return Err(LayerMutationError::Stale {
                    expected,
                    current: group.layers_version,
                });
            }
            if group
                .layers
                .iter()
                .any(|existing| existing.id == insert.layer.id)
            {
                return Err(LayerMutationError::DuplicateLayer {
                    layer_id: insert.layer.id,
                });
            }
            let layer = insert.layer.normalized();
            if let Err(errors) = layer.validate() {
                return Err(LayerMutationError::InvalidLayer { errors });
            }
            let effective_len = group.layers.clone().len();
            if let Some(index) = insert.index
                && index > effective_len
            {
                return Err(LayerMutationError::InvalidIndex {
                    index,
                    len: effective_len,
                });
            }
            normalized_inserts.push(SceneGroupLayerInsert {
                group_id: insert.group_id,
                layer,
                index: insert.index,
                expected_version: None,
            });
        }

        for insert in normalized_inserts {
            let group = scene
                .zones
                .iter_mut()
                .find(|group| group.id == insert.group_id)
                .ok_or(LayerMutationError::GroupMissing)?;
            if let Some(index) = insert.index {
                group.layers.insert(index, insert.layer);
            } else {
                group.layers.push(insert.layer);
            }
            group.layers_version = group.layers_version.saturating_add(1);
        }

        if active_scene_id == Some(scene_id) {
            self.refresh_active_render_groups();
        }
        let scene = self
            .scenes
            .get(&scene_id)
            .ok_or(LayerMutationError::SceneMissing)?;
        target_order
            .into_iter()
            .map(|group_id| {
                scene
                    .zones
                    .iter()
                    .find(|group| group.id == group_id)
                    .cloned()
                    .ok_or(LayerMutationError::GroupMissing)
            })
            .collect()
    }

    pub fn update_group_layer(
        &mut self,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        let scene_id = self
            .active_scene_id()
            .copied()
            .ok_or(LayerMutationError::NoActiveScene)?;
        self.update_scene_group_layer(scene_id, group_id, layer_id, layer, expected_version)
    }

    pub fn update_scene_group_layer(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        layer: SceneLayer,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            if layer.id != layer_id {
                return Err(LayerMutationError::InvalidLayer {
                    errors: vec![format!(
                        "layer id {} does not match target {}",
                        layer.id, layer_id
                    )],
                });
            }
            let Some(index) = group.layers.iter().position(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            let layer = layer.normalized();
            if let Err(errors) = layer.validate() {
                return Err(LayerMutationError::InvalidLayer { errors });
            }
            group.layers[index] = layer;
            Ok(())
        })
    }

    pub fn remove_group_layer(
        &mut self,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        let scene_id = self
            .active_scene_id()
            .copied()
            .ok_or(LayerMutationError::NoActiveScene)?;
        self.remove_scene_group_layer(scene_id, group_id, layer_id, expected_version)
    }

    pub fn remove_scene_group_layer(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            let Some(index) = group.layers.iter().position(|layer| layer.id == layer_id) else {
                return Err(LayerMutationError::LayerMissing { layer_id });
            };
            group.layers.remove(index);
            Ok(())
        })
    }

    pub fn reorder_group_layers(
        &mut self,
        group_id: ZoneId,
        layer_ids: Vec<SceneLayerId>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        let scene_id = self
            .active_scene_id()
            .copied()
            .ok_or(LayerMutationError::NoActiveScene)?;
        self.reorder_scene_group_layers(scene_id, group_id, layer_ids, expected_version)
    }

    pub fn reorder_scene_group_layers(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer_ids: Vec<SceneLayerId>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            let current_ids = group
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<HashSet<_>>();
            let requested_ids = layer_ids.iter().copied().collect::<HashSet<_>>();
            if current_ids.len() != group.layers.len()
                || requested_ids.len() != layer_ids.len()
                || current_ids != requested_ids
            {
                return Err(LayerMutationError::InvalidOrder);
            }

            let mut layers_by_id = group
                .layers
                .drain(..)
                .map(|layer| (layer.id, layer))
                .collect::<HashMap<_, _>>();
            group.layers = layer_ids
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

    pub fn patch_layer_effect_controls(
        &mut self,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        let scene_id = self
            .active_scene_id()
            .copied()
            .ok_or(LayerMutationError::NoActiveScene)?;
        self.patch_scene_layer_effect_controls(
            scene_id,
            group_id,
            layer_id,
            updates,
            expected_version,
        )
    }

    pub fn patch_scene_layer_effect_controls(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            let Some(layer) = group.layers.iter_mut().find(|layer| layer.id == layer_id) else {
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
    pub fn patch_scene_layer_controls_and_bindings(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        clear_bindings: &[String],
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), LayerMutationError> {
        self.mutate_scene_group_layers(scene_id, group_id, expected_version, |group| {
            let Some(layer) = group.layers.iter_mut().find(|layer| layer.id == layer_id) else {
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
    pub fn remove_display_groups_for_device(
        &mut self,
        device_id: DeviceId,
    ) -> Vec<(SceneId, Zone)> {
        let active_scene_id = self.active_scene_id().copied();
        let mut removed_groups = Vec::new();

        for scene in self.scenes.values_mut() {
            let mut index = 0;
            while index < scene.zones.len() {
                let matches_device = scene.zones[index].role == ZoneRole::Display
                    && scene.zones[index]
                        .display_target
                        .as_ref()
                        .is_some_and(|target| target.device_id == device_id);
                if matches_device {
                    removed_groups.push((scene.id, scene.zones.remove(index)));
                } else {
                    index += 1;
                }
            }
        }

        if active_scene_id.is_some_and(|scene_id| {
            removed_groups
                .iter()
                .any(|(removed_scene_id, _)| *removed_scene_id == scene_id)
        }) {
            self.refresh_active_render_groups();
        }

        removed_groups
    }

    pub fn patch_group_controls(
        &mut self,
        group_id: ZoneId,
        updates: HashMap<String, ControlValue>,
    ) -> Option<&Zone> {
        self.patch_group_controls_with_precondition(group_id, updates, None)
            .ok()
            .map(|(group, _version)| group)
    }

    /// Apply control updates subject to an optional version precondition.
    ///
    /// `expected_version = Some(N)` returns
    /// `Err(ControlsVersionMismatch { current })` if the group's
    /// current version is not `N`. `None` skips the check (the common
    /// patch_group_controls caller is preserved). On success the
    /// returned tuple carries the new version so callers can echo it
    /// back as an `ETag`.
    pub fn patch_group_controls_with_precondition(
        &mut self,
        group_id: ZoneId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), ControlsVersionMismatch> {
        self.patch_effect_controls_with_precondition(group_id, None, updates, expected_version)
    }

    /// Patch a zone's controls, optionally requiring the group
    /// to currently be bound to a specific `expected_effect_id`.
    ///
    /// The `expected_effect_id` gate closes the TOCTOU window the
    /// Viewport Designer modal would otherwise hit: a GET resolves an
    /// `effect_id → group_id` mapping, the modal edits, and later
    /// issues a PATCH. If another client swaps the Default-zone effect in
    /// between, `group_id` will be reused ([`upsert_primary_group`])
    /// but `effect_id` will have changed — so the PATCH would land on
    /// the wrong effect. Requiring `effect_id` to match at write time
    /// turns that silent drift into a clean `GroupMissing` error.
    pub fn patch_effect_controls_with_precondition(
        &mut self,
        group_id: ZoneId,
        expected_effect_id: Option<EffectId>,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<(&Zone, u64), ControlsVersionMismatch> {
        let scene = self
            .active_scene_mut()
            .ok_or(ControlsVersionMismatch::NoActiveScene)?;
        let group = scene
            .zones
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or(ControlsVersionMismatch::GroupMissing)?;
        if let Some(expected_effect_id) = expected_effect_id
            && effect_layer_id(group) != Some(expected_effect_id)
        {
            // The group no longer loads the effect the caller thought
            // it was editing. Reporting `GroupMissing` (vs a new
            // "effect changed" variant) keeps the API surface small
            // and routes clients into the same "re-seed your draft
            // from the live scene" recovery path as a true
            // missing-group case.
            return Err(ControlsVersionMismatch::GroupMissing);
        }
        if let Some(expected) = expected_version
            && expected != group.controls_version
        {
            return Err(ControlsVersionMismatch::Stale {
                current: group.controls_version,
            });
        }
        {
            let effect_layer_count = group
                .layers
                .iter()
                .filter(|layer| matches!(layer.source, LayerSource::Effect { .. }))
                .count();
            if effect_layer_count > 1 {
                return Err(ControlsVersionMismatch::AmbiguousLayerStack);
            }
            let matching_indices = group
                .layers
                .iter()
                .enumerate()
                .filter_map(|(index, layer)| match &layer.source {
                    LayerSource::Effect { effect_id, .. }
                        if expected_effect_id.is_none_or(|expected| expected == *effect_id) =>
                    {
                        Some(index)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [index] = matching_indices.as_slice() else {
                return if matching_indices.is_empty() {
                    Err(ControlsVersionMismatch::GroupMissing)
                } else {
                    Err(ControlsVersionMismatch::AmbiguousLayerStack)
                };
            };
            let LayerSource::Effect { controls, .. } = &mut group.layers[*index].source else {
                unreachable!("matching layer index must point to an effect layer");
            };
            controls.extend(updates);
        }
        group.controls_version = group.controls_version.saturating_add(1);
        let new_version = group.controls_version;
        self.refresh_active_render_groups();
        let current = self
            .active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
            .ok_or(ControlsVersionMismatch::GroupMissing)?;
        Ok((current, new_version))
    }

    pub fn reset_group_controls(
        &mut self,
        group_id: ZoneId,
        defaults: HashMap<String, ControlValue>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.zones.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            controls,
            preset_id,
            ..
        }) = effect_layer_source_mut(group)
        {
            *controls = defaults;
            *preset_id = None;
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            return None;
        }
        // Reset is a controls mutation from a concurrency standpoint
        // — any modal that opened before this call is holding a
        // stale snapshot, so its next `If-Match` PATCH must fail.
        // Same treatment as `patch_group_controls_with_precondition`.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
    }

    /// Apply an effect to a named (non-Primary) zone — the
    /// zone-targeted counterpart of [`Self::upsert_primary_group`]. Sets
    /// the group's effect, controls, and preset; the group's layout,
    /// role, and device assignment are left untouched. The group must
    /// already exist — an effect apply never creates a zone.
    pub fn apply_effect_to_group(
        &mut self,
        group_id: ZoneId,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        active_preset_id: Option<PresetId>,
    ) -> Result<&Zone> {
        let scene = self
            .active_scene_mut()
            .ok_or_else(|| anyhow::anyhow!("no active scene"))?;
        let group = scene
            .zones
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| anyhow::anyhow!("zone {group_id:?} is not in the active scene"))?;
        if group.role == ZoneRole::Display {
            anyhow::bail!("zone {group_id:?} is a display face, not an LED zone");
        }
        let effect_changed = effect_layer_id(group) != Some(effect.id);
        let control_bindings = if effect_changed {
            HashMap::new()
        } else {
            effect_control_bindings(group)
        };
        replace_effect_layer_stack(
            group,
            effect.id,
            controls,
            control_bindings,
            active_preset_id,
        );
        group.enabled = true;
        // An effect swap is the TOCTOU trigger for an open controls
        // modal, so the version advances — mirrors upsert_primary_group.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|scene| scene.zones.iter().find(|group| group.id == group_id))
            .ok_or_else(|| anyhow::anyhow!("zone vanished after effect apply"))
    }

    pub fn clear_group_effect(&mut self, group_id: ZoneId) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.zones.iter_mut().find(|group| group.id == group_id)?;
        if !group.layers.is_empty() {
            group.layers.clear();
            group.layers_version = group.layers_version.saturating_add(1);
        }
        // Clearing the effect zeros every control; that is by far the
        // most dramatic controls mutation and must invalidate every
        // outstanding modal draft.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
    }

    pub fn set_group_control_binding(
        &mut self,
        group_id: ZoneId,
        control_id: String,
        binding: ControlBinding,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.zones.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            control_bindings, ..
        }) = effect_layer_source_mut(group)
        {
            control_bindings.insert(control_id, binding);
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            return None;
        }
        // Bindings surface as control values at render time, so a
        // new binding changes what the user would see if they opened
        // the modal — version must advance alongside raw control
        // mutations.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
    }

    pub fn set_group_preset_id(
        &mut self,
        group_id: ZoneId,
        preset_id: Option<PresetId>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.zones.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            preset_id: layer_preset_id,
            ..
        }) = effect_layer_source_mut(group)
        {
            *layer_preset_id = preset_id;
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            return None;
        }
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.zones.iter().find(|group| group.id == group_id))
    }

    /// Refresh the active scene's full-scope (primary-role, non-display) groups
    /// so their `layout` matches the supplied layout.
    ///
    /// The primary group's layout is a snapshot taken when an effect is
    /// applied. When the active spatial layout changes, that snapshot goes
    /// stale and the render pipeline stops seeing the real device zones. Call
    /// this after applying a new active layout to keep the primary group in
    /// sync. Custom and display groups are left alone — they own their own
    /// layouts.
    ///
    /// Returns `true` if any group's layout changed.
    pub fn sync_primary_group_layout(&mut self, layout: &SpatialLayout) -> bool {
        let Some(scene) = self.active_scene_mut() else {
            return false;
        };
        if scene_has_custom_led_groups(scene) {
            return false;
        }
        let mut changed = false;
        for group in &mut scene.zones {
            if group.role != ZoneRole::Primary || group.display_target.is_some() {
                continue;
            }
            if group.layout != *layout {
                group.layout = layout.clone();
                changed = true;
            }
        }
        if changed {
            bump_zones_revision(scene);
            self.refresh_active_render_groups();
        }
        changed
    }

    /// Build the active render groups that would result from synchronizing the
    /// primary layout, without changing scene state.
    #[must_use]
    pub fn active_render_groups_for_primary_layout(
        &self,
        layout: &SpatialLayout,
    ) -> (Arc<[Zone]>, u64) {
        let Some(scene) = self.active_scene() else {
            return (
                Arc::clone(&self.active_render_groups),
                self.active_render_groups_revision,
            );
        };
        if scene_has_custom_led_groups(scene) {
            return (
                Arc::clone(&self.active_render_groups),
                self.active_render_groups_revision,
            );
        }

        let mut changed = false;
        let groups = self
            .active_render_groups
            .iter()
            .cloned()
            .map(|mut group| {
                if group.role == ZoneRole::Primary
                    && group.display_target.is_none()
                    && group.layout != *layout
                {
                    group.layout = layout.clone();
                    changed = true;
                }
                group
            })
            .collect::<Vec<_>>();
        if changed {
            (
                groups.into(),
                self.active_render_groups_revision.saturating_add(1),
            )
        } else {
            (
                Arc::clone(&self.active_render_groups),
                self.active_render_groups_revision,
            )
        }
    }

    fn active_scene_mut(&mut self) -> Option<&mut Scene> {
        let scene_id = *self.active_scene_id()?;
        self.scenes.get_mut(&scene_id)
    }

    fn mutate_scene_group_layers<F>(
        &mut self,
        scene_id: SceneId,
        group_id: ZoneId,
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
        let group = scene
            .zones
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or(LayerMutationError::GroupMissing)?;
        if let Some(expected) = expected_version
            && expected != group.layers_version
        {
            return Err(LayerMutationError::Stale {
                expected,
                current: group.layers_version,
            });
        }

        mutate(group)?;
        group.layers_version = group.layers_version.saturating_add(1);
        let new_version = group.layers_version;

        if active_scene_id == Some(scene_id) {
            self.refresh_active_render_groups();
        }
        let current = self
            .scenes
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|group| group.id == group_id))
            .ok_or(LayerMutationError::GroupMissing)?;
        Ok((current, new_version))
    }

    fn refresh_active_render_groups(&mut self) {
        let mut next_groups: Vec<Zone> = self
            .active_scene()
            .map(|scene| scene.zones.clone())
            .unwrap_or_default();
        for default_group in &self.default_display_groups {
            let Some(target) = default_group.display_target.as_ref() else {
                continue;
            };
            let covered = self
                .active_scene()
                .and_then(|scene| scene.display_zone_for(target.device_id))
                .is_some_and(|zone| effect_layer_id(zone).is_some());
            if !covered {
                next_groups.push(default_group.clone());
            }
        }
        let next_groups = Arc::<[Zone]>::from(next_groups);
        if self.active_render_groups.as_ref() != next_groups.as_ref() {
            self.active_render_groups_revision =
                self.active_render_groups_revision.saturating_add(1);
        }
        self.active_render_groups = next_groups;
    }

    // ── Default display faces (spec 69 §3.6) ───────────────────────

    /// Install or update the runtime default face zone for a display.
    ///
    /// The zone's identity is keyed by its display target's device; updates
    /// keep the existing [`ZoneId`] so effect slots stay stable. The zone
    /// only reaches the active render groups while the active scene has no
    /// assigned display zone for the same device.
    pub fn set_default_display_group(&mut self, mut zone: Zone) {
        let Some(device_id) = zone.display_target.as_ref().map(|target| target.device_id) else {
            return;
        };
        if let Some(existing) = self.default_display_groups.iter_mut().find(|group| {
            group
                .display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        }) {
            zone.id = existing.id;
            *existing = zone;
        } else {
            self.default_display_groups.push(zone);
        }
        self.refresh_active_render_groups();
    }

    /// Remove the runtime default face zone for a display, if present.
    pub fn remove_default_display_group(&mut self, device_id: DeviceId) -> bool {
        let before = self.default_display_groups.len();
        self.default_display_groups.retain(|group| {
            group
                .display_target
                .as_ref()
                .is_none_or(|target| target.device_id != device_id)
        });
        let removed = self.default_display_groups.len() != before;
        if removed {
            self.refresh_active_render_groups();
        }
        removed
    }

    /// Every runtime default face zone, in insertion order.
    ///
    /// A default hidden behind an assigned display zone never reaches
    /// the resolved render groups, so a caller comparing scene state
    /// for concurrent modification has to read the set directly.
    #[must_use]
    pub fn default_display_groups(&self) -> &[Zone] {
        &self.default_display_groups
    }

    /// The runtime default face zone registered for a display, if any.
    #[must_use]
    pub fn default_display_group_for(&self, device_id: DeviceId) -> Option<&Zone> {
        self.default_display_groups.iter().find(|group| {
            group
                .display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        })
    }
}

#[must_use]
pub fn default_primary_group(mut layout: SpatialLayout) -> Zone {
    DEFAULT_ZONE_NAME.clone_into(&mut layout.name);
    Zone {
        id: ZoneId::new(),
        name: DEFAULT_ZONE_NAME.to_owned(),
        description: Some("Default zone.".to_owned()),
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

fn bump_zones_revision(scene: &mut Scene) {
    scene.zones_revision = scene.zones_revision.saturating_add(1);
}

fn display_group_has_face(group: &Zone) -> bool {
    effect_layer_id(group).is_some()
}

fn scene_has_custom_led_groups(scene: &Scene) -> bool {
    scene
        .zones
        .iter()
        .any(|group| group.role == ZoneRole::Custom && group.display_target.is_none())
}

fn unclaimed_primary_layout(scene: &Scene, mut full_scope_layout: SpatialLayout) -> SpatialLayout {
    let claimed = scene
        .zones
        .iter()
        .filter(|group| group.role == ZoneRole::Custom && group.display_target.is_none())
        .flat_map(|group| group.layout.zones.iter().map(|zone| zone.id.as_str()))
        .collect::<HashSet<_>>();
    full_scope_layout
        .zones
        .retain(|zone| !claimed.contains(zone.id.as_str()));
    full_scope_layout
}

fn empty_scene_group_layout(
    group_id: ZoneId,
    canvas_width: u32,
    canvas_height: u32,
) -> SpatialLayout {
    SpatialLayout {
        id: format!("zone-{group_id}"),
        name: "Zone Layout".to_owned(),
        description: None,
        canvas_width,
        canvas_height,
        zones: Vec::new(),
        default_sampling_mode: crate::types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: crate::types::spatial::EdgeBehavior::Clamp,
        spaces: None,
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
        default_sampling_mode: crate::types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: crate::types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

/// Place a freshly assigned output at a modest default size, cascaded by
/// its slot in the target zone so successive adds neither stack on one
/// spot nor blanket the whole canvas. `size` is a normalized extent and
/// `position` is the box center, so a 0.2 x 0.15 box centered inside the
/// canvas stays small and movable; the user repositions from there.
/// How [`SceneManager::assign_device_zone`] places an output that lands in a
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

/// Replace a zone's whole stack with a freshly identified effect layer.
fn replace_effect_layer_stack(
    group: &mut Zone,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    control_bindings: HashMap<String, ControlBinding>,
    preset_id: Option<PresetId>,
) {
    group.layers = vec![SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        controls,
        control_bindings,
        preset_id,
    )];
    group.layers_version = group.layers_version.saturating_add(1);
}

fn effect_layer_source_mut(group: &mut Zone) -> Option<&mut LayerSource> {
    group
        .layers
        .iter_mut()
        .find_map(|layer| match &mut layer.source {
            source @ LayerSource::Effect { .. } => Some(source),
            _ => None,
        })
}

fn effect_layer_id(group: &Zone) -> Option<EffectId> {
    group.layers.iter().find_map(|layer| match layer.source {
        LayerSource::Effect { effect_id, .. } => Some(effect_id),
        _ => None,
    })
}

fn effect_control_bindings(group: &Zone) -> HashMap<String, ControlBinding> {
    group
        .layers
        .iter()
        .find_map(|layer| match &layer.source {
            LayerSource::Effect {
                control_bindings, ..
            } => Some(control_bindings.clone()),
            _ => None,
        })
        .unwrap_or_default()
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
    use crate::types::scene::{ColorInterpolation, EasingFunction, TransitionSpec};

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
        unassigned_behavior: crate::types::scene::UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}
