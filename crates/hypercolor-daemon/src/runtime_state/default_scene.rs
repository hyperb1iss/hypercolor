//! Authored Default-scene fields that accompany its persisted zones.

use hypercolor_types::identity::LayoutId;
use hypercolor_types::scene::{
    Scene, SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The synthesized identity/name/kind stay local; every editable field survives
/// a restart alongside the separately owned zone list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDefaultSceneSettings {
    pub description: Option<String>,
    pub transition: TransitionSpec,
    pub priority: ScenePriority,
    pub enabled: bool,
    pub metadata: HashMap<String, String>,
    pub unassigned_behavior: UnassignedBehavior,
    pub layout_id: Option<LayoutId>,
    pub activation_brightness: Option<f32>,
    pub mutation_mode: SceneMutationMode,
}

impl From<&Scene> for RuntimeDefaultSceneSettings {
    fn from(scene: &Scene) -> Self {
        Self {
            description: scene.description.clone(),
            transition: scene.transition.clone(),
            priority: scene.priority,
            enabled: scene.enabled,
            metadata: scene.metadata.clone(),
            unassigned_behavior: scene.unassigned_behavior.clone(),
            layout_id: scene.layout_id.clone(),
            activation_brightness: scene.activation_brightness,
            mutation_mode: scene.mutation_mode,
        }
    }
}

impl RuntimeDefaultSceneSettings {
    pub(crate) fn apply(&self, scene: &mut Scene) {
        scene.description.clone_from(&self.description);
        scene.transition.clone_from(&self.transition);
        scene.priority = self.priority;
        scene.enabled = self.enabled;
        scene.metadata.clone_from(&self.metadata);
        scene
            .unassigned_behavior
            .clone_from(&self.unassigned_behavior);
        scene.layout_id.clone_from(&self.layout_id);
        scene.activation_brightness = self.activation_brightness;
        scene.mutation_mode = self.mutation_mode;
    }
}
