use super::frame_scheduler::SceneRuntimeSnapshot;
use super::producer_queue::ProducerFrame;
use super::sparkleflinger::{CompositionLayer, CompositionMode, CompositionPlan};

#[derive(Debug, Clone)]
pub(crate) struct PlannedSceneLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
}

impl PlannedSceneLayer {
    pub(crate) fn replace(frame: ProducerFrame) -> Self {
        Self {
            frame,
            mode: CompositionMode::Replace,
            opacity: 1.0,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 4 planner tests exercise layered compilation before multi-producer plans are live"
    )]
    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 4 planner tests exercise layered compilation before multi-producer plans are live"
    )]
    pub(crate) fn add(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Add,
            opacity,
        }
    }

    #[allow(
        dead_code,
        reason = "Wave 4 planner tests exercise layered compilation before multi-producer plans are live"
    )]
    pub(crate) fn screen(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Screen,
            opacity,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CompiledCompositionMetadata {
    pub logical_layer_count: u32,
    pub scene_active: bool,
    pub transition_active: bool,
}

pub(crate) struct CompiledCompositionPlan {
    pub plan: CompositionPlan,
    pub metadata: CompiledCompositionMetadata,
}

#[derive(Debug, Default)]
pub(crate) struct CompositionPlanner;

impl CompositionPlanner {
    pub const fn new() -> Self {
        Self
    }

    pub(crate) fn compile(
        &mut self,
        width: u32,
        height: u32,
        scene_runtime: &SceneRuntimeSnapshot,
        layers: Vec<PlannedSceneLayer>,
    ) -> CompiledCompositionPlan {
        let logical_layer_count = u32::try_from(layers.len()).unwrap_or(u32::MAX);
        let transition_active =
            scene_runtime
                .active_transition
                .as_ref()
                .is_some_and(|transition| {
                    transition.progress < 1.0
                        || transition.eased_progress < 1.0
                        || transition.from_scene.is_some()
                        || transition.to_scene.is_some()
                });
        let metadata = CompiledCompositionMetadata {
            logical_layer_count,
            scene_active: scene_runtime.active_scene_id.is_some(),
            transition_active,
        };
        let composition_layers = layers
            .into_iter()
            .map(|layer| CompositionLayer::from_parts(layer.frame, layer.mode, layer.opacity))
            .collect::<Vec<_>>();
        let plan = if composition_layers.len() == 1 {
            let layer = composition_layers
                .into_iter()
                .next()
                .expect("single layer should exist");
            CompositionPlan::single(width, height, layer)
        } else {
            CompositionPlan::with_layers(width, height, composition_layers)
        };

        CompiledCompositionPlan { plan, metadata }
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{Canvas, Rgba};

    use super::{CompositionPlanner, PlannedSceneLayer};
    use crate::render_thread::frame_scheduler::{SceneRuntimeSnapshot, SceneTransitionSnapshot};
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::sparkleflinger::SparkleFlinger;

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    #[test]
    fn planner_marks_scene_transition_metadata() {
        let mut planner = CompositionPlanner::new();
        let compiled = planner.compile(
            2,
            2,
            &SceneRuntimeSnapshot {
                active_scene_id: Some(hypercolor_types::scene::SceneId::new()),
                active_transition: Some(SceneTransitionSnapshot {
                    from_scene: Some(hypercolor_types::scene::SceneId::new()),
                    to_scene: Some(hypercolor_types::scene::SceneId::new()),
                    progress: 0.25,
                    eased_progress: 0.5,
                }),
            },
            vec![PlannedSceneLayer::replace(ProducerFrame::Canvas(
                solid_canvas(Rgba::new(12, 34, 56, 255)),
            ))],
        );

        assert_eq!(compiled.metadata.logical_layer_count, 1);
        assert!(compiled.metadata.scene_active);
        assert!(compiled.metadata.transition_active);
    }

    #[test]
    fn planner_compiles_multi_layer_plan_for_sparkleflinger() {
        let mut planner = CompositionPlanner::new();
        let compiled = planner.compile(
            2,
            2,
            &SceneRuntimeSnapshot::default(),
            vec![
                PlannedSceneLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::new(
                    255, 0, 0, 255,
                )))),
                PlannedSceneLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 255, 255))),
                    0.5,
                ),
            ],
        );
        let mut sparkleflinger = SparkleFlinger::new();
        let composed = sparkleflinger.compose(compiled.plan);

        assert_eq!(compiled.metadata.logical_layer_count, 2);
        assert!(!composed.bypassed);
        assert_eq!(composed.sampling_canvas.width(), 2);
        assert_eq!(composed.sampling_canvas.height(), 2);
    }
}
