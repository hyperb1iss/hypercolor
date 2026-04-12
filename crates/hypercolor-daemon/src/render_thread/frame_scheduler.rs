use std::sync::Arc;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::scene::{RenderGroup, SceneId};

use crate::session::OutputPowerState;

use super::frame_state::EffectDemand;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SceneTransitionSnapshot {
    pub from_scene: Option<SceneId>,
    pub to_scene: Option<SceneId>,
    pub progress: f32,
    pub eased_progress: f32,
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
}

impl SceneRuntimeSnapshot {
    pub(crate) fn has_active_render_groups(&self) -> bool {
        self.active_render_group_count > 0
    }

    pub(crate) fn active_render_group_count(&self) -> u32 {
        self.active_render_group_count
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

    #[allow(
        clippy::unused_self,
        reason = "FrameScheduler keeps a method-shaped API for future scheduling state"
    )]
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
    use hypercolor_types::scene::{RenderGroup, RenderGroupId};
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };
    use uuid::Uuid;

    use crate::session::OutputPowerState;

    use super::{
        EffectDemand, FrameSceneSnapshotInputs, FrameScheduler, SceneRuntimeSnapshot,
        SceneTransitionSnapshot,
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

    fn sample_group() -> RenderGroup {
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Desk".into(),
            description: None,
            effect_id: Some(EffectId::from(Uuid::now_v7())),
            controls: std::collections::HashMap::new(),
            preset_id: None,
            layout: SpatialLayout {
                id: "group-layout".into(),
                name: "Group Layout".into(),
                description: None,
                canvas_width: 320,
                canvas_height: 200,
                zones: vec![DeviceZone {
                    id: "desk:main".into(),
                    name: "Desk".into(),
                    device_id: "mock:device".into(),
                    zone_name: None,
                    position: NormalizedPosition::new(0.5, 0.5),
                    size: NormalizedPosition::new(1.0, 1.0),
                    rotation: 0.0,
                    scale: 1.0,
                    display_order: 0,
                    orientation: None,
                    topology: LedTopology::Strip {
                        count: 1,
                        direction: StripDirection::LeftToRight,
                    },
                    led_positions: Vec::new(),
                    led_mapping: None,
                    sampling_mode: Some(SamplingMode::Bilinear),
                    edge_behavior: Some(EdgeBehavior::Clamp),
                    shape: None,
                    shape_preset: None,
                    attachment: None,
                }],
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
        }
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
                active_render_groups: vec![sample_group()].into(),
                active_render_groups_revision: 1,
                active_render_group_count: 1,
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
