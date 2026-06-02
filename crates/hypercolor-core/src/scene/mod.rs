//! Scene engine — scene lifecycle, transition blending, priority management,
//! and automation rule evaluation.
//!
//! This module is the orchestration layer that sits between the effect
//! pipeline and the event-driven automation system. It manages:
//!
//! - **Scene CRUD** — create, read, update, delete scenes.
//! - **Activation** — activate a scene with a transition, track the active scene.
//! - **Deactivation** — deactivate the current scene, restoring the previous one.
//! - **Transitions** — cross-fade blending via [`TransitionState`].
//! - **Priority stacking** — conflict resolution via [`PriorityStack`].
//! - **Automation** — rule evaluation via [`AutomationEngine`].

pub mod automation;
pub mod priority;
pub mod transition;

pub use automation::AutomationEngine;
pub use priority::{PriorityStack, StackEntry};
pub use transition::{TransitionState, interpolate_color, interpolate_oklab, interpolate_srgb};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::types::device::DeviceId;
use crate::types::effect::{ControlBinding, ControlValue, EffectId, EffectMetadata};
use crate::types::layer::{LayerSource, SceneLayer, SceneLayerId};
use crate::types::library::PresetId;
use crate::types::scene::{
    ColorInterpolation, DisplayFaceBlendMode, DisplayFaceTarget, EasingFunction, Scene, SceneId,
    SceneKind, SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId,
    ZoneRole,
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
    /// The supplied `layers_version` precondition is stale.
    Stale { current: u64 },
    /// The supplied layer payload violates layer-stack invariants.
    InvalidLayer { errors: Vec<String> },
    /// The requested insertion index is outside the current layer stack.
    InvalidIndex { index: usize, len: usize },
    /// The supplied order is not an exact permutation of current layer ids.
    InvalidOrder,
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

/// Central scene lifecycle manager.
///
/// Owns the scene store, the priority stack, and the active transition
/// state. The render loop calls into the manager each frame to advance
/// transitions and resolve the effective zone assignments.
#[derive(Debug)]
pub struct SceneManager {
    /// All registered scenes, keyed by [`SceneId`].
    scenes: HashMap<SceneId, Scene>,

    /// Priority stack for active scene arbitration.
    priority_stack: PriorityStack,

    /// In-progress transition (if any).
    active_transition: Option<TransitionState>,

    /// History of previously active scene IDs, most recent first.
    /// Used for restore-previous semantics.
    activation_history: Vec<SceneId>,

    /// Cached active zones for cheap frame snapshot reads.
    active_render_groups: Arc<[Zone]>,

    /// Monotonic revision for the active render-group cache.
    active_render_groups_revision: u64,
}

impl SceneManager {
    /// Create a new scene manager with no scenes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scenes: HashMap::new(),
            priority_stack: PriorityStack::new(),
            active_transition: None,
            activation_history: Vec::new(),
            active_render_groups: Arc::default(),
            active_render_groups_revision: 0,
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
            scope: crate::types::scene::SceneScope::Full,
            zone_assignments: Vec::new(),
            groups: vec![default_primary_group(layout)],
            groups_revision: 0,
            transition: TransitionSpec {
                duration_ms: 1_000,
                easing: EasingFunction::Linear,
                color_interpolation: ColorInterpolation::Oklab,
            },
            priority: ScenePriority::AMBIENT,
            enabled: true,
            metadata: HashMap::new(),
            unassigned_behavior: crate::types::scene::UnassignedBehavior::Off,
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
        let to_assignments = scene.effective_zone_assignments();
        let to_id = scene.id;

        // Capture from-state before pushing.
        let from_state = self.active_scene_id().copied();
        let from_assignments = from_state
            .as_ref()
            .and_then(|fid| self.scenes.get(fid))
            .map(Scene::effective_zone_assignments)
            .unwrap_or_default();

        // Record history.
        if let Some(prev_id) = from_state {
            self.activation_history.insert(0, prev_id);
        }

        self.priority_stack.push(to_id, priority);

        // Start transition if there's a from-scene.
        if let Some(from_id) = from_state {
            if spec.duration_ms > 0 {
                self.active_transition = Some(TransitionState::new(
                    from_id,
                    to_id,
                    spec,
                    from_assignments,
                    to_assignments,
                ));
            } else {
                // Instant activation — no transition.
                self.active_transition = None;
            }
        } else {
            self.active_transition = None;
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
            self.active_transition = None;

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

    /// Invalidate caches derived from the active zones when an
    /// external dependency changes without mutating the scene graph itself.
    pub fn invalidate_active_render_groups(&mut self) {
        self.active_render_groups_revision = self.active_render_groups_revision.saturating_add(1);
    }

    // ── Transition ──────────────────────────────────────────────────

    /// Advance the active transition by `delta_secs`.
    ///
    /// If the transition completes, it is cleared.
    pub fn tick_transition(&mut self, delta_secs: f32) {
        if let Some(ref mut transition) = self.active_transition {
            transition.tick(delta_secs);
            if transition.is_complete() {
                self.active_transition = None;
            }
        }
    }

    /// Get a reference to the active transition (if any).
    #[must_use]
    pub fn active_transition(&self) -> Option<&TransitionState> {
        self.active_transition.as_ref()
    }

    /// Whether a transition is currently in progress.
    #[must_use]
    pub fn is_transitioning(&self) -> bool {
        self.active_transition.is_some()
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
                .primary_group()
                .map(|group| group.layout.clone())
                .unwrap_or_else(|| unclaimed_primary_layout(scene, full_scope_layout))
        } else {
            full_scope_layout
        };

        let mut structural_changed = false;
        if let Some(group) = scene.primary_group_mut() {
            let effect_changed = group.effect_id != Some(effect.id);
            let control_bindings = if effect_changed {
                HashMap::new()
            } else {
                group.control_bindings.clone()
            };
            replace_legacy_effect_layer_stack(
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
            scene.groups.push(Zone {
                id: ZoneId::new(),
                name: DEFAULT_ZONE_NAME.to_owned(),
                description: Some("Default zone.".to_owned()),
                effect_id: Some(effect.id),
                controls,
                control_bindings: HashMap::new(),
                preset_id: active_preset_id,
                layers: Vec::new(),
                layout: next_primary_layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: None,
                role: ZoneRole::Primary,
                controls_version: 0,
                layers_version: 0,
            });
            structural_changed = true;
        }

        if structural_changed {
            bump_groups_revision(scene);
        }

        self.refresh_active_render_groups();
        Ok(self
            .active_scene()
            .and_then(Scene::primary_group)
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

        if let Some(group) = scene.display_group_for_mut(device_id) {
            let effect_changed = group.effect_id != Some(effect.id);
            let control_bindings = if effect_changed {
                HashMap::new()
            } else {
                group.control_bindings.clone()
            };
            replace_legacy_effect_layer_stack(group, effect.id, controls, control_bindings, None);
            group.layout = layout;
            group.display_target = Some(DisplayFaceTarget::new(device_id));
            group.enabled = true;
            group.role = ZoneRole::Display;
            if group.name.trim().is_empty() {
                group.name = format!("{device_name} Face");
            }
        } else {
            scene.groups.push(Zone {
                id: ZoneId::new(),
                name: format!("{device_name} Face"),
                description: Some(format!("Display face for {device_name}")),
                effect_id: Some(effect.id),
                controls,
                control_bindings: HashMap::new(),
                preset_id: None,
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
        }

        self.refresh_active_render_groups();
        Ok(self
            .active_scene()
            .and_then(|scene| scene.display_group_for(device_id))
            .expect("display group should exist after upsert"))
    }

    pub fn remove_display_group(&mut self, device_id: DeviceId) -> Result<bool> {
        let Some(scene) = self.active_scene_mut() else {
            bail!("no active scene");
        };
        let previous_len = scene.groups.len();
        scene.groups.retain(|group| {
            group.role != ZoneRole::Display
                || group
                    .display_target
                    .as_ref()
                    .is_none_or(|target| target.device_id != device_id)
        });
        let removed = scene.groups.len() != previous_len;
        if removed {
            self.refresh_active_render_groups();
        }
        Ok(removed)
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
            .groups
            .iter()
            .find(|group| group.display_target.is_none())
            .map_or(fallback_canvas, |group| {
                (group.layout.canvas_width, group.layout.canvas_height)
            });
        let id = ZoneId::new();
        scene.groups.push(Zone {
            id,
            name,
            description: None,
            effect_id: None,
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
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
        bump_groups_revision(scene);
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
        let Some(index) = scene.groups.iter().position(|group| group.id == group_id) else {
            return Err(ZoneMutationError::GroupMissing);
        };
        let role = scene.groups[index].role;
        if role != ZoneRole::Custom {
            return Err(ZoneMutationError::InvalidRole { role });
        }

        scene.groups.remove(index);
        bump_groups_revision(scene);
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
        let Some(index) = scene.groups.iter().position(|group| group.id == group_id) else {
            return Err(ZoneMutationError::GroupMissing);
        };

        if role_change {
            for group in &mut scene.groups {
                if group.role == ZoneRole::Primary {
                    group.role = ZoneRole::Custom;
                }
            }
            let group = &mut scene.groups[index];
            group.role = ZoneRole::Primary;
            group.display_target = None;
            bump_groups_revision(scene);
        }

        let group = &mut scene.groups[index];
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
            .groups
            .iter()
            .position(|group| group.id == group_id)
            .ok_or(ZoneMutationError::GroupMissing)?;

        let current_owner = scene.groups.iter().position(|group| {
            group
                .layout
                .zones
                .iter()
                .any(|zone| zone.id == device_zone.id)
        });

        if current_owner == Some(target_index) {
            if let Some(zone) = scene.groups[target_index]
                .layout
                .zones
                .iter_mut()
                .find(|zone| zone.id == device_zone.id)
            {
                *zone = device_zone;
            }
        } else {
            for group in &mut scene.groups {
                group.layout.zones.retain(|zone| zone.id != device_zone.id);
            }
            let slot = scene.groups[target_index].layout.zones.len();
            let mut moved = device_zone;
            reset_device_zone_placement(&mut moved, slot);
            scene.groups[target_index].layout.zones.push(moved);
        }

        bump_groups_revision(scene);
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
        for group in &mut scene.groups {
            let previous_len = group.layout.zones.len();
            group.layout.zones.retain(|zone| zone.id != device_zone_id);
            removed |= group.layout.zones.len() != previous_len;
        }
        if !removed {
            return Err(ZoneMutationError::OutputMissing);
        }
        bump_groups_revision(scene);
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
                .groups
                .iter()
                .any(|group| group.id == group_id && group.display_target.is_none())
        {
            return Err(ZoneMutationError::GroupMissing);
        }
        scene.unassigned_behavior = behavior;
        bump_groups_revision(scene);
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
            .groups
            .iter()
            .position(|group| group.id == zone_id)
            .ok_or(ZoneMutationError::GroupMissing)?;

        // The request must carry exactly the outputs the zone owns. Adds
        // and drops are not placement edits — they route through the
        // device endpoints, which keep scene-wide exclusivity intact.
        let stored_ids = scene.groups[index]
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
        let group = &mut scene.groups[index];
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
        bump_groups_revision(scene);
        if active_scene_id == Some(*scene_id) {
            self.refresh_active_render_groups();
        }
        Ok(updated)
    }

    pub fn patch_display_group_target(
        &mut self,
        group_id: ZoneId,
        blend_mode: Option<DisplayFaceBlendMode>,
        opacity: Option<f32>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.groups.iter_mut().find(|group| group.id == group_id)?;
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
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
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
                .groups
                .iter()
                .find(|group| group.id == insert.group_id)
                .ok_or(LayerMutationError::GroupMissing)?;
            if let Some(expected) = insert.expected_version
                && expected != group.layers_version
            {
                return Err(LayerMutationError::Stale {
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
            let effective_len = if group.layers.is_empty() && group.effect_id.is_some() {
                1
            } else {
                group.layers.len()
            };
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
                .groups
                .iter_mut()
                .find(|group| group.id == insert.group_id)
                .ok_or(LayerMutationError::GroupMissing)?;
            materialize_legacy_effect_layer(group);
            if let Some(index) = insert.index {
                group.layers.insert(index, insert.layer);
            } else {
                group.layers.push(insert.layer);
            }
            sync_legacy_effect_fields(group);
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
                    .groups
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
            let LayerSource::Effect {
                controls,
                preset_id,
                ..
            } = &mut layer.source
            else {
                return Err(LayerMutationError::InvalidLayer {
                    errors: vec![format!("layer {layer_id} is not an effect layer")],
                });
            };
            controls.extend(updates);
            *preset_id = None;
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
            while index < scene.groups.len() {
                let matches_device = scene.groups[index].role == ZoneRole::Display
                    && scene.groups[index]
                        .display_target
                        .as_ref()
                        .is_some_and(|target| target.device_id == device_id);
                if matches_device {
                    removed_groups.push((scene.id, scene.groups.remove(index)));
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
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or(ControlsVersionMismatch::GroupMissing)?;
        if let Some(expected_effect_id) = expected_effect_id
            && group.effect_id != Some(expected_effect_id)
        {
            // The group no longer loads the effect the caller thought
            // it was editing. Reporting `GroupMissing` (vs a new
            // "effect changed" variant) keeps the API surface small
            // and routes clients into the same "re-seed your draft
            // from /effects/active" recovery path as a true
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
        if group.layers.is_empty() {
            group.controls.extend(updates);
            group.preset_id = None;
        } else {
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
            let LayerSource::Effect {
                controls,
                preset_id,
                ..
            } = &mut group.layers[*index].source
            else {
                unreachable!("matching layer index must point to an effect layer");
            };
            controls.extend(updates);
            *preset_id = None;
            sync_legacy_effect_fields(group);
        }
        group.controls_version = group.controls_version.saturating_add(1);
        let new_version = group.controls_version;
        self.refresh_active_render_groups();
        let current = self
            .active_scene()
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
            .ok_or(ControlsVersionMismatch::GroupMissing)?;
        Ok((current, new_version))
    }

    pub fn reset_group_controls(
        &mut self,
        group_id: ZoneId,
        defaults: HashMap<String, ControlValue>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.groups.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            controls,
            preset_id,
            ..
        }) = legacy_effect_layer_source_mut(group)
        {
            *controls = defaults;
            *preset_id = None;
            sync_legacy_effect_fields(group);
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            group.controls = defaults;
            group.preset_id = None;
        }
        // Reset is a controls mutation from a concurrency standpoint
        // — any modal that opened before this call is holding a
        // stale snapshot, so its next `If-Match` PATCH must fail.
        // Same treatment as `patch_group_controls_with_precondition`.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
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
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| anyhow::anyhow!("zone {group_id:?} is not in the active scene"))?;
        if group.role == ZoneRole::Display {
            anyhow::bail!("zone {group_id:?} is a display face, not an LED zone");
        }
        let effect_changed = group.effect_id != Some(effect.id);
        let control_bindings = if effect_changed {
            HashMap::new()
        } else {
            group.control_bindings.clone()
        };
        replace_legacy_effect_layer_stack(
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
            .and_then(|scene| scene.groups.iter().find(|group| group.id == group_id))
            .ok_or_else(|| anyhow::anyhow!("zone vanished after effect apply"))
    }

    pub fn clear_group_effect(&mut self, group_id: ZoneId) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.groups.iter_mut().find(|group| group.id == group_id)?;
        group.effect_id = None;
        group.controls.clear();
        group.control_bindings.clear();
        group.preset_id = None;
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
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
    }

    pub fn set_group_control_binding(
        &mut self,
        group_id: ZoneId,
        control_id: String,
        binding: ControlBinding,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.groups.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            control_bindings,
            preset_id,
            ..
        }) = legacy_effect_layer_source_mut(group)
        {
            control_bindings.insert(control_id, binding);
            *preset_id = None;
            sync_legacy_effect_fields(group);
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            group.control_bindings.insert(control_id, binding);
            group.preset_id = None;
        }
        // Bindings surface as control values at render time, so a
        // new binding changes what the user would see if they opened
        // the modal — version must advance alongside raw control
        // mutations.
        group.controls_version = group.controls_version.saturating_add(1);
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
    }

    pub fn set_group_preset_id(
        &mut self,
        group_id: ZoneId,
        preset_id: Option<PresetId>,
    ) -> Option<&Zone> {
        let scene = self.active_scene_mut()?;
        let group = scene.groups.iter_mut().find(|group| group.id == group_id)?;
        if let Some(LayerSource::Effect {
            preset_id: layer_preset_id,
            ..
        }) = legacy_effect_layer_source_mut(group)
        {
            *layer_preset_id = preset_id;
            sync_legacy_effect_fields(group);
            group.layers_version = group.layers_version.saturating_add(1);
        } else {
            group.preset_id = preset_id;
        }
        self.refresh_active_render_groups();
        self.active_scene()
            .and_then(|active| active.groups.iter().find(|group| group.id == group_id))
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
        for group in &mut scene.groups {
            if group.role != ZoneRole::Primary || group.display_target.is_some() {
                continue;
            }
            if group.layout != *layout {
                group.layout = layout.clone();
                changed = true;
            }
        }
        if changed {
            bump_groups_revision(scene);
            self.refresh_active_render_groups();
        }
        changed
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
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or(LayerMutationError::GroupMissing)?;
        if let Some(expected) = expected_version
            && expected != group.layers_version
        {
            return Err(LayerMutationError::Stale {
                current: group.layers_version,
            });
        }

        materialize_legacy_effect_layer(group);
        mutate(group)?;
        sync_legacy_effect_fields(group);
        group.layers_version = group.layers_version.saturating_add(1);
        let new_version = group.layers_version;

        if active_scene_id == Some(scene_id) {
            self.refresh_active_render_groups();
        }
        let current = self
            .scenes
            .get(&scene_id)
            .and_then(|scene| scene.groups.iter().find(|group| group.id == group_id))
            .ok_or(LayerMutationError::GroupMissing)?;
        Ok((current, new_version))
    }

    fn refresh_active_render_groups(&mut self) {
        let next_groups = self
            .active_scene()
            .map(|scene| Arc::<[Zone]>::from(scene.groups.clone()))
            .unwrap_or_default();
        if self.active_render_groups.as_ref() != next_groups.as_ref() {
            self.active_render_groups_revision =
                self.active_render_groups_revision.saturating_add(1);
        }
        self.active_render_groups = next_groups;
    }
}

#[must_use]
pub fn default_primary_group(mut layout: SpatialLayout) -> Zone {
    DEFAULT_ZONE_NAME.clone_into(&mut layout.name);
    Zone {
        id: ZoneId::new(),
        name: DEFAULT_ZONE_NAME.to_owned(),
        description: Some("Default zone.".to_owned()),
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
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

fn materialize_legacy_effect_layer(group: &mut Zone) {
    if !group.layers.is_empty() {
        return;
    }
    let Some(effect_id) = group.effect_id else {
        return;
    };
    group.layers.push(SceneLayer::from_effect(
        group.legacy_layer_id(),
        effect_id,
        group.controls.clone(),
        group.control_bindings.clone(),
        group.preset_id,
    ));
}

fn bump_groups_revision(scene: &mut Scene) {
    scene.groups_revision = scene.groups_revision.saturating_add(1);
}

fn scene_has_custom_led_groups(scene: &Scene) -> bool {
    scene
        .groups
        .iter()
        .any(|group| group.role == ZoneRole::Custom && group.display_target.is_none())
}

fn unclaimed_primary_layout(scene: &Scene, mut full_scope_layout: SpatialLayout) -> SpatialLayout {
    let claimed = scene
        .groups
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

fn replace_legacy_effect_layer_stack(
    group: &mut Zone,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    control_bindings: HashMap<String, ControlBinding>,
    preset_id: Option<PresetId>,
) {
    group.effect_id = Some(effect_id);
    group.controls.clone_from(&controls);
    group.control_bindings.clone_from(&control_bindings);
    group.preset_id = preset_id;
    group.layers = vec![SceneLayer::from_effect(
        group.legacy_layer_id(),
        effect_id,
        controls,
        control_bindings,
        preset_id,
    )];
    group.layers_version = group.layers_version.saturating_add(1);
}

fn legacy_effect_layer_source_mut(group: &mut Zone) -> Option<&mut LayerSource> {
    group
        .layers
        .iter_mut()
        .find_map(|layer| match &mut layer.source {
            source @ LayerSource::Effect { .. } => Some(source),
            _ => None,
        })
}

fn sync_legacy_effect_fields(group: &mut Zone) {
    let legacy = group.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect {
            effect_id,
            controls,
            control_bindings,
            preset_id,
        } => Some((
            Some(*effect_id),
            controls.clone(),
            control_bindings.clone(),
            *preset_id,
        )),
        _ => None,
    });

    if let Some((effect_id, controls, control_bindings, preset_id)) = legacy {
        group.effect_id = effect_id;
        group.controls = controls;
        group.control_bindings = control_bindings;
        group.preset_id = preset_id;
    } else {
        group.effect_id = None;
        group.controls.clear();
        group.control_bindings.clear();
        group.preset_id = None;
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
    use crate::types::scene::{ColorInterpolation, EasingFunction, SceneScope, TransitionSpec};

    Scene {
        id: SceneId::new(),
        name: name.to_string(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        groups_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: crate::types::scene::UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}
