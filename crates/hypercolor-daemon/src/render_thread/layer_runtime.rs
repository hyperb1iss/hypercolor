use std::collections::{HashMap, HashSet};

use hypercolor_types::event::{HypercolorEvent, LayerHealth};
use hypercolor_types::layer::SceneLayerId;
use hypercolor_types::scene::{RenderGroup, RenderGroupId, SceneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LayerRuntimeKey {
    pub(crate) group_id: RenderGroupId,
    pub(crate) layer_id: SceneLayerId,
}

impl LayerRuntimeKey {
    pub(crate) const fn new(group_id: RenderGroupId, layer_id: SceneLayerId) -> Self {
        Self { group_id, layer_id }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LayerRuntimeState {
    scene_id: SceneId,
    health: LayerHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingLayerHealthEvent {
    scene_id: SceneId,
    key: LayerRuntimeKey,
    health: LayerHealth,
}

#[derive(Debug, Default)]
pub(crate) struct LayerRuntimeRegistry {
    states: HashMap<LayerRuntimeKey, LayerRuntimeState>,
    pending: Vec<PendingLayerHealthEvent>,
}

impl LayerRuntimeRegistry {
    pub(crate) fn reconcile(&mut self, active_scene_id: Option<SceneId>, groups: &[RenderGroup]) {
        let scene_id = active_scene_id.unwrap_or(SceneId::DEFAULT);
        let active_keys = active_layer_keys(groups);
        self.states.retain(|key, state| {
            if !active_keys.contains(key) {
                return false;
            }
            state.scene_id == scene_id
        });
        self.pending
            .retain(|event| active_keys.contains(&event.key) && event.scene_id == scene_id);
    }

    pub(crate) fn clear(&mut self) {
        self.states.clear();
        self.pending.clear();
    }

    pub(crate) fn note_health(
        &mut self,
        active_scene_id: Option<SceneId>,
        group_id: RenderGroupId,
        layer_id: SceneLayerId,
        health: LayerHealth,
    ) {
        let scene_id = active_scene_id.unwrap_or(SceneId::DEFAULT);
        let key = LayerRuntimeKey::new(group_id, layer_id);
        if self
            .states
            .get(&key)
            .is_some_and(|state| state.scene_id == scene_id && state.health == health)
        {
            return;
        }

        self.states.insert(
            key,
            LayerRuntimeState {
                scene_id,
                health: health.clone(),
            },
        );
        self.pending
            .retain(|event| event.scene_id != scene_id || event.key != key);
        self.pending.push(PendingLayerHealthEvent {
            scene_id,
            key,
            health,
        });
    }

    pub(crate) fn drain_events(&mut self) -> Vec<HypercolorEvent> {
        self.pending
            .drain(..)
            .map(|event| HypercolorEvent::LayerHealthChanged {
                scene_id: event.scene_id,
                group_id: event.key.group_id,
                layer_id: event.key.layer_id,
                health: event.health,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.states.len()
    }

    #[cfg(test)]
    pub(crate) fn health(&self, key: LayerRuntimeKey) -> Option<&LayerHealth> {
        self.states.get(&key).map(|state| &state.health)
    }
}

fn active_layer_keys(groups: &[RenderGroup]) -> HashSet<LayerRuntimeKey> {
    groups
        .iter()
        .filter(|group| group.enabled)
        .flat_map(|group| {
            group
                .effective_layers()
                .into_iter()
                .filter(|layer| layer.enabled)
                .map(|layer| LayerRuntimeKey::new(group.id, layer.id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::SceneLayer;
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
    use uuid::Uuid;

    use super::*;

    fn sample_group() -> RenderGroup {
        let effect_id = EffectId::from(Uuid::now_v7());
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Layer Runtime".into(),
            description: None,
            effect_id: Some(effect_id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layers: vec![SceneLayer::from_effect(
                SceneLayerId::new(),
                effect_id,
                HashMap::new(),
                HashMap::new(),
                None,
            )],
            layout: SpatialLayout {
                id: "layer-runtime".into(),
                name: "Layer Runtime".into(),
                description: None,
                canvas_width: 4,
                canvas_height: 4,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: hypercolor_types::scene::RenderGroupRole::Custom,
            controls_version: 0,
            layers_version: 0,
        }
    }

    #[test]
    fn layer_health_events_coalesce_per_layer() {
        let group = sample_group();
        let layer_id = group.layers[0].id;
        let mut registry = LayerRuntimeRegistry::default();

        registry.note_health(
            Some(SceneId::DEFAULT),
            group.id,
            layer_id,
            LayerHealth::Loading,
        );
        registry.note_health(
            Some(SceneId::DEFAULT),
            group.id,
            layer_id,
            LayerHealth::Active,
        );

        let events = registry.drain_events();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            HypercolorEvent::LayerHealthChanged {
                health: LayerHealth::Active,
                ..
            }
        ));
    }

    #[test]
    fn unchanged_layer_health_does_not_emit_again() {
        let group = sample_group();
        let layer_id = group.layers[0].id;
        let mut registry = LayerRuntimeRegistry::default();

        registry.note_health(
            Some(SceneId::DEFAULT),
            group.id,
            layer_id,
            LayerHealth::Active,
        );
        assert_eq!(registry.drain_events().len(), 1);
        registry.note_health(
            Some(SceneId::DEFAULT),
            group.id,
            layer_id,
            LayerHealth::Active,
        );

        assert!(registry.drain_events().is_empty());
    }

    #[test]
    fn removed_layers_clear_runtime_state() {
        let group = sample_group();
        let key = LayerRuntimeKey::new(group.id, group.layers[0].id);
        let mut registry = LayerRuntimeRegistry::default();
        registry.note_health(
            Some(SceneId::DEFAULT),
            key.group_id,
            key.layer_id,
            LayerHealth::Active,
        );

        registry.reconcile(Some(SceneId::DEFAULT), &[]);

        assert_eq!(registry.len(), 0);
        assert_eq!(registry.health(key), None);
    }
}
