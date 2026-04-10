use hypercolor_types::scene::SceneId;

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

    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CompiledCompositionMetadata {
    pub logical_layer_count: u32,
    pub render_group_count: u32,
    pub scene_active: bool,
    pub transition_active: bool,
}

pub(crate) struct CompiledCompositionPlan {
    pub plan: CompositionPlan,
    pub metadata: CompiledCompositionMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneTransitionKey {
    from_scene: SceneId,
    to_scene: SceneId,
}

#[derive(Debug, Default)]
pub(crate) struct CompositionPlanner {
    active_transition: Option<SceneTransitionKey>,
    transition_base_frame: Option<ProducerFrame>,
    last_stable_frame: Option<ProducerFrame>,
}

impl CompositionPlanner {
    pub const fn new() -> Self {
        Self {
            active_transition: None,
            transition_base_frame: None,
            last_stable_frame: None,
        }
    }

    pub(crate) fn compile(
        &mut self,
        width: u32,
        height: u32,
        scene_runtime: &SceneRuntimeSnapshot,
        layers: Vec<PlannedSceneLayer>,
    ) -> CompiledCompositionPlan {
        let metadata = composition_metadata(scene_runtime, layers.len());
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

    pub(crate) fn compile_primary_frame(
        &mut self,
        width: u32,
        height: u32,
        scene_runtime: &SceneRuntimeSnapshot,
        current_frame: ProducerFrame,
    ) -> CompiledCompositionPlan {
        if transition_key(scene_runtime).is_none() {
            self.active_transition = None;
            self.transition_base_frame = None;
            self.last_stable_frame = Some(current_frame.clone());
            return CompiledCompositionPlan {
                plan: CompositionPlan::single(
                    width,
                    height,
                    CompositionLayer::replace(current_frame),
                ),
                metadata: composition_metadata(scene_runtime, 1),
            };
        }

        let layers = self.transition_layers(scene_runtime, &current_frame);
        let compiled = self.compile(width, height, scene_runtime, layers);
        if self.active_transition.is_none() {
            self.last_stable_frame = Some(current_frame);
        }
        compiled
    }

    fn transition_layers(
        &mut self,
        scene_runtime: &SceneRuntimeSnapshot,
        current_frame: &ProducerFrame,
    ) -> Vec<PlannedSceneLayer> {
        let transition = scene_runtime.active_transition.as_ref();
        let transition_key = transition_key(scene_runtime);

        match transition_key {
            Some(key) => {
                if self.active_transition != Some(key) {
                    self.active_transition = Some(key);
                    self.transition_base_frame = self
                        .last_stable_frame
                        .clone()
                        .or_else(|| Some(current_frame.clone()));
                }

                let opacity = transition
                    .map(|transition| transition.eased_progress.clamp(0.0, 1.0))
                    .unwrap_or(1.0);
                let mut layers = Vec::with_capacity(2);
                if let Some(base_frame) = self.transition_base_frame.clone() {
                    layers.push(PlannedSceneLayer::replace(base_frame));
                }
                if opacity < 1.0 {
                    layers.push(PlannedSceneLayer::alpha(current_frame.clone(), opacity));
                } else {
                    layers.push(PlannedSceneLayer::replace(current_frame.clone()));
                }
                layers
            }
            None => {
                self.active_transition = None;
                self.transition_base_frame = None;
                vec![PlannedSceneLayer::replace(current_frame.clone())]
            }
        }
    }
}

fn composition_metadata(
    scene_runtime: &SceneRuntimeSnapshot,
    logical_layer_count: usize,
) -> CompiledCompositionMetadata {
    let logical_layer_count = u32::try_from(logical_layer_count).unwrap_or(u32::MAX);
    let transition_active = scene_runtime
        .active_transition
        .as_ref()
        .is_some_and(|transition| {
            transition.progress < 1.0
                || transition.eased_progress < 1.0
                || transition.from_scene.is_some()
                || transition.to_scene.is_some()
        });

    CompiledCompositionMetadata {
        logical_layer_count,
        render_group_count: scene_runtime.active_render_group_count(),
        scene_active: scene_runtime.active_scene_id.is_some(),
        transition_active,
    }
}

fn transition_key(scene_runtime: &SceneRuntimeSnapshot) -> Option<SceneTransitionKey> {
    let transition = scene_runtime.active_transition.as_ref()?;
    Some(SceneTransitionKey {
        from_scene: transition.from_scene?,
        to_scene: transition.to_scene?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_core::types::canvas::{Canvas, Rgba};
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::scene::{RenderGroup, RenderGroupId};
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };
    use uuid::Uuid;

    use super::{CompositionPlanner, PlannedSceneLayer};
    use crate::render_thread::frame_scheduler::{SceneRuntimeSnapshot, SceneTransitionSnapshot};
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::sparkleflinger::SparkleFlinger;

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    fn sample_group() -> RenderGroup {
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Desk".into(),
            description: None,
            effect_id: Some(EffectId::from(Uuid::now_v7())),
            controls: HashMap::new(),
            preset_id: None,
            layout: SpatialLayout {
                id: "desk".into(),
                name: "Desk".into(),
                description: None,
                canvas_width: 2,
                canvas_height: 2,
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
        }
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
                active_render_groups: vec![sample_group()].into(),
                active_render_groups_revision: 1,
                active_render_group_count: 1,
            },
            vec![PlannedSceneLayer::replace(ProducerFrame::Canvas(
                solid_canvas(Rgba::new(12, 34, 56, 255)),
            ))],
        );

        assert_eq!(compiled.metadata.logical_layer_count, 1);
        assert_eq!(compiled.metadata.render_group_count, 1);
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
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
        let composed = sparkleflinger.compose(compiled.plan);

        assert_eq!(compiled.metadata.logical_layer_count, 2);
        assert_eq!(compiled.metadata.render_group_count, 0);
        assert!(!composed.bypassed);
        assert_eq!(composed.sampling_canvas.width(), 2);
        assert_eq!(composed.sampling_canvas.height(), 2);
    }

    #[test]
    fn planner_crossfades_from_last_stable_frame_during_scene_transition() {
        let mut planner = CompositionPlanner::new();
        let stable = ProducerFrame::Canvas(solid_canvas(Rgba::new(255, 0, 0, 255)));
        let entering = ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 255, 255)));
        let stable_runtime = SceneRuntimeSnapshot::default();
        let _ = planner.compile_primary_frame(2, 2, &stable_runtime, stable);

        let transition_runtime = SceneRuntimeSnapshot {
            active_scene_id: Some(hypercolor_types::scene::SceneId::new()),
            active_transition: Some(SceneTransitionSnapshot {
                from_scene: Some(hypercolor_types::scene::SceneId::new()),
                to_scene: Some(hypercolor_types::scene::SceneId::new()),
                progress: 0.5,
                eased_progress: 0.5,
            }),
            active_render_groups: Vec::new().into(),
            active_render_groups_revision: 0,
            active_render_group_count: 0,
        };
        let compiled = planner.compile_primary_frame(2, 2, &transition_runtime, entering);
        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Cpu);
        let composed = sparkleflinger.compose(compiled.plan);

        assert_eq!(compiled.metadata.logical_layer_count, 2);
        assert_eq!(compiled.metadata.render_group_count, 0);
        assert!(!composed.bypassed);
        let pixel = &composed.sampling_canvas.as_rgba_bytes()[0..4];
        assert_ne!(pixel, [255, 0, 0, 255].as_slice());
        assert_ne!(pixel, [0, 0, 255, 255].as_slice());
    }
}
