use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::scene::{ColorInterpolation, RenderGroup, RenderGroupId, SceneId};

use crate::session::OutputPowerState;

use super::scene_dependency::SceneDependencyKey;

#[derive(Debug, Clone)]
pub(crate) struct SceneTransitionSnapshot {
    pub from_scene: Option<SceneId>,
    pub to_scene: Option<SceneId>,
    pub progress: f32,
    pub eased_progress: f32,
    pub color_interpolation: ColorInterpolation,
}

impl Default for SceneTransitionSnapshot {
    fn default() -> Self {
        Self {
            from_scene: None,
            to_scene: None,
            progress: 0.0,
            eased_progress: 0.0,
            color_interpolation: ColorInterpolation::Srgb,
        }
    }
}

#[derive(Debug, Clone, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the `active_` prefix keeps the runtime snapshot aligned with scene manager terminology"
)]
pub(crate) struct SceneRuntimeSnapshot {
    pub active_scene_id: Option<SceneId>,
    pub active_transition: Option<SceneTransitionSnapshot>,
    pub active_render_groups: Arc<[RenderGroup]>,
    pub active_render_groups_revision: u64,
    pub active_render_group_count: u32,
    pub active_display_group_target_fps: HashMap<RenderGroupId, u32>,
}

impl SceneRuntimeSnapshot {
    pub(crate) fn active_render_group_count(&self) -> u32 {
        self.active_render_group_count
    }

    pub(crate) const fn dependency_key(&self, dependency_generation: u64) -> SceneDependencyKey {
        SceneDependencyKey::new(self.active_render_groups_revision, dependency_generation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectDemand {
    pub(crate) effect_running: bool,
    pub(crate) audio_capture_active: bool,
    pub(crate) screen_capture_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectSceneSnapshot {
    pub(crate) demand: EffectDemand,
    pub(crate) dependency_key: SceneDependencyKey,
}

#[derive(Debug, Clone)]
pub(crate) struct FrameSceneSnapshot {
    pub frame_token: u64,
    pub elapsed_ms: u32,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_dependency_key: SceneDependencyKey,
    pub scene_runtime: SceneRuntimeSnapshot,
    pub spatial_engine: SpatialEngine,
}

#[derive(Debug, Clone, Default)]
struct CachedDisplayGroupTargetFps {
    dependency_key: SceneDependencyKey,
    values: HashMap<RenderGroupId, u32>,
}

#[derive(Debug, Clone, Copy)]
struct CachedEffectDemand {
    dependency_key: SceneDependencyKey,
    screen_capture_configured: bool,
    demand: EffectDemand,
}

#[derive(Debug, Default)]
pub(crate) struct SceneSnapshotCache {
    cached_display_group_target_fps: Option<CachedDisplayGroupTargetFps>,
    cached_effect_demand: Option<CachedEffectDemand>,
}

impl SceneSnapshotCache {
    pub const fn new() -> Self {
        Self {
            cached_display_group_target_fps: None,
            cached_effect_demand: None,
        }
    }

    pub(crate) fn cached_display_group_target_fps(
        &self,
        dependency_key: SceneDependencyKey,
    ) -> Option<HashMap<RenderGroupId, u32>> {
        self.cached_display_group_target_fps
            .as_ref()
            .filter(|cache| cache.dependency_key == dependency_key)
            .map(|cache| cache.values.clone())
    }

    pub(crate) fn cache_display_group_target_fps(
        &mut self,
        dependency_key: SceneDependencyKey,
        values: &HashMap<RenderGroupId, u32>,
    ) {
        self.cached_display_group_target_fps = Some(CachedDisplayGroupTargetFps {
            dependency_key,
            values: values.clone(),
        });
    }

    pub(crate) fn cached_effect_demand(
        &self,
        dependency_key: SceneDependencyKey,
        screen_capture_configured: bool,
    ) -> Option<EffectDemand> {
        self.cached_effect_demand
            .filter(|cache| {
                cache.dependency_key == dependency_key
                    && cache.screen_capture_configured == screen_capture_configured
            })
            .map(|cache| cache.demand)
    }

    pub(crate) fn cache_effect_demand(
        &mut self,
        dependency_key: SceneDependencyKey,
        screen_capture_configured: bool,
        demand: EffectDemand,
    ) {
        self.cached_effect_demand = Some(CachedEffectDemand {
            dependency_key,
            screen_capture_configured,
            demand,
        });
    }

}

#[cfg(test)]
mod tests {
    use hypercolor_types::scene::RenderGroupId;

    use super::{EffectDemand, SceneDependencyKey, SceneSnapshotCache};

    #[test]
    fn scene_snapshot_cache_caches_display_group_target_fps_by_revision_and_registry_generation() {
        let mut scheduler = SceneSnapshotCache::new();
        let group_id = RenderGroupId::new();
        let values = std::collections::HashMap::from([(group_id, 30)]);
        let dependency_key = SceneDependencyKey::new(1, 7);

        assert!(
            scheduler
                .cached_display_group_target_fps(dependency_key)
                .is_none()
        );

        scheduler.cache_display_group_target_fps(dependency_key, &values);

        assert_eq!(
            scheduler.cached_display_group_target_fps(dependency_key),
            Some(values.clone())
        );
        assert!(
            scheduler
                .cached_display_group_target_fps(SceneDependencyKey::new(2, 7))
                .is_none()
        );
        assert!(
            scheduler
                .cached_display_group_target_fps(SceneDependencyKey::new(1, 8))
                .is_none()
        );
    }

    #[test]
    fn scene_snapshot_cache_caches_effect_demand_by_dependency_key_and_capture_mode() {
        let mut scheduler = SceneSnapshotCache::new();
        let dependency_key = SceneDependencyKey::new(1, 7);
        let demand = EffectDemand {
            effect_running: true,
            audio_capture_active: true,
            screen_capture_active: false,
        };

        assert!(scheduler.cached_effect_demand(dependency_key, false).is_none());

        scheduler.cache_effect_demand(dependency_key, false, demand);

        assert_eq!(scheduler.cached_effect_demand(dependency_key, false), Some(demand));
        assert!(
            scheduler
                .cached_effect_demand(SceneDependencyKey::new(2, 7), false)
                .is_none()
        );
        assert!(
            scheduler
                .cached_effect_demand(SceneDependencyKey::new(1, 8), false)
                .is_none()
        );
        assert!(scheduler.cached_effect_demand(dependency_key, true).is_none());
    }
}
