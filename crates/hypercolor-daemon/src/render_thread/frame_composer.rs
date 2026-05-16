use std::time::Instant;

use anyhow::Result;
use tracing::warn;

use hypercolor_core::types::canvas::PublishedSurface;
use hypercolor_types::event::{EffectDegradationState, HypercolorEvent};
use hypercolor_types::scene::RenderGroupId;

use super::frame_policy::SkipDecision;
use super::frame_sampling::LedSamplingStrategy;
use super::pipeline_runtime::{ComposeRuntime, FrameInputs};
use super::producer_queue::{ProducerFrame, ProducerFrameState};
use super::render_groups::{
    GroupCanvasFrame, PendingGroupCanvasFrame, RenderGroupEffectError, RenderGroupResult,
};
use super::scene_snapshot::FrameSceneSnapshot;
use super::sparkleflinger::{ComposedFrameSet, PreviewSurfaceRequest};
#[cfg(feature = "servo-gpu-import")]
use super::sparkleflinger::{CompositionLayer, CompositionPlan};
use super::{RenderThreadState, micros_between, micros_u32};
use crate::preview_runtime::PreviewDemandSummary;

#[allow(
    clippy::struct_excessive_bools,
    reason = "render stage stats intentionally preserve distinct reuse and scene state flags"
)]
pub(crate) struct RenderStageStats {
    pub(crate) composed_frame: ComposedFrameSet,
    pub(crate) preview_requested: bool,
    pub(crate) web_viewport_preview: Option<PublishedSurface>,
    pub(crate) group_canvases: Vec<(RenderGroupId, GroupCanvasFrame)>,
    pub(crate) active_group_canvas_ids: Vec<RenderGroupId>,
    pub(crate) led_sampling_strategy: LedSamplingStrategy,
    pub(crate) producer_render_us: u32,
    pub(crate) producer_scene_compose_us: u32,
    pub(crate) sampled_us: u32,
    pub(crate) producer_us: u32,
    pub(crate) producer_done_us: u32,
    pub(crate) composition_us: u32,
    pub(crate) composition_done_us: u32,
    pub(crate) total_us: u32,
    pub(crate) logical_layer_count: u32,
    pub(crate) render_group_count: u32,
    pub(crate) scene_active: bool,
    pub(crate) scene_transition_active: bool,
    pub(crate) effect_retained: bool,
    pub(crate) screen_retained: bool,
    pub(crate) composition_bypassed: bool,
}

pub(crate) struct ComposeRequest<'a> {
    pub(crate) state: &'a RenderThreadState,
    pub(crate) compose: ComposeRuntime<'a>,
    pub(crate) scene_snapshot: &'a FrameSceneSnapshot,
    pub(crate) publish_canvas_preview: bool,
    pub(crate) publish_screen_canvas_preview: bool,
    pub(crate) skip_decision: SkipDecision,
    pub(crate) inputs: &'a mut FrameInputs,
    pub(crate) delta_secs: f32,
}

struct ProducedFrame {
    frame: ProducerFrame,
    opaque_hint: bool,
    producer_us: u32,
    state: Option<ProducerFrameState>,
}

struct ComposeContext<'a> {
    state: &'a RenderThreadState,
    compose: ComposeRuntime<'a>,
    scene_snapshot: &'a FrameSceneSnapshot,
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    skip_decision: SkipDecision,
    inputs: &'a mut FrameInputs,
    delta_secs: f32,
}

pub(crate) async fn compose_frame(request: ComposeRequest<'_>) -> RenderStageStats {
    ComposeContext {
        state: request.state,
        compose: request.compose,
        scene_snapshot: request.scene_snapshot,
        publish_canvas_preview: request.publish_canvas_preview,
        publish_screen_canvas_preview: request.publish_screen_canvas_preview,
        skip_decision: request.skip_decision,
        inputs: request.inputs,
        delta_secs: request.delta_secs,
    }
    .compose()
    .await
}

fn effective_render_group_layer_count(plan_layers: u32, group_layers: u32) -> u32 {
    if group_layers == 0 {
        return plan_layers;
    }

    group_layers.saturating_add(plan_layers.saturating_sub(1))
}

fn render_group_requires_full_composition(
    transition_active: bool,
    led_sampling_strategy: &LedSamplingStrategy,
) -> bool {
    led_sampling_strategy.requires_full_composition(transition_active)
}

impl ComposeContext<'_> {
    async fn compose(&mut self) -> RenderStageStats {
        self.compose_render_group_frame_set(Instant::now()).await
    }

    async fn compose_render_group_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        if self
            .scene_snapshot
            .scene_runtime
            .active_render_group_count()
            == 0
        {
            return self.compose_idle_frame_set(stage_start);
        }

        let producer_start = Instant::now();
        let (render_group_result, effect_retained) = {
            let registry = self.state.effect_registry.read().await;
            let live_dependency_key = self
                .scene_snapshot
                .scene_runtime
                .dependency_key(registry.generation());
            self.compose.reuse_or_render_scene(
                self.scene_snapshot,
                live_dependency_key,
                &registry,
                self.skip_decision,
                self.delta_secs,
                self.inputs,
            )
        };
        if !effect_retained {
            let producer_done_at = Instant::now();
            let producer_us = micros_between(producer_start, producer_done_at);
            let producer_done_us = micros_between(stage_start, producer_done_at);
            return self.finish_render_group_frame_set(
                render_group_result,
                producer_us,
                producer_done_us,
                false,
                stage_start,
            );
        }

        let producer_us = 0;
        let producer_done_us = micros_u32(stage_start.elapsed());
        self.finish_render_group_frame_set(
            render_group_result,
            producer_us,
            producer_done_us,
            effect_retained,
            stage_start,
        )
    }

    fn compose_idle_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        self.compose.render_group_runtime.clear_inactive_groups();
        let ProducedFrame {
            frame: source_frame,
            opaque_hint: source_frame_opaque,
            producer_us,
            state: producer_state,
        } = if self.scene_snapshot.effect_demand.screen_capture_active {
            self.latch_screen_frame().unwrap_or_else(|| ProducedFrame {
                frame: ProducerFrame::Surface(self.compose.output_artifacts.static_surface(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    [0, 0, 0],
                )),
                opaque_hint: true,
                producer_us: 0,
                state: None,
            })
        } else {
            ProducedFrame {
                frame: ProducerFrame::Surface(self.compose.output_artifacts.static_surface(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    [0, 0, 0],
                )),
                opaque_hint: true,
                producer_us: 0,
                state: None,
            }
        };
        let producer_done_at = Instant::now();
        let producer_done_us = micros_between(stage_start, producer_done_at);
        let composition_start = producer_done_at;
        let compiled_plan = self.compose.composition_planner.compile_primary_frame(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
            &self.scene_snapshot.scene_runtime,
            source_frame,
            source_frame_opaque,
        );
        let producer_retained = producer_state.is_some_and(ProducerFrameState::is_retained);
        let preview_request = self.preview_surface_request();
        let composed = self.compose.sparkleflinger.compose_for_outputs(
            compiled_plan.plan.with_cpu_replay_cacheable(
                producer_retained && !compiled_plan.metadata.transition_active,
            ),
            self.requires_cpu_sampling_canvas(),
            preview_request,
        );
        let composition_done_at = Instant::now();
        let composition_us = micros_between(composition_start, composition_done_at);
        let composition_done_us = micros_between(stage_start, composition_done_at);

        RenderStageStats {
            composition_bypassed: composed.bypassed,
            composed_frame: composed,
            preview_requested: preview_request.is_some(),
            web_viewport_preview: None,
            group_canvases: Vec::new(),
            active_group_canvas_ids: Vec::new(),
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(
                self.scene_snapshot.spatial_engine.clone(),
            ),
            producer_render_us: 0,
            producer_scene_compose_us: 0,
            sampled_us: 0,
            producer_us,
            producer_done_us,
            composition_us,
            composition_done_us,
            total_us: composition_done_us,
            logical_layer_count: compiled_plan.metadata.logical_layer_count,
            render_group_count: compiled_plan.metadata.render_group_count,
            scene_active: compiled_plan.metadata.scene_active,
            scene_transition_active: compiled_plan.metadata.transition_active,
            effect_retained: false,
            screen_retained: self.scene_snapshot.effect_demand.screen_capture_active
                && producer_retained,
        }
    }

    fn finish_render_group_frame_set(
        &mut self,
        render_group_result: Result<RenderGroupResult>,
        producer_us: u32,
        producer_done_us: u32,
        effect_retained: bool,
        stage_start: Instant,
    ) -> RenderStageStats {
        match render_group_result {
            Ok(render_group_result) => {
                self.publish_effect_recovered();
                let scene_frame = render_group_result.scene_frame.clone();
                let composition_start = Instant::now();
                let group_canvases =
                    self.materialize_group_canvases(render_group_result.group_canvases);
                let compiled_plan = self.compose.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    scene_frame,
                    true,
                );
                let preview_request = self.preview_surface_request();
                let requires_full_composition = render_group_requires_full_composition(
                    compiled_plan.metadata.transition_active,
                    &render_group_result.led_sampling_strategy,
                );
                let requires_cpu_sampling_canvas = render_group_result
                    .led_sampling_strategy
                    .sparkleflinger_engine()
                    .is_some_and(|spatial_engine| {
                        requires_cpu_sampling_canvas(
                            self.compose
                                .sparkleflinger
                                .can_sample_zone_plan(spatial_engine.sampling_plan().as_ref()),
                        )
                    });
                let composed = if requires_full_composition {
                    self.compose.sparkleflinger.compose_for_outputs(
                        compiled_plan.plan.with_cpu_replay_cacheable(
                            effect_retained && !compiled_plan.metadata.transition_active,
                        ),
                        requires_cpu_sampling_canvas,
                        preview_request,
                    )
                } else {
                    self.compose
                        .sparkleflinger
                        .preview_only_frame(render_group_result.scene_frame, preview_request)
                };
                let composition_bypassed = composed.bypassed;
                let composition_done_at = Instant::now();
                let composition_us = micros_between(composition_start, composition_done_at);
                let composition_done_us = micros_between(stage_start, composition_done_at);

                RenderStageStats {
                    composed_frame: composed,
                    preview_requested: preview_request.is_some(),
                    web_viewport_preview: None,
                    group_canvases,
                    active_group_canvas_ids: render_group_result.active_group_canvas_ids,
                    led_sampling_strategy: render_group_result.led_sampling_strategy,
                    producer_render_us: render_group_result.render_us,
                    producer_scene_compose_us: render_group_result.scene_compose_us,
                    sampled_us: render_group_result.sample_us,
                    producer_us,
                    producer_done_us,
                    composition_us,
                    composition_done_us,
                    total_us: composition_done_us,
                    logical_layer_count: effective_render_group_layer_count(
                        compiled_plan.metadata.logical_layer_count,
                        render_group_result.logical_layer_count,
                    ),
                    render_group_count: compiled_plan.metadata.render_group_count,
                    scene_active: compiled_plan.metadata.scene_active,
                    scene_transition_active: compiled_plan.metadata.transition_active,
                    effect_retained,
                    screen_retained: false,
                    composition_bypassed,
                }
            }
            Err(error) => {
                if self.publish_effect_error(&error)
                    || error.downcast_ref::<RenderGroupEffectError>().is_none()
                {
                    warn!(%error, "failed to render active scene groups; publishing black frame");
                }
                let source_frame =
                    ProducerFrame::Surface(self.compose.output_artifacts.static_surface(
                        self.state.canvas_dims.width(),
                        self.state.canvas_dims.height(),
                        [0, 0, 0],
                    ));
                let composition_start = Instant::now();
                let compiled_plan = self.compose.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    source_frame,
                    true,
                );
                let preview_request = self.preview_surface_request();
                let composed = self.compose.sparkleflinger.compose_for_outputs(
                    compiled_plan.plan.with_cpu_replay_cacheable(false),
                    self.requires_cpu_sampling_canvas(),
                    preview_request,
                );
                let composition_bypassed = composed.bypassed;
                let composition_done_at = Instant::now();
                let composition_us = micros_between(composition_start, composition_done_at);
                let composition_done_us = micros_between(stage_start, composition_done_at);

                RenderStageStats {
                    composed_frame: composed,
                    preview_requested: preview_request.is_some(),
                    web_viewport_preview: None,
                    group_canvases: Vec::new(),
                    active_group_canvas_ids: Vec::new(),
                    led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(
                        self.scene_snapshot.spatial_engine.clone(),
                    ),
                    producer_render_us: 0,
                    producer_scene_compose_us: 0,
                    sampled_us: 0,
                    producer_us,
                    producer_done_us,
                    composition_us,
                    composition_done_us,
                    total_us: composition_done_us,
                    logical_layer_count: compiled_plan.metadata.logical_layer_count,
                    render_group_count: compiled_plan.metadata.render_group_count,
                    scene_active: compiled_plan.metadata.scene_active,
                    scene_transition_active: compiled_plan.metadata.transition_active,
                    effect_retained: false,
                    screen_retained: false,
                    composition_bypassed,
                }
            }
        }
    }

    fn latch_screen_frame(&mut self) -> Option<ProducedFrame> {
        if let Some(screen_surface) = self
            .inputs
            .screen_data
            .as_ref()
            .and_then(|data| data.canvas_downscale.as_ref())
            && screen_surface.width() == self.state.canvas_dims.width()
            && screen_surface.height() == self.state.canvas_dims.height()
        {
            let _ = self
                .compose
                .screen_queue
                .submit_latest(ProducerFrame::Surface(screen_surface.clone()));
        } else if let Some(screen_surface) = self.inputs.screen_surface_for_frame(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
        ) {
            let _ = self
                .compose
                .screen_queue
                .submit_latest(ProducerFrame::Surface(screen_surface));
        }

        self.compose
            .screen_queue
            .latch_latest()
            .map(|frame| ProducedFrame {
                frame: frame.frame,
                opaque_hint: false,
                producer_us: 0,
                state: Some(frame.state),
            })
    }

    fn requires_cpu_sampling_canvas(&self) -> bool {
        requires_cpu_sampling_canvas(
            self.compose
                .sparkleflinger
                .can_sample_zone_plan(self.scene_snapshot.spatial_engine.sampling_plan().as_ref()),
        )
    }

    fn preview_surface_request(&self) -> Option<PreviewSurfaceRequest> {
        preview_surface_request(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
            self.publish_canvas_preview,
            self.publish_screen_canvas_preview,
            self.scene_snapshot.effect_demand.effect_running,
            self.scene_snapshot.effect_demand.screen_capture_active,
            self.state.scene_canvas_receiver_count(),
            self.state.preview_canvas_receiver_count(),
            self.state.preview_runtime.tracked_canvas_receiver_count(),
            self.state.preview_runtime.tracked_canvas_demand(),
            self.state.event_bus.screen_canvas_receiver_count(),
            self.state.preview_runtime.screen_canvas_receiver_count(),
            self.state.preview_runtime.screen_canvas_demand(),
        )
    }

    fn materialize_group_canvases(
        &mut self,
        group_canvases: Vec<(RenderGroupId, PendingGroupCanvasFrame)>,
    ) -> Vec<(RenderGroupId, GroupCanvasFrame)> {
        group_canvases
            .into_iter()
            .filter_map(|(group_id, frame)| {
                self.materialize_group_canvas(frame)
                    .map(|frame| (group_id, frame))
            })
            .collect()
    }

    #[cfg_attr(
        not(feature = "servo-gpu-import"),
        expect(
            clippy::unnecessary_wraps,
            reason = "the return type stays feature-stable because GPU readback can skip a frame"
        )
    )]
    fn materialize_group_canvas(
        &mut self,
        group_canvas: PendingGroupCanvasFrame,
    ) -> Option<GroupCanvasFrame> {
        let PendingGroupCanvasFrame {
            frame,
            display_target,
        } = group_canvas;
        let surface = match frame {
            ProducerFrame::Canvas(canvas) => PublishedSurface::from_owned_canvas(canvas, 0, 0),
            ProducerFrame::Surface(surface) => surface,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(frame) => {
                let width = frame.width;
                let height = frame.height;
                let plan = CompositionPlan::single(
                    width,
                    height,
                    CompositionLayer::replace_opaque(ProducerFrame::Gpu(frame)),
                )
                .with_cpu_replay_cacheable(false);
                let composed = self
                    .compose
                    .display_sparkleflinger
                    .compose_for_outputs(plan, true, None);
                composed.sampling_surface.or_else(|| {
                    composed
                        .sampling_canvas
                        .map(|canvas| PublishedSurface::from_owned_canvas(canvas, 0, 0))
                })?
            }
        };

        Some(GroupCanvasFrame {
            surface,
            display_target,
        })
    }

    fn publish_effect_error(&mut self, error: &anyhow::Error) -> bool {
        let Some(effect_error) = error.downcast_ref::<RenderGroupEffectError>() else {
            return false;
        };
        let Some(effect_error) = self
            .compose
            .render_group_runtime
            .note_effect_error(effect_error)
        else {
            return false;
        };

        self.state.event_bus.publish(HypercolorEvent::EffectError {
            effect_id: effect_error.effect_id.clone(),
            error: effect_error.to_string(),
            fallback: None,
        });
        self.publish_effect_degraded(&effect_error, EffectDegradationState::Failed, Some(error));
        true
    }

    fn publish_effect_recovered(&mut self) {
        let Some(effect_error) = self
            .compose
            .render_group_runtime
            .take_recovered_effect_error()
        else {
            return;
        };

        self.publish_effect_degraded(&effect_error, EffectDegradationState::Recovered, None);
    }

    fn publish_effect_degraded(
        &self,
        effect_error: &RenderGroupEffectError,
        state: EffectDegradationState,
        reason: Option<&anyhow::Error>,
    ) {
        self.state
            .event_bus
            .publish(HypercolorEvent::EffectDegraded {
                effect_id: effect_error.effect_id.clone(),
                group_id: Some(effect_error.group_id),
                group_name: Some(effect_error.group_name.clone()),
                state,
                reason: reason.map(ToString::to_string),
            });
    }
}

fn requires_cpu_sampling_canvas(can_gpu_sample: bool) -> bool {
    !can_gpu_sample
}

#[allow(
    clippy::fn_params_excessive_bools,
    reason = "preview publication depends on a small fixed matrix of boolean runtime states"
)]
fn requires_published_surface(
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
    scene_canvas_receivers: usize,
) -> bool {
    scene_canvas_receivers > 0
        || publish_canvas_preview
        || (publish_screen_canvas_preview && !effect_running && screen_capture_active)
}

#[allow(
    clippy::too_many_arguments,
    reason = "preview request sizing depends on tracked demand and receiver topology"
)]
#[allow(
    clippy::fn_params_excessive_bools,
    reason = "preview request sizing combines a fixed set of orthogonal publication switches"
)]
fn preview_surface_request(
    canvas_width: u32,
    canvas_height: u32,
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
    scene_canvas_receivers: usize,
    canvas_receivers: usize,
    tracked_canvas_receivers: usize,
    canvas_demand: PreviewDemandSummary,
    screen_canvas_receivers: usize,
    tracked_screen_canvas_receivers: usize,
    screen_canvas_demand: PreviewDemandSummary,
) -> Option<PreviewSurfaceRequest> {
    let wants_screen_passthrough =
        publish_screen_canvas_preview && !effect_running && screen_capture_active;
    if !requires_published_surface(
        publish_canvas_preview,
        publish_screen_canvas_preview,
        effect_running,
        screen_capture_active,
        scene_canvas_receivers,
    ) {
        return None;
    }

    if scene_canvas_receivers > 0 {
        return Some(PreviewSurfaceRequest {
            width: canvas_width,
            height: canvas_height,
        });
    }

    if (publish_canvas_preview && canvas_receivers > tracked_canvas_receivers)
        || (wants_screen_passthrough && screen_canvas_receivers > tracked_screen_canvas_receivers)
    {
        return Some(PreviewSurfaceRequest {
            width: canvas_width,
            height: canvas_height,
        });
    }

    let mut max_width = 0;
    let mut max_height = 0;
    let mut any_full_resolution = false;
    if publish_canvas_preview {
        max_width = max_width.max(canvas_demand.max_width);
        max_height = max_height.max(canvas_demand.max_height);
        any_full_resolution |= canvas_demand.any_full_resolution;
    }
    if wants_screen_passthrough {
        max_width = max_width.max(screen_canvas_demand.max_width);
        max_height = max_height.max(screen_canvas_demand.max_height);
        any_full_resolution |= screen_canvas_demand.any_full_resolution;
    }

    if any_full_resolution || max_width == 0 || max_height == 0 {
        return Some(PreviewSurfaceRequest {
            width: canvas_width,
            height: canvas_height,
        });
    }

    Some(PreviewSurfaceRequest {
        width: max_width.clamp(1, canvas_width),
        height: max_height.clamp(1, canvas_height),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewSurfaceRequest, effective_render_group_layer_count, preview_surface_request,
        render_group_requires_full_composition, requires_cpu_sampling_canvas,
        requires_published_surface,
    };
    use std::sync::Arc;

    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use crate::preview_runtime::PreviewDemandSummary;
    use crate::render_thread::frame_sampling::LedSamplingStrategy;
    use crate::render_thread::sparkleflinger::SparkleFlinger;
    use hypercolor_types::config::RenderAccelerationMode;

    #[test]
    fn render_group_layer_count_adds_transition_base_once() {
        assert_eq!(effective_render_group_layer_count(1, 4), 4);
        assert_eq!(effective_render_group_layer_count(2, 4), 5);
    }

    #[test]
    fn cpu_sampling_canvas_only_depends_on_preview_receivers_and_gpu_sampling() {
        assert!(!requires_cpu_sampling_canvas(true));
        assert!(requires_cpu_sampling_canvas(false));
    }

    #[test]
    fn composer_requires_cpu_sampling_canvas_for_gaussian_gpu_sampling_plan() {
        let Ok(sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
            return;
        };
        let spatial_engine = SpatialEngine::new(SpatialLayout {
            id: "layout".into(),
            name: "Layout".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![DeviceZone {
                id: "strip".into(),
                name: "Strip".into(),
                device_id: "device".into(),
                zone_name: None,
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 4,
                    direction: StripDirection::LeftToRight,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(SamplingMode::GaussianArea {
                    sigma: 1.0,
                    radius: 2,
                }),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
                brightness: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        });

        assert!(requires_cpu_sampling_canvas(
            sparkleflinger.can_sample_zone_plan(spatial_engine.sampling_plan().as_ref())
        ));
    }

    #[test]
    fn render_group_full_composition_is_required_when_sparkleflinger_owns_led_sampling() {
        let strategy = LedSamplingStrategy::SparkleFlinger(SpatialEngine::new(SpatialLayout {
            id: "layout".into(),
            name: "Layout".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }));
        assert!(render_group_requires_full_composition(false, &strategy));
    }

    #[test]
    fn render_group_presampled_leds_can_bypass_full_composition_without_transition() {
        let strategy = LedSamplingStrategy::PreSampled(Arc::new(SpatialLayout {
            id: "layout".into(),
            name: "Layout".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }));
        assert!(!render_group_requires_full_composition(false, &strategy));
        assert!(render_group_requires_full_composition(true, &strategy));
    }

    #[test]
    fn published_surface_depends_on_preview_and_screen_passthrough_receivers() {
        assert!(!requires_published_surface(false, false, false, false, 0));
        assert!(requires_published_surface(true, false, true, false, 0));
        assert!(requires_published_surface(false, true, false, true, 0));
        assert!(!requires_published_surface(false, true, true, true, 0));
    }

    #[test]
    fn published_surface_depends_on_scene_canvas_receivers() {
        assert!(requires_published_surface(false, false, false, false, 1));
    }

    #[test]
    fn preview_surface_request_uses_scaled_tracked_demand() {
        assert_eq!(
            preview_surface_request(
                1280,
                720,
                true,
                false,
                true,
                false,
                0,
                1,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 640,
                    max_height: 360,
                    ..PreviewDemandSummary::default()
                },
                0,
                0,
                PreviewDemandSummary::default(),
            ),
            Some(PreviewSurfaceRequest {
                width: 640,
                height: 360,
            })
        );
    }

    #[test]
    fn preview_surface_request_falls_back_to_full_size_for_untracked_receivers() {
        assert_eq!(
            preview_surface_request(
                1280,
                720,
                true,
                false,
                true,
                false,
                0,
                2,
                1,
                PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 20,
                    max_width: 640,
                    max_height: 360,
                    ..PreviewDemandSummary::default()
                },
                0,
                0,
                PreviewDemandSummary::default(),
            ),
            Some(PreviewSurfaceRequest {
                width: 1280,
                height: 720,
            })
        );
    }

    #[test]
    fn preview_surface_request_uses_full_resolution_for_authoritative_global_lane() {
        assert_eq!(
            preview_surface_request(
                1280,
                720,
                false,
                false,
                true,
                false,
                1,
                0,
                0,
                PreviewDemandSummary::default(),
                0,
                0,
                PreviewDemandSummary::default(),
            ),
            Some(PreviewSurfaceRequest {
                width: 1280,
                height: 720,
            })
        );
    }
}
