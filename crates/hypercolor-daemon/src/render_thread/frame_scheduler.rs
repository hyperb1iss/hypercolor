use hypercolor_core::spatial::SpatialEngine;

use crate::session::OutputPowerState;

use super::EffectDemand;

#[derive(Debug, Clone)]
pub(crate) struct FrameSceneSnapshot {
    pub frame_token: u64,
    pub elapsed_ms: u32,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_generation: u64,
    pub spatial_engine: SpatialEngine,
}

pub(crate) struct FrameSceneSnapshotInputs {
    pub frame_token: u64,
    pub elapsed_ms: u32,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_generation: u64,
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
            spatial_engine: inputs.spatial_engine,
        }
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::SpatialLayout;

    use crate::session::OutputPowerState;

    use super::{EffectDemand, FrameSceneSnapshotInputs, FrameScheduler};

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
            spatial_engine: empty_spatial_engine(),
        });

        assert_eq!(snapshot.frame_token, 42);
        assert_eq!(snapshot.elapsed_ms, 123);
        assert_eq!(snapshot.budget_us, 16_666);
        assert_eq!(snapshot.effect_generation, 7);
        assert!(snapshot.effect_demand.effect_running);
        assert!(snapshot.effect_demand.screen_capture_active);
        assert_eq!(snapshot.spatial_engine.layout().canvas_width, 320);
    }
}
