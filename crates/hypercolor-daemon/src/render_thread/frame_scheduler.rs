use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::effect::EffectId;
use hypercolor_types::scene::{RenderGroupId, SceneId};

use crate::session::OutputPowerState;

use super::EffectDemand;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SceneTransitionSnapshot {
    pub from_scene: Option<SceneId>,
    pub to_scene: Option<SceneId>,
    pub progress: f32,
    pub eased_progress: f32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RenderGroupSnapshot {
    #[allow(
        dead_code,
        reason = "render-group producer runtimes will key off this id once multi-producer composition is live"
    )]
    pub id: RenderGroupId,
    pub effect_id: Option<EffectId>,
    pub enabled: bool,
    pub zone_ids: Vec<String>,
}

impl RenderGroupSnapshot {
    pub(crate) fn participates_in_composition(&self) -> bool {
        self.enabled && self.effect_id.is_some() && !self.zone_ids.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SceneRuntimeSnapshot {
    pub active_scene_id: Option<SceneId>,
    pub active_transition: Option<SceneTransitionSnapshot>,
    pub active_groups: Vec<RenderGroupSnapshot>,
}

impl SceneRuntimeSnapshot {
    pub(crate) fn active_render_group_count(&self) -> u32 {
        u32::try_from(
            self.active_groups
                .iter()
                .filter(|group| group.participates_in_composition())
                .count(),
        )
        .unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FrameSceneSnapshot {
    pub frame_token: u64,
    pub elapsed_ms: u32,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_generation: u64,
    pub scene_runtime: SceneRuntimeSnapshot,
    pub spatial_engine: SpatialEngine,
}

pub(crate) struct FrameSceneSnapshotInputs {
    pub frame_token: u64,
    pub elapsed_ms: u32,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_generation: u64,
    pub scene_runtime: SceneRuntimeSnapshot,
    pub spatial_engine: SpatialEngine,
}

#[derive(Debug, Default)]
pub(crate) struct FrameScheduler;

impl FrameScheduler {
    pub const fn new() -> Self {
        Self
    }

    pub fn build_snapshot(&mut self, inputs: FrameSceneSnapshotInputs) -> FrameSceneSnapshot {
        FrameSceneSnapshot {
            frame_token: inputs.frame_token,
            elapsed_ms: inputs.elapsed_ms,
            budget_us: inputs.budget_us,
            output_power: inputs.output_power,
            effect_demand: inputs.effect_demand,
            effect_generation: inputs.effect_generation,
            scene_runtime: inputs.scene_runtime,
            spatial_engine: inputs.spatial_engine,
        }
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::scene::RenderGroupId;
    use hypercolor_types::spatial::SpatialLayout;
    use uuid::Uuid;

    use crate::session::OutputPowerState;

    use super::{
        EffectDemand, FrameSceneSnapshotInputs, FrameScheduler, RenderGroupSnapshot,
        SceneRuntimeSnapshot, SceneTransitionSnapshot,
    };

    fn empty_spatial_engine() -> SpatialEngine {
        SpatialEngine::new(SpatialLayout {
            id: "test".into(),
            name: "Test".into(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
            default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        })
    }

    #[test]
    fn frame_scheduler_builds_snapshot_from_frame_inputs() {
        let mut scheduler = FrameScheduler::new();

        let snapshot = scheduler.build_snapshot(FrameSceneSnapshotInputs {
            frame_token: 42,
            elapsed_ms: 123,
            budget_us: 16_666,
            output_power: OutputPowerState::default(),
            effect_demand: EffectDemand {
                effect_running: true,
                audio_capture_active: false,
                screen_capture_active: true,
            },
            effect_generation: 7,
            scene_runtime: SceneRuntimeSnapshot {
                active_scene_id: None,
                active_transition: Some(SceneTransitionSnapshot {
                    from_scene: None,
                    to_scene: None,
                    progress: 0.25,
                    eased_progress: 0.5,
                }),
                active_groups: vec![RenderGroupSnapshot {
                    id: RenderGroupId::new(),
                    effect_id: Some(EffectId::from(Uuid::now_v7())),
                    enabled: true,
                    zone_ids: vec!["desk:main".into()],
                }],
            },
            spatial_engine: empty_spatial_engine(),
        });

        assert_eq!(snapshot.frame_token, 42);
        assert_eq!(snapshot.elapsed_ms, 123);
        assert_eq!(snapshot.budget_us, 16_666);
        assert_eq!(snapshot.effect_generation, 7);
        assert!(snapshot.effect_demand.effect_running);
        assert!(snapshot.effect_demand.screen_capture_active);
        assert!(snapshot.scene_runtime.active_transition.is_some());
        assert_eq!(snapshot.scene_runtime.active_render_group_count(), 1);
        assert_eq!(snapshot.spatial_engine.layout().canvas_width, 320);
    }
}
