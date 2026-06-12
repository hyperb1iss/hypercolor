use std::time::Instant;

use anyhow::Result;
#[cfg(feature = "wgpu")]
use tracing::debug;
use tracing::warn;

use hypercolor_core::types::canvas::PublishedSurface;
use hypercolor_types::event::{EffectDegradationState, HypercolorEvent};
use hypercolor_types::scene::ZoneId;

use super::display_lane::{
    DisplayLaneContext, DisplayLaneMaterializer, DisplayLaneRoutes,
    display_groups_require_composed_scene,
};
use super::frame_policy::SkipDecision;
use super::frame_sampling::LedSamplingStrategy;
use super::pipeline_runtime::{ComposeRuntime, FrameInputs};
use super::producer_queue::{ProducerFrame, ProducerFrameState};
use super::render_groups::{GroupCanvasFrame, ZoneEffectError, ZoneResult};
use super::scene_dependency::SceneDependencyKey;
use super::scene_snapshot::FrameSceneSnapshot;
use super::sparkleflinger::{ComposedFrameSet, PreviewSurfaceRequest};
use super::{RenderThreadState, micros_between, micros_u32};
use crate::performance::FullFrameCopyMetrics;
use crate::preview_runtime::PreviewDemandSummary;

#[allow(
    clippy::struct_excessive_bools,
    reason = "render stage stats intentionally preserve distinct reuse and scene state flags"
)]
pub(crate) struct RenderStageStats {
    pub(crate) composed_frame: ComposedFrameSet,
    pub(crate) preview_requested: bool,
    pub(crate) web_viewport_preview: Option<PublishedSurface>,
    pub(crate) producer_full_frame_copy: FullFrameCopyMetrics,
    pub(crate) group_canvases: Vec<(ZoneId, GroupCanvasFrame)>,
    pub(crate) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(crate) active_group_canvas_ids: Vec<ZoneId>,
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
    pub(crate) preview_surface_pressure: bool,
    pub(crate) scene_canvas_forced_surface: bool,
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

fn producer_frame_requires_composition_for_preview(
    frame: &ProducerFrame,
    preview_requested: bool,
) -> bool {
    preview_requested && frame.is_gpu_resident()
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
        let (render_group_result, effect_retained, live_dependency_key) = {
            let registry = self.state.effect_registry.read().await;
            let live_dependency_key = self
                .scene_snapshot
                .scene_runtime
                .dependency_key(registry.generation());
            let (render_group_result, effect_retained) = self.compose.reuse_or_render_scene(
                self.scene_snapshot,
                live_dependency_key,
                &registry,
                self.skip_decision,
                self.delta_secs,
                self.inputs,
            );
            (render_group_result, effect_retained, live_dependency_key)
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
                live_dependency_key,
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
            live_dependency_key,
            stage_start,
        )
    }

    fn compose_idle_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        self.compose.clear_inactive_groups();
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
        let preview_surface_pressure = self.preview_surface_pressure();
        let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
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
            producer_full_frame_copy: FullFrameCopyMetrics::default(),
            group_canvases: Vec::new(),
            zone_canvases: Vec::new(),
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
            preview_surface_pressure,
            scene_canvas_forced_surface,
        }
    }

    fn finish_render_group_frame_set(
        &mut self,
        render_group_result: Result<ZoneResult>,
        producer_us: u32,
        producer_done_us: u32,
        effect_retained: bool,
        dependency_key: SceneDependencyKey,
        stage_start: Instant,
    ) -> RenderStageStats {
        match render_group_result {
            Ok(render_group_result) => {
                self.publish_effect_recovered();
                self.publish_layer_runtime_events();
                let scene_frame = render_group_result.scene_frame.clone();
                let composition_start = Instant::now();
                let compiled_plan = self.compose.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    scene_frame.clone(),
                    true,
                );
                let preview_request = self.preview_surface_request();
                let preview_surface_pressure = self.preview_surface_pressure();
                let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
                let display_blend_requires_scene =
                    display_groups_require_composed_scene(&render_group_result.group_canvases);
                let requires_full_composition = render_group_requires_full_composition(
                    compiled_plan.metadata.transition_active,
                    &render_group_result.led_sampling_strategy,
                ) || display_blend_requires_scene
                    || producer_frame_requires_composition_for_preview(
                        &scene_frame,
                        preview_request.is_some(),
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
                        .preview_only_frame(scene_frame.clone(), preview_request)
                };
                let scene_display_frame =
                    self.scene_display_frame_for_groups(&scene_frame, requires_full_composition);
                let (_, display_routes) =
                    self.state.event_bus.display_group_output_routes_snapshot();
                let display_lane_context = DisplayLaneContext {
                    elapsed_ms: self.scene_snapshot.elapsed_ms,
                    dependency_key,
                    target_fps: &self
                        .scene_snapshot
                        .scene_runtime
                        .active_display_group_target_fps,
                    routes: DisplayLaneRoutes {
                        current: &display_routes,
                        fallback: &self
                            .scene_snapshot
                            .scene_runtime
                            .active_display_group_output_routes,
                    },
                };
                let group_canvases =
                    DisplayLaneMaterializer::new(&mut self.compose, display_lane_context)
                        .materialize_group_canvases(
                            &render_group_result.active_group_canvas_ids,
                            render_group_result.group_canvases,
                            &scene_display_frame,
                        );
                let composition_bypassed = composed.bypassed;
                let composition_done_at = Instant::now();
                let composition_us = micros_between(composition_start, composition_done_at);
                let composition_done_us = micros_between(stage_start, composition_done_at);

                RenderStageStats {
                    composed_frame: composed,
                    preview_requested: preview_request.is_some(),
                    web_viewport_preview: None,
                    producer_full_frame_copy: render_group_result.producer_full_frame_copy,
                    group_canvases,
                    zone_canvases: render_group_result.zone_canvases,
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
                    preview_surface_pressure,
                    scene_canvas_forced_surface,
                }
            }
            Err(error) => {
                self.publish_layer_runtime_events();
                self.compose.clear_inactive_groups();
                if self.publish_effect_error(&error)
                    || error.downcast_ref::<ZoneEffectError>().is_none()
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
                let preview_surface_pressure = self.preview_surface_pressure();
                let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
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
                    producer_full_frame_copy: FullFrameCopyMetrics::default(),
                    group_canvases: Vec::new(),
                    zone_canvases: Vec::new(),
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
                    preview_surface_pressure,
                    scene_canvas_forced_surface,
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
        preview_surface_request(PreviewSurfaceRequestContext {
            canvas_width: self.state.canvas_dims.width(),
            canvas_height: self.state.canvas_dims.height(),
            publish_canvas_preview: self.publish_canvas_preview,
            publish_screen_canvas_preview: self.publish_screen_canvas_preview,
            effect_running: self.scene_snapshot.effect_demand.effect_running,
            screen_capture_active: self.scene_snapshot.effect_demand.screen_capture_active,
            scene_canvas: PreviewSurfaceDemandLane {
                receivers: self.state.scene_canvas_receiver_count(),
                tracked_receivers: self.state.preview_runtime.scene_canvas_receiver_count(),
                demand: self.state.preview_runtime.scene_canvas_demand(),
            },
            canvas: PreviewSurfaceDemandLane {
                receivers: self.state.preview_canvas_receiver_count(),
                tracked_receivers: self.state.preview_runtime.tracked_canvas_receiver_count(),
                demand: self.state.preview_runtime.tracked_canvas_demand(),
            },
            screen_canvas: PreviewSurfaceDemandLane {
                receivers: self.state.event_bus.screen_canvas_receiver_count(),
                tracked_receivers: self.state.preview_runtime.screen_canvas_receiver_count(),
                demand: self.state.preview_runtime.screen_canvas_demand(),
            },
        })
    }

    fn preview_surface_pressure(&self) -> bool {
        self.publish_canvas_preview
            || (self.publish_screen_canvas_preview
                && !self.scene_snapshot.effect_demand.effect_running
                && self.scene_snapshot.effect_demand.screen_capture_active)
    }

    fn scene_canvas_forced_surface(&self) -> bool {
        scene_canvas_forces_full_surface(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
            self.state.scene_canvas_receiver_count(),
            self.state.preview_runtime.scene_canvas_receiver_count(),
            self.state.preview_runtime.scene_canvas_demand(),
        )
    }

    fn scene_display_frame_for_groups(
        &mut self,
        fallback: &ProducerFrame,
        requires_full_composition: bool,
    ) -> ProducerFrame {
        #[cfg(feature = "wgpu")]
        if requires_full_composition {
            match self.compose.sparkleflinger.current_output_frame() {
                Ok(Some(frame)) => return ProducerFrame::GpuTexture(frame),
                Ok(None) => {}
                Err(error) => {
                    debug!(%error, "failed to export GPU scene frame for display finalization");
                }
            }
        }

        #[cfg(not(feature = "wgpu"))]
        let _ = requires_full_composition;

        fallback.clone()
    }

    fn publish_effect_error(&mut self, error: &anyhow::Error) -> bool {
        let Some(effect_error) = error.downcast_ref::<ZoneEffectError>() else {
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

    fn publish_layer_runtime_events(&mut self) {
        for event in self
            .compose
            .render_group_runtime
            .drain_layer_runtime_events()
        {
            self.state.event_bus.publish(event);
        }
    }

    fn publish_effect_degraded(
        &self,
        effect_error: &ZoneEffectError,
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

#[derive(Clone, Copy, Default)]
struct PreviewSurfaceDemandLane {
    receivers: usize,
    tracked_receivers: usize,
    demand: PreviewDemandSummary,
}

#[derive(Clone, Copy, Default)]
struct PreviewSurfaceRequestContext {
    canvas_width: u32,
    canvas_height: u32,
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
    scene_canvas: PreviewSurfaceDemandLane,
    canvas: PreviewSurfaceDemandLane,
    screen_canvas: PreviewSurfaceDemandLane,
}

fn preview_surface_request(context: PreviewSurfaceRequestContext) -> Option<PreviewSurfaceRequest> {
    let wants_screen_passthrough = context.publish_screen_canvas_preview
        && !context.effect_running
        && context.screen_capture_active;
    if !requires_published_surface(
        context.publish_canvas_preview,
        context.publish_screen_canvas_preview,
        context.effect_running,
        context.screen_capture_active,
        context.scene_canvas.receivers,
    ) {
        return None;
    }

    if context.scene_canvas.receivers > context.scene_canvas.tracked_receivers
        || (context.publish_canvas_preview
            && context.canvas.receivers > context.canvas.tracked_receivers)
        || (wants_screen_passthrough
            && context.screen_canvas.receivers > context.screen_canvas.tracked_receivers)
    {
        return Some(PreviewSurfaceRequest {
            width: context.canvas_width,
            height: context.canvas_height,
        });
    }

    let mut max_width = 0;
    let mut max_height = 0;
    let mut any_full_resolution = false;
    if context.publish_canvas_preview {
        max_width = max_width.max(context.canvas.demand.max_width);
        max_height = max_height.max(context.canvas.demand.max_height);
        any_full_resolution |= context.canvas.demand.any_full_resolution;
    }
    if context.scene_canvas.receivers > 0 {
        max_width = max_width.max(context.scene_canvas.demand.max_width);
        max_height = max_height.max(context.scene_canvas.demand.max_height);
        any_full_resolution |= context.scene_canvas.demand.any_full_resolution;
    }
    if wants_screen_passthrough {
        max_width = max_width.max(context.screen_canvas.demand.max_width);
        max_height = max_height.max(context.screen_canvas.demand.max_height);
        any_full_resolution |= context.screen_canvas.demand.any_full_resolution;
    }

    if any_full_resolution
        || context.canvas_width == 0
        || context.canvas_height == 0
        || max_width == 0
        || max_height == 0
    {
        return Some(PreviewSurfaceRequest {
            width: context.canvas_width,
            height: context.canvas_height,
        });
    }

    Some(PreviewSurfaceRequest {
        width: max_width.clamp(1, context.canvas_width),
        height: max_height.clamp(1, context.canvas_height),
    })
}

fn scene_canvas_forces_full_surface(
    canvas_width: u32,
    canvas_height: u32,
    scene_canvas_receivers: usize,
    tracked_scene_canvas_receivers: usize,
    scene_canvas_demand: PreviewDemandSummary,
) -> bool {
    if scene_canvas_receivers == 0 {
        return false;
    }

    if scene_canvas_receivers > tracked_scene_canvas_receivers {
        return true;
    }

    scene_canvas_demand.any_full_resolution
        || scene_canvas_demand.max_width == 0
        || scene_canvas_demand.max_height == 0
        || (scene_canvas_demand.max_width >= canvas_width
            && scene_canvas_demand.max_height >= canvas_height)
}

#[cfg(test)]
mod tests;
