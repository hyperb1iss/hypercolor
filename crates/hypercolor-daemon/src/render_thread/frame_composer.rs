use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
#[cfg(feature = "wgpu")]
use tracing::debug;
use tracing::warn;

use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::event::{EffectDegradationState, HypercolorEvent};
use hypercolor_types::scene::ZoneId;

use self::preview_policy::{
    PreviewSurfaceDemandLane, PreviewSurfaceRequestContext, preview_surface_request,
    requires_cpu_sampling_canvas, scene_canvas_forces_full_surface,
};
use super::display_lane::{
    DisplayLaneContext, DisplayLaneMaterializer, DisplayLaneRoutes,
    display_zones_require_composed_scene,
};
use super::frame_policy::SkipDecision;
use super::frame_sampling::LedSamplingStrategy;
use super::pipeline_runtime::{ComposeRuntime, FrameInputs};
use super::producer_queue::{ProducerFrame, ProducerFrameState, ProducerQueue};
use super::render_zones::{DisplayZoneCanvasFrame, ZoneEffectError, ZoneResult};
use super::scene_dependency::SceneDependencyKey;
use super::scene_snapshot::FrameSceneSnapshot;
use super::sparkleflinger::{ComposedFrameSet, PreviewSurfaceRequest, SparkleFlinger};
use super::{RenderThreadState, micros_between, micros_u32};
use crate::performance::FullFrameCopyMetrics;

mod preview_policy;

#[allow(
    clippy::struct_excessive_bools,
    reason = "render stage stats intentionally preserve distinct reuse and scene state flags"
)]
pub(crate) struct RenderStageStats {
    pub(crate) composed_frame: ComposedFrameSet,
    pub(crate) preview_requested: bool,
    pub(crate) web_viewport_preview: Option<PublishedSurface>,
    pub(crate) producer_full_frame_copy: FullFrameCopyMetrics,
    pub(crate) display_zone_frames: Vec<(ZoneId, DisplayZoneCanvasFrame)>,
    pub(crate) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(crate) active_display_zone_ids: Vec<ZoneId>,
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
    pub(crate) render_zone_count: u32,
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
    pub(crate) frame_delta: Duration,
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
    frame_delta: Duration,
    interrupted_transition_base: Option<(ProducerFrame, bool)>,
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
        frame_delta: request.frame_delta,
        interrupted_transition_base: None,
    }
    .compose()
    .await
}

fn effective_render_zone_layer_count(plan_layers: u32, zone_layers: u32) -> u32 {
    if zone_layers == 0 {
        return plan_layers;
    }

    zone_layers.saturating_add(plan_layers.saturating_sub(1))
}

fn render_zone_requires_full_composition(
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

fn shared_composed_frame(
    composed: &ComposedFrameSet,
    width: u32,
    height: u32,
) -> Option<ProducerFrame> {
    composed
        .sampling_surface
        .as_ref()
        .map(|surface| ProducerFrame::Surface(surface.clone()))
        .or_else(|| {
            composed
                .sampling_canvas
                .as_ref()
                .map(|canvas| ProducerFrame::Canvas(canvas.clone()))
        })
        .or_else(|| {
            composed.preview_surface.as_ref().and_then(|surface| {
                (surface.width() == width && surface.height() == height)
                    .then(|| ProducerFrame::Surface(surface.clone()))
            })
        })
}

impl ComposeContext<'_> {
    async fn compose(&mut self) -> RenderStageStats {
        self.interrupted_transition_base = self.capture_interrupted_transition_base();
        let observed_invalidation_epoch = self.inputs.screen_invalidation_epoch;
        if synchronize_screen_invalidation_epoch(
            self.compose.screen_queue,
            &mut self.inputs.screen_compositor_epoch,
            observed_invalidation_epoch,
        ) {
            self.compose.sparkleflinger.release_native_screen_caches();
        }
        let result = self.compose_render_zone_frame_set(Instant::now()).await;
        let composed_frame = shared_composed_frame(
            &result.composed_frame,
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
        );
        self.compose
            .composition_planner
            .observe_composed_frame(&self.scene_snapshot.scene_runtime, composed_frame);
        result
    }

    fn capture_interrupted_transition_base(&mut self) -> Option<(ProducerFrame, bool)> {
        let handoff = self
            .compose
            .composition_planner
            .take_interruption_handoff(&self.scene_snapshot.scene_runtime)?;
        self.compose
            .render_zone_runtime
            .release_retained_scene_frame();
        if let Some(frame) = handoff.frame {
            return Some((frame, handoff.opaque));
        }

        #[cfg(feature = "wgpu")]
        match self.compose.sparkleflinger.immutable_current_output_frame() {
            Ok(frame) => frame.map(|frame| (frame, handoff.opaque)),
            Err(error) => {
                debug!(%error, "failed to freeze interrupted scene transition output");
                None
            }
        }

        #[cfg(not(feature = "wgpu"))]
        None
    }

    async fn compose_render_zone_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        if self.scene_snapshot.scene_runtime.active_render_zone_count() == 0 {
            return self.compose_idle_frame_set(stage_start);
        }

        let producer_start = Instant::now();
        let registry = {
            let registry = self.state.effect_registry.read().await;
            self.compose
                .render_zone_runtime
                .effect_registry_snapshot(&registry)
        };
        let live_dependency_key = self
            .scene_snapshot
            .scene_runtime
            .dependency_key(registry.generation());
        let (render_zone_result, effect_retained) = self.compose.reuse_or_render_scene(
            self.scene_snapshot,
            live_dependency_key,
            &registry,
            self.skip_decision,
            self.frame_delta,
            self.inputs,
        );
        if !effect_retained {
            let producer_done_at = Instant::now();
            let producer_us = micros_between(producer_start, producer_done_at);
            let producer_done_us = micros_between(stage_start, producer_done_at);
            return self.finish_render_zone_frame_set(
                render_zone_result,
                producer_us,
                producer_done_us,
                false,
                live_dependency_key,
                stage_start,
            );
        }

        let producer_us = 0;
        let producer_done_us = micros_u32(stage_start.elapsed());
        self.finish_render_zone_frame_set(
            render_zone_result,
            producer_us,
            producer_done_us,
            effect_retained,
            live_dependency_key,
            stage_start,
        )
    }

    fn compose_idle_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        self.compose.clear_inactive_zones();
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
            self.interrupted_transition_base.take(),
        );
        let producer_retained = producer_state.is_some_and(ProducerFrameState::is_retained);
        let preview_request = self.preview_surface_request();
        let preview_surface_pressure = self.preview_surface_pressure();
        let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
        let requires_cpu_sampling_canvas = self.requires_cpu_sampling_canvas();
        let composed = self.compose.sparkleflinger.compose_for_outputs(
            compiled_plan.plan.with_cpu_replay_cacheable(
                producer_retained && !compiled_plan.metadata.transition_active,
            ),
            requires_cpu_sampling_canvas,
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
            display_zone_frames: Vec::new(),
            zone_canvases: Vec::new(),
            active_display_zone_ids: Vec::new(),
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
            render_zone_count: compiled_plan.metadata.render_zone_count,
            scene_active: compiled_plan.metadata.scene_active,
            scene_transition_active: compiled_plan.metadata.transition_active,
            effect_retained: false,
            screen_retained: self.scene_snapshot.effect_demand.screen_capture_active
                && producer_retained,
            preview_surface_pressure,
            scene_canvas_forced_surface,
        }
    }

    fn finish_render_zone_frame_set(
        &mut self,
        render_zone_result: Result<ZoneResult>,
        producer_us: u32,
        producer_done_us: u32,
        effect_retained: bool,
        dependency_key: SceneDependencyKey,
        stage_start: Instant,
    ) -> RenderStageStats {
        match render_zone_result {
            Ok(render_zone_result) => {
                self.publish_effect_recovered();
                self.publish_layer_runtime_events();
                let scene_frame = render_zone_result.scene_frame.clone();
                let composition_start = Instant::now();
                let compiled_plan = self.compose.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    scene_frame.clone(),
                    true,
                    self.interrupted_transition_base.take(),
                );
                let preview_request = self.preview_surface_request();
                let preview_surface_pressure = self.preview_surface_pressure();
                let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
                let display_blend_requires_scene =
                    display_zones_require_composed_scene(&render_zone_result.display_zone_frames);
                let requires_full_composition = render_zone_requires_full_composition(
                    compiled_plan.metadata.transition_active,
                    &render_zone_result.led_sampling_strategy,
                ) || display_blend_requires_scene
                    || producer_frame_requires_composition_for_preview(
                        &scene_frame,
                        preview_request.is_some(),
                    );
                let requires_cpu_sampling_canvas = render_zone_result
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
                    self.scene_display_frame_for_zones(&scene_frame, requires_full_composition);
                let (_, display_routes) =
                    self.state.event_bus.display_zone_output_routes_snapshot();
                let display_lane_context = DisplayLaneContext {
                    elapsed_ms: self.scene_snapshot.elapsed_ms,
                    dependency_key,
                    target_fps: &self
                        .scene_snapshot
                        .scene_runtime
                        .active_display_zone_target_fps,
                    routes: DisplayLaneRoutes {
                        current: &display_routes,
                        fallback: &self
                            .scene_snapshot
                            .scene_runtime
                            .active_display_zone_output_routes,
                    },
                };
                let display_zone_frames =
                    DisplayLaneMaterializer::new(&mut self.compose, display_lane_context)
                        .materialize_zone_canvases(
                            &render_zone_result.active_display_zone_ids,
                            render_zone_result.display_zone_frames,
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
                    producer_full_frame_copy: render_zone_result.producer_full_frame_copy,
                    display_zone_frames,
                    zone_canvases: render_zone_result.zone_canvases,
                    active_display_zone_ids: render_zone_result.active_display_zone_ids,
                    led_sampling_strategy: render_zone_result.led_sampling_strategy,
                    producer_render_us: render_zone_result.render_us,
                    producer_scene_compose_us: render_zone_result.scene_compose_us,
                    sampled_us: render_zone_result.sample_us,
                    producer_us,
                    producer_done_us,
                    composition_us,
                    composition_done_us,
                    total_us: composition_done_us,
                    logical_layer_count: effective_render_zone_layer_count(
                        compiled_plan.metadata.logical_layer_count,
                        render_zone_result.logical_layer_count,
                    ),
                    render_zone_count: compiled_plan.metadata.render_zone_count,
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
                let published_effect_error = self.publish_effect_error(&error);
                if let Some(retained) = self.compose.render_zone_runtime.reuse_scene(dependency_key)
                {
                    warn!(%error, "failed to render active scene zones; retaining the last frame");
                    return self.finish_render_zone_frame_set(
                        Ok(retained),
                        producer_us,
                        producer_done_us,
                        true,
                        dependency_key,
                        stage_start,
                    );
                }
                self.compose.clear_inactive_zones();
                if published_effect_error || error.downcast_ref::<ZoneEffectError>().is_none() {
                    warn!(%error, "failed to render active scene zones without a retained frame; publishing black frame");
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
                    self.interrupted_transition_base.take(),
                );
                let preview_request = self.preview_surface_request();
                let preview_surface_pressure = self.preview_surface_pressure();
                let scene_canvas_forced_surface = self.scene_canvas_forced_surface();
                let requires_cpu_sampling_canvas = self.requires_cpu_sampling_canvas();
                let composed = self.compose.sparkleflinger.compose_for_outputs(
                    compiled_plan.plan.with_cpu_replay_cacheable(false),
                    requires_cpu_sampling_canvas,
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
                    display_zone_frames: Vec::new(),
                    zone_canvases: Vec::new(),
                    active_display_zone_ids: Vec::new(),
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
                    render_zone_count: compiled_plan.metadata.render_zone_count,
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
        let native_submitted = {
            #[cfg(feature = "wgpu")]
            {
                self.inputs
                    .screen_publication
                    .as_ref()
                    .is_some_and(|publication| {
                        let outcome = self
                            .compose
                            .sparkleflinger
                            .copy_screen_publication_outcome(publication);
                        apply_native_screen_copy_outcome(self.compose.screen_queue, outcome)
                    })
            }
            #[cfg(not(feature = "wgpu"))]
            {
                false
            }
        };
        if !native_submitted
            && let Some(publication) = self
                .inputs
                .screen_publication
                .as_ref()
                .and_then(|publication| ProducerFrame::screen_publication(Arc::clone(publication)))
        {
            let _ = self.compose.screen_queue.submit_latest(publication);
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

    fn requires_cpu_sampling_canvas(&mut self) -> bool {
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

    fn scene_display_frame_for_zones(
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
            .render_zone_runtime
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
            .render_zone_runtime
            .take_recovered_effect_error()
        else {
            return;
        };

        self.publish_effect_degraded(&effect_error, EffectDegradationState::Recovered, None);
    }

    fn publish_layer_runtime_events(&mut self) {
        for event in self
            .compose
            .render_zone_runtime
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
                zone_id: Some(effect_error.zone_id),
                zone_name: Some(effect_error.zone_name.clone()),
                state,
                reason: reason.map(ToString::to_string),
            });
    }
}

/// Fold one native screen copy outcome into the screen queue.
///
/// Returns whether a native frame is latched for this frame: a fresh copy
/// or a retained last-good frame after a transient or non-invalidating
/// failure. Invalidated and unavailable outcomes drop the stale frame so
/// the CPU path takes over.
#[cfg(feature = "wgpu")]
fn apply_native_screen_copy_outcome(
    screen_queue: &mut ProducerQueue,
    outcome: super::sparkleflinger::gpu::NativeScreenCopyOutcome,
) -> bool {
    use super::sparkleflinger::gpu::NativeScreenCopyOutcome;
    match outcome {
        NativeScreenCopyOutcome::Copied(frame) => {
            let _ = screen_queue.submit_latest(ProducerFrame::GpuTexture(frame));
            true
        }
        NativeScreenCopyOutcome::Ignored => false,
        NativeScreenCopyOutcome::Deferred(error) => {
            let retained = native_copy_failure_retains_last_frame(screen_queue);
            tracing::debug!(%error, retained, "Native screen copy deferred");
            retained
        }
        NativeScreenCopyOutcome::Failed(error) => {
            let retained = native_copy_failure_retains_last_frame(screen_queue);
            warn!(%error, retained, "Native screen copy failed");
            retained
        }
        NativeScreenCopyOutcome::Invalidated(error) => {
            let _ = screen_queue.clear_latest();
            warn!(%error, "Native screen copy invalidated");
            false
        }
        NativeScreenCopyOutcome::Unavailable(error) => {
            let _ = screen_queue.clear_latest();
            warn!(%error, "Native screen execution unavailable");
            false
        }
    }
}

#[cfg(all(test, feature = "wgpu"))]
mod native_screen_recovery_tests {
    use hypercolor_types::canvas::Canvas;

    use super::{ProducerFrame, ProducerQueue, apply_native_screen_copy_outcome};
    use crate::render_thread::sparkleflinger::gpu::NativeScreenCopyOutcome;

    fn queue_with_frame() -> ProducerQueue {
        let mut queue = ProducerQueue::new();
        let _ = queue.submit_latest(ProducerFrame::Canvas(Canvas::new(2, 2)));
        queue
    }

    #[test]
    fn transient_native_copy_failure_retains_last_good_frame() {
        let mut queue = queue_with_frame();

        assert!(apply_native_screen_copy_outcome(
            &mut queue,
            NativeScreenCopyOutcome::Deferred(anyhow::anyhow!("transient GPU fence pressure")),
        ));
        assert!(queue.has_latest());

        assert!(apply_native_screen_copy_outcome(
            &mut queue,
            NativeScreenCopyOutcome::Failed(anyhow::anyhow!("copy failed, target intact")),
        ));
        assert!(queue.has_latest());
    }

    #[test]
    fn structural_native_copy_failure_clears_stale_frame() {
        let mut invalidated = queue_with_frame();
        assert!(!apply_native_screen_copy_outcome(
            &mut invalidated,
            NativeScreenCopyOutcome::Invalidated(anyhow::anyhow!("structural import failure")),
        ));
        assert!(!invalidated.has_latest());

        let mut unavailable = queue_with_frame();
        assert!(!apply_native_screen_copy_outcome(
            &mut unavailable,
            NativeScreenCopyOutcome::Unavailable(anyhow::anyhow!("native reconstruction failed")),
        ));
        assert!(!unavailable.has_latest());
    }
}

pub(super) fn synchronize_screen_plan_generation(
    sparkleflinger: &mut SparkleFlinger,
    screen_queue: &mut ProducerQueue,
    generation: u64,
) -> bool {
    let changed = sparkleflinger.synchronize_screen_plan_generation(generation);
    if changed {
        let _ = screen_queue.clear_latest();
    }
    changed
}

fn synchronize_screen_invalidation_epoch(
    screen_queue: &mut ProducerQueue,
    current_epoch: &mut u64,
    observed_epoch: u64,
) -> bool {
    if observed_epoch <= *current_epoch {
        return false;
    }
    let _ = screen_queue.clear_latest();
    *current_epoch = observed_epoch;
    true
}

#[cfg(any(test, feature = "wgpu"))]
fn native_copy_failure_retains_last_frame(screen_queue: &ProducerQueue) -> bool {
    screen_queue.has_latest()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod h21_tests {
    use hypercolor_types::canvas::Canvas;

    use super::{ProducerFrame, ProducerQueue, synchronize_screen_invalidation_epoch};

    #[test]
    fn invalidation_epoch_clears_old_output_before_fresh_publication() {
        let mut queue = ProducerQueue::new();
        let mut epoch = 0;
        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(4, 4)));

        assert!(synchronize_screen_invalidation_epoch(
            &mut queue, &mut epoch, 1
        ));
        assert!(!queue.has_latest());

        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(4, 4)));
        assert!(!synchronize_screen_invalidation_epoch(
            &mut queue, &mut epoch, 1
        ));
        assert!(queue.has_latest());
    }
}
