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

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::types::scene::{RenderGroup, Scene, SceneId, ScenePriority, TransitionSpec};

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

    /// Cached active render groups for cheap frame snapshot reads.
    active_render_groups: Arc<[RenderGroup]>,

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

    // ── CRUD ────────────────────────────────────────────────────────

    /// Register a new scene. Returns an error if a scene with the same
    /// ID already exists.
    pub fn create(&mut self, scene: Scene) -> Result<()> {
        if self.scenes.contains_key(&scene.id) {
            bail!("scene already exists: {}", scene.id);
        }
        if let Err(conflicts) = scene.validate_group_exclusivity() {
            bail!(
                "scene '{}' has overlapping render groups: {}",
                scene.name,
                conflicts.join("; ")
            );
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
        if !self.scenes.contains_key(&scene.id) {
            bail!("scene not found: {}", scene.id);
        }
        if let Err(conflicts) = scene.validate_group_exclusivity() {
            bail!(
                "scene '{}' has overlapping render groups: {}",
                scene.name,
                conflicts.join("; ")
            );
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

    /// Get the cached active render groups for cheap frame snapshots.
    #[must_use]
    pub fn active_render_groups(&self) -> Arc<[RenderGroup]> {
        Arc::clone(&self.active_render_groups)
    }

    /// Monotonic revision of the cached active render groups.
    #[must_use]
    pub fn active_render_groups_revision(&self) -> u64 {
        self.active_render_groups_revision
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

    fn refresh_active_render_groups(&mut self) {
        let next_groups = self
            .active_scene()
            .map(|scene| Arc::<[RenderGroup]>::from(scene.groups.clone()))
            .unwrap_or_default();
        if self.active_render_groups.as_ref() != next_groups.as_ref() {
            self.active_render_groups_revision =
                self.active_render_groups_revision.saturating_add(1);
        }
        self.active_render_groups = next_groups;
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
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: crate::types::scene::UnassignedBehavior::Off,
    }
}
