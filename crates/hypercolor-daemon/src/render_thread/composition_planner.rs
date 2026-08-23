use hypercolor_types::scene::SceneId;

use super::producer_queue::ProducerFrame;
use super::scene_snapshot::SceneRuntimeSnapshot;
use super::sparkleflinger::{CompositionLayer, CompositionMode, CompositionPlan};

#[derive(Debug, Clone)]
pub(crate) struct PlannedSceneLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
    opaque_hint: bool,
}

impl PlannedSceneLayer {
    pub(crate) fn replace(frame: ProducerFrame, opaque_hint: bool) -> Self {
        Self {
            frame,
            mode: CompositionMode::Replace,
            opacity: 1.0,
            opaque_hint,
        }
    }

    pub(crate) fn alpha(frame: ProducerFrame, opacity: f32, opaque_hint: bool) -> Self {
        Self {
            frame,
            mode: CompositionMode::Alpha,
            opacity,
            opaque_hint,
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

pub(crate) struct TransitionHandoff {
    pub(crate) frame: Option<ProducerFrame>,
    pub(crate) opaque: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneTransitionKey {
    epoch: u64,
    from_scene: SceneId,
    to_scene: SceneId,
}

#[derive(Debug, Default)]
pub(crate) struct CompositionPlanner {
    active_transition: Option<SceneTransitionKey>,
    transition_base_frame: Option<ProducerFrame>,
    transition_base_opaque: bool,
    last_stable_frame: Option<ProducerFrame>,
    last_stable_opaque: bool,
    last_composed_frame: Option<ProducerFrame>,
    last_composed_opaque: bool,
}

impl CompositionPlanner {
    pub const fn new() -> Self {
        Self {
            active_transition: None,
            transition_base_frame: None,
            transition_base_opaque: false,
            last_stable_frame: None,
            last_stable_opaque: false,
            last_composed_frame: None,
            last_composed_opaque: false,
        }
    }

    pub(crate) fn take_interruption_handoff(
        &mut self,
        scene_runtime: &SceneRuntimeSnapshot,
    ) -> Option<TransitionHandoff> {
        let interrupts_active = self.active_transition.is_some()
            && transition_key(scene_runtime).is_some_and(|key| Some(key) != self.active_transition);
        if !interrupts_active {
            return None;
        }

        let frame = self.last_composed_frame.take();
        let opaque = self.last_composed_opaque;
        self.active_transition = None;
        self.transition_base_frame = None;
        self.transition_base_opaque = false;
        self.last_stable_frame = None;
        self.last_stable_opaque = false;
        self.last_composed_opaque = false;
        Some(TransitionHandoff { frame, opaque })
    }

    pub(crate) fn observe_composed_frame(
        &mut self,
        scene_runtime: &SceneRuntimeSnapshot,
        frame: Option<ProducerFrame>,
    ) {
        if transition_key(scene_runtime).is_some() {
            self.last_composed_frame = frame;
        } else {
            self.last_composed_frame = None;
            self.last_composed_opaque = false;
        }
    }

    pub(crate) fn compile(
        width: u32,
        height: u32,
        scene_runtime: &SceneRuntimeSnapshot,
        layers: Vec<PlannedSceneLayer>,
    ) -> CompiledCompositionPlan {
        let metadata = composition_metadata(scene_runtime, layers.len());
        let composition_layers = layers
            .into_iter()
            .map(|layer| {
                CompositionLayer::from_parts(
                    layer.frame,
                    layer.mode,
                    layer.opacity,
                    layer.opaque_hint,
                )
            })
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
        current_frame_opaque: bool,
        interrupted_frame: Option<(ProducerFrame, bool)>,
    ) -> CompiledCompositionPlan {
        if transition_key(scene_runtime).is_none() {
            self.active_transition = None;
            self.transition_base_frame = None;
            self.transition_base_opaque = false;
            self.last_composed_frame = None;
            self.last_composed_opaque = false;
            self.last_stable_frame = Some(current_frame.clone());
            self.last_stable_opaque = current_frame_opaque;
            return CompiledCompositionPlan {
                plan: CompositionPlan::single(
                    width,
                    height,
                    CompositionLayer::from_parts(
                        current_frame,
                        CompositionMode::Replace,
                        1.0,
                        current_frame_opaque,
                    ),
                ),
                metadata: composition_metadata(scene_runtime, 1),
            };
        }

        let layers = self.transition_layers(
            scene_runtime,
            &current_frame,
            current_frame_opaque,
            interrupted_frame,
        );
        Self::compile(width, height, scene_runtime, layers)
    }

    fn transition_layers(
        &mut self,
        scene_runtime: &SceneRuntimeSnapshot,
        current_frame: &ProducerFrame,
        current_frame_opaque: bool,
        interrupted_frame: Option<(ProducerFrame, bool)>,
    ) -> Vec<PlannedSceneLayer> {
        let transition = scene_runtime.active_transition.as_ref();
        let transition_key = transition_key(scene_runtime);

        if let Some(key) = transition_key {
            if self.active_transition != Some(key) {
                self.active_transition = Some(key);
                let (interrupted_frame, interrupted_opaque) = interrupted_frame.unzip();
                self.transition_base_opaque = interrupted_opaque.unwrap_or_else(|| {
                    if self.last_stable_frame.is_some() {
                        self.last_stable_opaque
                    } else {
                        current_frame_opaque
                    }
                });
                self.transition_base_frame = interrupted_frame
                    .or_else(|| self.last_stable_frame.clone())
                    .or_else(|| Some(current_frame.clone()));
            }

            let opacity =
                transition.map_or(1.0, |transition| transition.eased_progress.clamp(0.0, 1.0));
            self.last_composed_opaque =
                self.transition_base_opaque || opacity >= 1.0 && current_frame_opaque;
            let mut layers = Vec::with_capacity(2);
            if let Some(base_frame) = self.transition_base_frame.clone() {
                layers.push(PlannedSceneLayer::replace(
                    base_frame,
                    self.transition_base_opaque,
                ));
            }
            if opacity < 1.0 {
                layers.push(PlannedSceneLayer::alpha(
                    current_frame.clone(),
                    opacity,
                    current_frame_opaque,
                ));
            } else {
                layers.push(PlannedSceneLayer::replace(
                    current_frame.clone(),
                    current_frame_opaque,
                ));
            }
            layers
        } else {
            self.active_transition = None;
            self.transition_base_frame = None;
            self.transition_base_opaque = false;
            vec![PlannedSceneLayer::replace(
                current_frame.clone(),
                current_frame_opaque,
            )]
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
        epoch: transition.epoch,
        from_scene: transition.from_scene?,
        to_scene: transition.to_scene?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::canvas::{Canvas, Rgba};
    #[cfg(feature = "wgpu")]
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{SceneLayer, SceneLayerId};
    use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
        StripDirection,
    };
    use uuid::Uuid;

    use super::{CompositionPlanner, PlannedSceneLayer};
    use crate::render_thread::producer_queue::ProducerFrame;
    use crate::render_thread::scene_snapshot::{SceneRuntimeSnapshot, SceneTransitionSnapshot};
    use crate::render_thread::sparkleflinger::SparkleFlinger;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::sparkleflinger::{CompositionLayer, CompositionPlan};

    fn solid_canvas(color: Rgba) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(color);
        canvas
    }

    fn sample_group() -> Zone {
        let effect_id = EffectId::from(Uuid::now_v7());
        Zone {
            id: ZoneId::new(),
            name: "Desk".into(),
            description: None,
            layers: vec![SceneLayer::from_effect(
                SceneLayerId::new(),
                effect_id,
                HashMap::new(),
                HashMap::new(),
                None,
            )],
            layout: SpatialLayout {
                id: "desk".into(),
                name: "Desk".into(),
                description: None,
                canvas_width: 2,
                canvas_height: 2,
                zones: vec![Output {
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
                    brightness: None,
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
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        }
    }

    #[test]
    fn planner_marks_scene_transition_metadata() {
        let compiled = CompositionPlanner::compile(
            2,
            2,
            &SceneRuntimeSnapshot {
                active_scene_id: Some(hypercolor_types::scene::SceneId::new()),
                active_scene_name: None,
                active_transition: Some(SceneTransitionSnapshot {
                    epoch: 1,
                    from_scene: Some(hypercolor_types::scene::SceneId::new()),
                    to_scene: Some(hypercolor_types::scene::SceneId::new()),
                    progress: 0.25,
                    eased_progress: 0.5,
                    color_interpolation: hypercolor_types::scene::ColorInterpolation::Srgb,
                }),
                resolved_zones: vec![sample_group()].into(),
                resolved_zones_revision: 1,
                zone_layout_preview_generation: 0,
                active_render_group_count: 1,
                active_display_zone_target_fps: std::collections::HashMap::new(),
                active_display_zone_output_routes: std::collections::HashMap::new(),
                active_display_zone_descriptors: std::collections::HashMap::new(),
                unassigned_behavior: UnassignedBehavior::default(),
                device_registry_generation: 0,
            },
            vec![PlannedSceneLayer::replace(
                ProducerFrame::Canvas(solid_canvas(Rgba::new(12, 34, 56, 255))),
                true,
            )],
        );

        assert_eq!(compiled.metadata.logical_layer_count, 1);
        assert_eq!(compiled.metadata.render_group_count, 1);
        assert!(compiled.metadata.scene_active);
        assert!(compiled.metadata.transition_active);
    }

    #[test]
    fn planner_compiles_multi_layer_plan_for_sparkleflinger() {
        let compiled = CompositionPlanner::compile(
            2,
            2,
            &SceneRuntimeSnapshot::default(),
            vec![
                PlannedSceneLayer::replace(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(255, 0, 0, 255))),
                    true,
                ),
                PlannedSceneLayer::alpha(
                    ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 255, 255))),
                    0.5,
                    true,
                ),
            ],
        );
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(compiled.plan);

        assert_eq!(compiled.metadata.logical_layer_count, 2);
        assert_eq!(compiled.metadata.render_group_count, 0);
        assert!(!composed.bypassed);
        let canvas = composed
            .sampling_canvas
            .as_ref()
            .expect("planner test expects a materialized canvas");
        assert_eq!(canvas.width(), 2);
        assert_eq!(canvas.height(), 2);
    }

    #[test]
    fn planner_crossfades_from_last_stable_frame_during_scene_transition() {
        let mut planner = CompositionPlanner::new();
        let stable = ProducerFrame::Canvas(solid_canvas(Rgba::new(255, 0, 0, 255)));
        let entering = ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 255, 255)));
        let stable_runtime = SceneRuntimeSnapshot::default();
        let _ = planner.compile_primary_frame(2, 2, &stable_runtime, stable, true, None);

        let transition_runtime = SceneRuntimeSnapshot {
            active_scene_id: Some(hypercolor_types::scene::SceneId::new()),
            active_scene_name: None,
            active_transition: Some(SceneTransitionSnapshot {
                epoch: 1,
                from_scene: Some(hypercolor_types::scene::SceneId::new()),
                to_scene: Some(hypercolor_types::scene::SceneId::new()),
                progress: 0.5,
                eased_progress: 0.5,
                color_interpolation: hypercolor_types::scene::ColorInterpolation::Srgb,
            }),
            resolved_zones: Vec::new().into(),
            resolved_zones_revision: 0,
            zone_layout_preview_generation: 0,
            active_render_group_count: 0,
            active_display_zone_target_fps: std::collections::HashMap::new(),
            active_display_zone_output_routes: std::collections::HashMap::new(),
            active_display_zone_descriptors: std::collections::HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let compiled =
            planner.compile_primary_frame(2, 2, &transition_runtime, entering, true, None);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(compiled.plan);

        assert_eq!(compiled.metadata.logical_layer_count, 2);
        assert_eq!(compiled.metadata.render_group_count, 0);
        assert!(!composed.bypassed);
        let pixel = &composed
            .sampling_canvas
            .as_ref()
            .expect("planner transition test expects a materialized canvas")
            .as_rgba_bytes()[0..4];
        assert_ne!(pixel, [255, 0, 0, 255].as_slice());
        assert_ne!(pixel, [0, 0, 255, 255].as_slice());
    }

    #[test]
    fn interrupted_transition_starts_from_latest_composed_frame() {
        let mut planner = CompositionPlanner::new();
        let stable = ProducerFrame::Canvas(solid_canvas(Rgba::new(255, 0, 0, 255)));
        let entering = ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 0, 255, 255)));
        let stable_runtime = SceneRuntimeSnapshot::default();
        let _ = planner.compile_primary_frame(2, 2, &stable_runtime, stable, true, None);

        let from_scene = hypercolor_types::scene::SceneId::new();
        let to_scene = hypercolor_types::scene::SceneId::new();
        let transition_runtime = SceneRuntimeSnapshot {
            active_scene_id: Some(to_scene),
            active_transition: Some(SceneTransitionSnapshot {
                epoch: 1,
                from_scene: Some(from_scene),
                to_scene: Some(to_scene),
                progress: 0.5,
                eased_progress: 0.5,
                color_interpolation: hypercolor_types::scene::ColorInterpolation::Srgb,
            }),
            ..SceneRuntimeSnapshot::default()
        };
        let compiled =
            planner.compile_primary_frame(2, 2, &transition_runtime, entering, true, None);
        let mut sparkleflinger = SparkleFlinger::cpu();
        let composed = sparkleflinger.compose(compiled.plan);
        let blended = composed
            .sampling_canvas
            .as_ref()
            .expect("transition should produce a canvas")
            .clone();
        let blended_pixel = blended.as_rgba_bytes()[0..4].to_vec();
        planner.observe_composed_frame(&transition_runtime, Some(ProducerFrame::Canvas(blended)));

        let next_scene = hypercolor_types::scene::SceneId::new();
        let interrupted_runtime = SceneRuntimeSnapshot {
            active_scene_id: Some(next_scene),
            active_transition: Some(SceneTransitionSnapshot {
                epoch: 2,
                from_scene: Some(to_scene),
                to_scene: Some(next_scene),
                progress: 0.0,
                eased_progress: 0.0,
                color_interpolation: hypercolor_types::scene::ColorInterpolation::Srgb,
            }),
            ..SceneRuntimeSnapshot::default()
        };
        let handoff = planner
            .take_interruption_handoff(&interrupted_runtime)
            .expect("new transition identity should interrupt the active transition");
        let frozen = handoff.frame.map(|frame| (frame, handoff.opaque));
        let compiled = planner.compile_primary_frame(
            2,
            2,
            &interrupted_runtime,
            ProducerFrame::Canvas(solid_canvas(Rgba::new(0, 255, 0, 255))),
            true,
            frozen,
        );
        let composed = sparkleflinger.compose(compiled.plan);
        let interrupted_pixel = &composed
            .sampling_canvas
            .as_ref()
            .expect("interrupted transition should produce a canvas")
            .as_rgba_bytes()[0..4];

        assert_eq!(interrupted_pixel, blended_pixel.as_slice());
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn interruption_releases_gpu_slots_for_frozen_and_incoming_frames() {
        let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
            return;
        };
        let preparation = sparkleflinger
            .prepare_canvas_resize(2, 2)
            .expect("GPU interruption test canvas should prepare");
        if !preparation.gpu_output_admitted() {
            return;
        }
        sparkleflinger.apply_canvas_resize(preparation);
        let compose = |sparkleflinger: &mut SparkleFlinger, color| {
            sparkleflinger.compose_for_outputs(
                CompositionPlan::single(
                    2,
                    2,
                    CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(color))),
                ),
                false,
                None,
            );
        };

        compose(&mut sparkleflinger, Rgba::new(255, 0, 0, 255));
        let first = sparkleflinger
            .immutable_current_output_frame()
            .expect("first snapshot should succeed")
            .expect("first snapshot should exist");
        compose(&mut sparkleflinger, Rgba::new(0, 0, 255, 255));
        let retained_second = sparkleflinger
            .immutable_current_output_frame()
            .expect("second snapshot should succeed")
            .expect("second snapshot should exist");
        compose(&mut sparkleflinger, Rgba::new(0, 255, 0, 255));

        let from_scene = hypercolor_types::scene::SceneId::new();
        let to_scene = hypercolor_types::scene::SceneId::new();
        let mut planner = CompositionPlanner::new();
        planner.active_transition = Some(super::SceneTransitionKey {
            epoch: 1,
            from_scene,
            to_scene,
        });
        planner.last_stable_frame = Some(first.clone());
        planner.transition_base_frame = Some(first);
        planner.last_composed_opaque = true;
        let interrupted_runtime = SceneRuntimeSnapshot {
            active_scene_id: Some(hypercolor_types::scene::SceneId::new()),
            active_transition: Some(SceneTransitionSnapshot {
                epoch: 2,
                from_scene: Some(to_scene),
                to_scene: Some(hypercolor_types::scene::SceneId::new()),
                progress: 0.0,
                eased_progress: 0.0,
                color_interpolation: hypercolor_types::scene::ColorInterpolation::Srgb,
            }),
            ..SceneRuntimeSnapshot::default()
        };

        let handoff = planner
            .take_interruption_handoff(&interrupted_runtime)
            .expect("new transition should release the prior planner state");
        assert!(handoff.frame.is_none());
        assert!(handoff.opaque);
        assert!(planner.last_stable_frame.is_none());
        assert!(planner.transition_base_frame.is_none());
        drop(retained_second);
        let frozen = sparkleflinger
            .immutable_current_output_frame()
            .expect("released outgoing leases should make the blend snapshot available")
            .expect("the frozen blend snapshot should exist");
        compose(&mut sparkleflinger, Rgba::new(255, 255, 0, 255));
        let incoming = sparkleflinger
            .immutable_current_output_frame()
            .expect("the incoming scene should fit beside the frozen blend")
            .expect("the incoming scene snapshot should exist");
        assert_ne!(frozen.width(), 0);
        assert_ne!(incoming.width(), 0);
    }
}
