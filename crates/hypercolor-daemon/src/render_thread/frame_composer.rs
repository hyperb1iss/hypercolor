use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tracing::{debug, warn};

use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::RenderGroupId;
use hypercolor_types::spatial::SpatialLayout;

use super::frame_pacing::SkipDecision;
use super::frame_scheduler::FrameSceneSnapshot;
use super::frame_sources::{render_effect_into, static_surface};
use super::pipeline_runtime::{FrameInputs, RenderCaches};
use super::producer_queue::{ProducerFrame, ProducerFrameState};
use super::render_groups::{GroupCanvasFrame, RenderGroupResult};
use super::sparkleflinger::{ComposedFrameSet, PreviewSurfaceRequest};
use super::{
    MAX_RENDER_SURFACE_SLOTS, RenderThreadState, desired_render_surface_slots, micros_between,
    micros_u32,
};
use crate::preview_runtime::PreviewDemandSummary;

#[allow(
    clippy::struct_excessive_bools,
    reason = "render stage stats intentionally preserve distinct reuse and scene state flags"
)]
pub(crate) struct RenderStageStats {
    pub(crate) composed_frame: ComposedFrameSet,
    pub(crate) web_viewport_preview: Option<Canvas>,
    pub(crate) group_canvases: Vec<(RenderGroupId, GroupCanvasFrame)>,
    pub(crate) active_group_canvas_ids: Vec<RenderGroupId>,
    pub(crate) sampled_layout: Option<Arc<SpatialLayout>>,
    pub(crate) sampled_zones: Option<Vec<ZoneColors>>,
    pub(crate) reuse_published_frame: bool,
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
    pub(crate) render: &'a mut RenderCaches,
    pub(crate) scene_snapshot: &'a FrameSceneSnapshot,
    pub(crate) publish_canvas_preview: bool,
    pub(crate) publish_screen_canvas_preview: bool,
    pub(crate) skip_decision: SkipDecision,
    pub(crate) inputs: &'a mut FrameInputs,
    pub(crate) delta_secs: f32,
}

struct ProducedFrame {
    frame: ProducerFrame,
    web_viewport_preview: Option<Canvas>,
    opaque_hint: bool,
    producer_us: u32,
    state: Option<ProducerFrameState>,
}

struct ComposeContext<'a> {
    state: &'a RenderThreadState,
    render: &'a mut RenderCaches,
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
        render: request.render,
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

impl ComposeContext<'_> {
    async fn compose(&mut self) -> RenderStageStats {
        let stage_start = Instant::now();
        if self.scene_snapshot.scene_runtime.has_active_render_groups() {
            return self.compose_render_group_frame_set(stage_start).await;
        }

        let ProducedFrame {
            frame: source_frame,
            web_viewport_preview,
            opaque_hint: source_frame_opaque,
            producer_us,
            state: producer_state,
        } = if !self.scene_snapshot.effect_demand.effect_running {
            self.render.effect_queue.clear();
            if let Some(screen_frame) = self.latch_screen_frame() {
                screen_frame
            } else {
                ProducedFrame {
                    frame: ProducerFrame::Surface(static_surface(
                        &mut self.render.static_surface_cache,
                        self.state.canvas_dims.width(),
                        self.state.canvas_dims.height(),
                        [0, 0, 0],
                    )),
                    web_viewport_preview: None,
                    opaque_hint: true,
                    producer_us: 0,
                    state: None,
                }
            }
        } else if self.skip_decision == SkipDecision::ReuseCanvas {
            if let Some(frame) = self
                .render
                .effect_queue
                .latch_for_generation(self.scene_snapshot.effect_generation)
            {
                ProducedFrame {
                    frame: frame.frame,
                    web_viewport_preview: None,
                    opaque_hint: true,
                    producer_us: 0,
                    state: Some(frame.state),
                }
            } else {
                render_effect_frame(
                    self.state,
                    self.render,
                    self.delta_secs,
                    self.scene_snapshot.effect_generation,
                    &self.inputs.audio,
                    &self.inputs.interaction,
                    self.inputs.screen_data.as_ref(),
                    self.inputs.sensors.as_ref(),
                )
                .await
            }
        } else {
            self.render.effect_queue.clear();
            render_effect_frame(
                self.state,
                self.render,
                self.delta_secs,
                self.scene_snapshot.effect_generation,
                &self.inputs.audio,
                &self.inputs.interaction,
                self.inputs.screen_data.as_ref(),
                self.inputs.sensors.as_ref(),
            )
            .await
        };
        let producer_done_at = Instant::now();
        let producer_done_us = micros_between(stage_start, producer_done_at);
        let composition_start = producer_done_at;
        let compiled_plan = self.render.composition_planner.compile_primary_frame(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
            &self.scene_snapshot.scene_runtime,
            source_frame,
            source_frame_opaque,
        );
        let producer_retained = producer_state.is_some_and(ProducerFrameState::is_retained);
        let composed = self.render.sparkleflinger.compose_for_outputs(
            compiled_plan.plan.with_cpu_replay_cacheable(
                producer_retained && !compiled_plan.metadata.transition_active,
            ),
            self.requires_cpu_sampling_canvas(),
            self.preview_surface_request(),
        );
        let composition_done_at = Instant::now();
        let composition_us = micros_between(composition_start, composition_done_at);
        let composition_done_us = micros_between(stage_start, composition_done_at);
        RenderStageStats {
            composition_bypassed: composed.bypassed,
            composed_frame: composed,
            web_viewport_preview,
            group_canvases: Vec::new(),
            active_group_canvas_ids: Vec::new(),
            sampled_layout: None,
            sampled_zones: None,
            reuse_published_frame: false,
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
            effect_retained: self.scene_snapshot.effect_demand.effect_running
                && producer_state.is_some_and(ProducerFrameState::is_retained),
            screen_retained: !self.scene_snapshot.effect_demand.effect_running
                && self.scene_snapshot.effect_demand.screen_capture_active
                && producer_state.is_some_and(ProducerFrameState::is_retained),
        }
    }

    async fn compose_render_group_frame_set(&mut self, stage_start: Instant) -> RenderStageStats {
        let (render_group_result, effect_retained) =
            if self.skip_decision == SkipDecision::ReuseCanvas {
                if let Some(retained) = self.render.render_group_runtime.reuse_scene(
                    self.scene_snapshot
                        .scene_runtime
                        .active_render_groups_revision,
                ) {
                    (Ok(retained), true)
                } else {
                    let producer_start = Instant::now();
                    let result = {
                        let registry = self.state.effect_registry.read().await;
                        self.render.render_group_runtime.render_scene(
                            self.scene_snapshot
                                .scene_runtime
                                .active_render_groups
                                .as_ref(),
                            self.scene_snapshot
                                .scene_runtime
                                .active_render_groups_revision,
                            &registry,
                            self.delta_secs,
                            &self.inputs.audio,
                            &self.inputs.interaction,
                            self.inputs.screen_data.as_ref(),
                            self.inputs.sensors.as_ref(),
                            &mut self.render.recycled_frame.zones,
                        )
                    };
                    let producer_done_at = Instant::now();
                    let producer_us = micros_between(producer_start, producer_done_at);
                    let producer_done_us = micros_between(stage_start, producer_done_at);
                    return self.finish_render_group_frame_set(
                        result,
                        producer_us,
                        producer_done_us,
                        false,
                        stage_start,
                    );
                }
            } else {
                let producer_start = Instant::now();
                let result = {
                    let registry = self.state.effect_registry.read().await;
                    self.render.render_group_runtime.render_scene(
                        self.scene_snapshot
                            .scene_runtime
                            .active_render_groups
                            .as_ref(),
                        self.scene_snapshot
                            .scene_runtime
                            .active_render_groups_revision,
                        &registry,
                        self.delta_secs,
                        &self.inputs.audio,
                        &self.inputs.interaction,
                        self.inputs.screen_data.as_ref(),
                        self.inputs.sensors.as_ref(),
                        &mut self.render.recycled_frame.zones,
                    )
                };
                let producer_done_at = Instant::now();
                let producer_us = micros_between(producer_start, producer_done_at);
                let producer_done_us = micros_between(stage_start, producer_done_at);
                return self.finish_render_group_frame_set(
                    result,
                    producer_us,
                    producer_done_us,
                    false,
                    stage_start,
                );
            };

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
                let composition_start = Instant::now();
                let compiled_plan = self.render.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    render_group_result.preview_frame,
                    true,
                );
                let composed = self.render.sparkleflinger.compose_for_outputs(
                    compiled_plan.plan.with_cpu_replay_cacheable(
                        effect_retained && !compiled_plan.metadata.transition_active,
                    ),
                    self.requires_cpu_sampling_canvas(),
                    self.preview_surface_request(),
                );
                let composition_bypassed = composed.bypassed;
                let composition_done_at = Instant::now();
                let composition_us = micros_between(composition_start, composition_done_at);
                let composition_done_us = micros_between(stage_start, composition_done_at);

                RenderStageStats {
                    composed_frame: composed,
                    web_viewport_preview: None,
                    group_canvases: render_group_result.group_canvases,
                    active_group_canvas_ids: render_group_result.active_group_canvas_ids,
                    sampled_layout: Some(render_group_result.layout),
                    sampled_zones: None,
                    reuse_published_frame: render_group_result.reuse_published_zones,
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
                warn!(%error, "failed to render active scene groups; publishing black frame");
                let source_frame = ProducerFrame::Surface(static_surface(
                    &mut self.render.static_surface_cache,
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    [0, 0, 0],
                ));
                let composition_start = Instant::now();
                let compiled_plan = self.render.composition_planner.compile_primary_frame(
                    self.state.canvas_dims.width(),
                    self.state.canvas_dims.height(),
                    &self.scene_snapshot.scene_runtime,
                    source_frame,
                    true,
                );
                let composed = self.render.sparkleflinger.compose_for_outputs(
                    compiled_plan.plan.with_cpu_replay_cacheable(false),
                    self.requires_cpu_sampling_canvas(),
                    self.preview_surface_request(),
                );
                let composition_bypassed = composed.bypassed;
                let composition_done_at = Instant::now();
                let composition_us = micros_between(composition_start, composition_done_at);
                let composition_done_us = micros_between(stage_start, composition_done_at);

                RenderStageStats {
                    composed_frame: composed,
                    web_viewport_preview: None,
                    group_canvases: Vec::new(),
                    active_group_canvas_ids: Vec::new(),
                    sampled_layout: None,
                    sampled_zones: None,
                    reuse_published_frame: false,
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
                .render
                .screen_queue
                .submit_latest(ProducerFrame::Surface(screen_surface.clone()));
        } else if let Some(screen_canvas) = self.inputs.screen_canvas_for_frame(
            self.state.canvas_dims.width(),
            self.state.canvas_dims.height(),
        ) {
            let _ = self
                .render
                .screen_queue
                .submit_latest(ProducerFrame::Canvas(screen_canvas));
        }

        self.render
            .screen_queue
            .latch_latest()
            .map(|frame| ProducedFrame {
                frame: frame.frame,
                web_viewport_preview: None,
                opaque_hint: false,
                producer_us: 0,
                state: Some(frame.state),
            })
    }

    fn requires_cpu_sampling_canvas(&self) -> bool {
        requires_cpu_sampling_canvas(
            self.render
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
            self.state.preview_canvas_receiver_count(),
            self.state.preview_runtime.canvas_receiver_count(),
            self.state.preview_runtime.canvas_demand(),
            self.state.event_bus.screen_canvas_receiver_count(),
            self.state.preview_runtime.screen_canvas_receiver_count(),
            self.state.preview_runtime.screen_canvas_demand(),
        )
    }
}

fn requires_cpu_sampling_canvas(can_gpu_sample: bool) -> bool {
    !can_gpu_sample
}

fn requires_published_surface(
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
) -> bool {
    publish_canvas_preview
        || (publish_screen_canvas_preview && !effect_running && screen_capture_active)
}

#[expect(
    clippy::too_many_arguments,
    reason = "preview request sizing depends on tracked demand and receiver topology"
)]
fn preview_surface_request(
    canvas_width: u32,
    canvas_height: u32,
    publish_canvas_preview: bool,
    publish_screen_canvas_preview: bool,
    effect_running: bool,
    screen_capture_active: bool,
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
    ) {
        return None;
    }

    if (publish_canvas_preview && canvas_receivers > tracked_canvas_receivers)
        || (wants_screen_passthrough
            && screen_canvas_receivers > tracked_screen_canvas_receivers)
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

async fn render_effect_frame(
    state: &RenderThreadState,
    render: &mut RenderCaches,
    delta_secs: f32,
    effect_generation: u64,
    audio: &AudioData,
    interaction: &InteractionData,
    screen_data: Option<&ScreenData>,
    sensors: &hypercolor_types::sensor::SystemSnapshot,
) -> ProducedFrame {
    let render_start = Instant::now();
    let lease = if let lease @ Some(_) = render.render_surface_pool.dequeue() {
        lease
    } else {
        if render.render_surface_pool.slot_count() < MAX_RENDER_SURFACE_SLOTS {
            let previous_slots = render.render_surface_pool.slot_count();
            let receiver_count = state.preview_canvas_receiver_count();
            let expanded_slots = desired_render_surface_slots(receiver_count)
                .max(previous_slots.saturating_add(1))
                .min(MAX_RENDER_SURFACE_SLOTS);
            render.render_surface_pool.ensure_slot_count(expanded_slots);
            debug!(
                previous_slots,
                expanded_slots,
                canvas_receivers = receiver_count,
                "expanded render surface pool under retention pressure"
            );
        }
        render.render_surface_pool.dequeue()
    };

    if let Some(mut lease) = lease {
        let web_viewport_preview;
        {
            let target = lease.canvas_mut();
            web_viewport_preview = render_effect_into(
                state,
                effect_generation,
                delta_secs,
                audio,
                interaction,
                screen_data,
                sensors,
                target,
            )
            .await;
        }
        let surface = lease.submit(0, 0);
        let frame = ProducerFrame::Surface(surface);
        let _ = render
            .effect_queue
            .submit_for_generation(frame.clone(), effect_generation);
        return ProducedFrame {
            frame,
            web_viewport_preview,
            opaque_hint: true,
            producer_us: micros_u32(render_start.elapsed()),
            state: Some(ProducerFrameState::Fresh),
        };
    }

    debug!(
        slot_count = render.render_surface_pool.slot_count(),
        canvas_receivers = state.preview_canvas_receiver_count(),
        "render surface pool exhausted, falling back to owned canvas publish path"
    );
    let mut rendered = render
        .effect_target_canvas
        .take()
        .filter(|canvas| {
            canvas.width() == state.canvas_dims.width()
                && canvas.height() == state.canvas_dims.height()
        })
        .unwrap_or_else(|| Canvas::new(state.canvas_dims.width(), state.canvas_dims.height()));
    let web_viewport_preview = render_effect_into(
        state,
        effect_generation,
        delta_secs,
        audio,
        interaction,
        screen_data,
        sensors,
        &mut rendered,
    )
    .await;
    let frame = ProducerFrame::Canvas(rendered);
    let recycled = render
        .effect_queue
        .submit_for_generation(frame.clone(), effect_generation);
    render.effect_target_canvas = recycled.and_then(|previous| match previous {
        ProducerFrame::Canvas(canvas)
            if canvas.width() == state.canvas_dims.width()
                && canvas.height() == state.canvas_dims.height()
                && !canvas.is_shared() =>
        {
            Some(canvas)
        }
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => None,
    });
    ProducedFrame {
        frame,
        web_viewport_preview,
        opaque_hint: true,
        producer_us: micros_u32(render_start.elapsed()),
        state: Some(ProducerFrameState::Fresh),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewSurfaceRequest, effective_render_group_layer_count, preview_surface_request,
        requires_cpu_sampling_canvas, requires_published_surface,
    };
    use crate::preview_runtime::PreviewDemandSummary;

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
    fn published_surface_depends_on_preview_and_screen_passthrough_receivers() {
        assert!(!requires_published_surface(false, false, false, false));
        assert!(requires_published_surface(true, false, true, false));
        assert!(requires_published_surface(false, true, false, true));
        assert!(!requires_published_surface(false, true, true, true));
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
}
