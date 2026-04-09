use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{debug, trace, warn};

use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_core::types::event::FrameTiming;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;

use super::frame_io::{publish_frame_updates, sample_inputs};
use super::frame_scheduler::FrameSceneSnapshot;
use super::frame_state::{
    build_frame_scene_snapshot, reconcile_audio_capture, reconcile_screen_capture,
};
use super::pipeline_runtime::{FrameInputs, PipelineRuntime, RenderCaches};
use super::producer_queue::{ProducerFrame, ProducerFrameState};
use super::render_groups::RenderGroupResult;
use super::sparkleflinger::ComposedFrameSet;
use super::{
    MAX_RENDER_SURFACE_SLOTS, NextWake, RenderThreadState, SkipDecision,
    handle_async_write_failures, maybe_idle_throttle, maybe_sleep_throttle, micros_u32, u64_to_u32,
};
use crate::performance::{FrameTimeline, LatestFrameMetrics};

pub(crate) struct FrameExecution {
    pub(crate) next_wake: NextWake,
    pub(crate) next_skip_decision: SkipDecision,
}

struct RenderStageStats {
    composed_frame: ComposedFrameSet,
    sampled_layout: Option<Arc<SpatialLayout>>,
    sampled_zones: Option<Vec<ZoneColors>>,
    sampled_us: u32,
    producer_us: u32,
    producer_done_us: u32,
    composition_us: u32,
    composition_done_us: u32,
    total_us: u32,
    logical_layer_count: u32,
    render_group_count: u32,
    scene_active: bool,
    scene_transition_active: bool,
    effect_retained: bool,
    screen_retained: bool,
    composition_bypassed: bool,
}

struct ProducedFrame {
    frame: ProducerFrame,
    producer_us: u32,
    state: Option<ProducerFrameState>,
}

struct ComposeContext<'a> {
    state: &'a RenderThreadState,
    render: &'a mut RenderCaches,
    scene_snapshot: &'a FrameSceneSnapshot,
    skip_decision: SkipDecision,
    inputs: &'a FrameInputs,
    delta_secs: f32,
}

impl<'a> ComposeContext<'a> {
    async fn compose(&mut self) -> RenderStageStats {
        let stage_start = Instant::now();
        if self.scene_snapshot.scene_runtime.has_active_render_groups() {
            return self.compose_render_group_frame_set(stage_start).await;
        }

        let ProducedFrame {
            frame: source_frame,
            producer_us,
            state: producer_state,
        } = if !self.scene_snapshot.effect_demand.effect_running {
            self.render.effect_queue.clear();
            if let Some(screen_frame) = self.latch_screen_frame() {
                screen_frame
            } else {
                ProducedFrame {
                    frame: ProducerFrame::Surface(super::static_surface(
                        &mut self.render.static_surface_cache,
                        self.state.canvas_width,
                        self.state.canvas_height,
                        [0, 0, 0],
                    )),
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
                    producer_us: 0,
                    state: Some(frame.state),
                }
            } else {
                self.render_effect_frame(
                    self.scene_snapshot.effect_generation,
                    &self.inputs.audio,
                    &self.inputs.interaction,
                    self.inputs.screen_data.as_ref(),
                )
                .await
            }
        } else {
            self.render_effect_frame(
                self.scene_snapshot.effect_generation,
                &self.inputs.audio,
                &self.inputs.interaction,
                self.inputs.screen_data.as_ref(),
            )
            .await
        };
        let producer_done_us = micros_u32(stage_start.elapsed());
        let composition_start = Instant::now();
        let compiled_plan = self.render.composition_planner.compile_primary_frame(
            self.state.canvas_width,
            self.state.canvas_height,
            &self.scene_snapshot.scene_runtime,
            source_frame,
        );
        let composed = self.render.sparkleflinger.compose(compiled_plan.plan);
        let composition_us = micros_u32(composition_start.elapsed());
        let composition_done_us = micros_u32(stage_start.elapsed());
        RenderStageStats {
            composition_bypassed: composed.bypassed,
            composed_frame: composed,
            sampled_layout: None,
            sampled_zones: None,
            sampled_us: 0,
            producer_us,
            producer_done_us,
            composition_us,
            composition_done_us,
            total_us: micros_u32(stage_start.elapsed()),
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
                if let Some(retained) = self
                    .render
                    .render_group_runtime
                    .reuse_scene(&self.scene_snapshot.scene_runtime.active_render_groups)
                {
                    (Ok(retained), true)
                } else {
                    let producer_start = Instant::now();
                    let result = {
                        let registry = self.state.effect_registry.read().await;
                        self.render.render_group_runtime.render_scene(
                            &self.scene_snapshot.scene_runtime.active_render_groups,
                            &registry,
                            self.delta_secs,
                            &self.inputs.audio,
                            &self.inputs.interaction,
                            self.inputs.screen_data.as_ref(),
                        )
                    };
                    let producer_us = micros_u32(producer_start.elapsed());
                    let producer_done_us = micros_u32(stage_start.elapsed());
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
                        &self.scene_snapshot.scene_runtime.active_render_groups,
                        &registry,
                        self.delta_secs,
                        &self.inputs.audio,
                        &self.inputs.interaction,
                        self.inputs.screen_data.as_ref(),
                    )
                };
                let producer_us = micros_u32(producer_start.elapsed());
                let producer_done_us = micros_u32(stage_start.elapsed());
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
                    self.state.canvas_width,
                    self.state.canvas_height,
                    &self.scene_snapshot.scene_runtime,
                    render_group_result.preview_frame,
                );
                let composed = self.render.sparkleflinger.compose(compiled_plan.plan);
                let composition_bypassed = composed.bypassed;
                let composition_us = micros_u32(composition_start.elapsed());
                let composition_done_us = micros_u32(stage_start.elapsed());

                RenderStageStats {
                    composed_frame: composed,
                    sampled_layout: Some(render_group_result.layout),
                    sampled_zones: Some(render_group_result.zones),
                    sampled_us: render_group_result.sample_us,
                    producer_us,
                    producer_done_us,
                    composition_us,
                    composition_done_us,
                    total_us: micros_u32(stage_start.elapsed()),
                    logical_layer_count: compiled_plan
                        .metadata
                        .logical_layer_count
                        .max(render_group_result.logical_layer_count),
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
                let source_frame = ProducerFrame::Surface(super::static_surface(
                    &mut self.render.static_surface_cache,
                    self.state.canvas_width,
                    self.state.canvas_height,
                    [0, 0, 0],
                ));
                let composition_start = Instant::now();
                let compiled_plan = self.render.composition_planner.compile_primary_frame(
                    self.state.canvas_width,
                    self.state.canvas_height,
                    &self.scene_snapshot.scene_runtime,
                    source_frame,
                );
                let composed = self.render.sparkleflinger.compose(compiled_plan.plan);
                let composition_bypassed = composed.bypassed;
                let composition_us = micros_u32(composition_start.elapsed());
                let composition_done_us = micros_u32(stage_start.elapsed());

                RenderStageStats {
                    composed_frame: composed,
                    sampled_layout: None,
                    sampled_zones: None,
                    sampled_us: 0,
                    producer_us,
                    producer_done_us,
                    composition_us,
                    composition_done_us,
                    total_us: micros_u32(stage_start.elapsed()),
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
        if let Some(screen_surface) = self.inputs.screen_preview_surface.as_ref()
            && screen_surface.width() == self.state.canvas_width
            && screen_surface.height() == self.state.canvas_height
        {
            self.render
                .screen_queue
                .submit(ProducerFrame::Surface(screen_surface.clone()), 0);
        } else if let Some(screen_canvas) = self.inputs.screen_canvas.clone() {
            self.render
                .screen_queue
                .submit(ProducerFrame::Canvas(screen_canvas), 0);
        }

        self.render
            .screen_queue
            .latch_latest()
            .map(|frame| ProducedFrame {
                frame: frame.frame,
                producer_us: 0,
                state: Some(frame.state),
            })
    }

    async fn render_effect_frame(
        &mut self,
        effect_generation: u64,
        audio: &AudioData,
        interaction: &InteractionData,
        screen_data: Option<&ScreenData>,
    ) -> ProducedFrame {
        let render_start = Instant::now();
        let lease = match self.render.render_surface_pool.dequeue() {
            lease @ Some(_) => lease,
            None => {
                if self.render.render_surface_pool.slot_count() < MAX_RENDER_SURFACE_SLOTS {
                    let previous_slots = self.render.render_surface_pool.slot_count();
                    let expanded_slots = (previous_slots + 1).min(MAX_RENDER_SURFACE_SLOTS);
                    self.render
                        .render_surface_pool
                        .ensure_slot_count(expanded_slots);
                    debug!(
                        previous_slots,
                        expanded_slots,
                        canvas_receivers = self.state.event_bus.canvas_receiver_count(),
                        "expanded render surface pool under retention pressure"
                    );
                }
                self.render.render_surface_pool.dequeue()
            }
        };

        if let Some(mut lease) = lease {
            {
                let target = lease.canvas_mut();
                super::render_effect_into(
                    self.state,
                    effect_generation,
                    self.delta_secs,
                    audio,
                    interaction,
                    screen_data,
                    target,
                )
                .await;
            }
            let surface = lease.submit(0, 0);
            let frame = ProducerFrame::Surface(surface);
            self.render
                .effect_queue
                .submit(frame.clone(), effect_generation);
            return ProducedFrame {
                frame,
                producer_us: micros_u32(render_start.elapsed()),
                state: Some(ProducerFrameState::Fresh),
            };
        }

        debug!(
            slot_count = self.render.render_surface_pool.slot_count(),
            canvas_receivers = self.state.event_bus.canvas_receiver_count(),
            "render surface pool exhausted, falling back to owned canvas publish path"
        );
        let mut rendered = self
            .render
            .effect_target_canvas
            .take()
            .filter(|canvas| {
                canvas.width() == self.state.canvas_width
                    && canvas.height() == self.state.canvas_height
            })
            .unwrap_or_else(|| Canvas::new(self.state.canvas_width, self.state.canvas_height));
        super::render_effect_into(
            self.state,
            effect_generation,
            self.delta_secs,
            audio,
            interaction,
            screen_data,
            &mut rendered,
        )
        .await;
        self.render.effect_target_canvas = Some(rendered.clone());
        let frame = ProducerFrame::Canvas(rendered);
        self.render
            .effect_queue
            .submit(frame.clone(), effect_generation);
        ProducedFrame {
            frame,
            producer_us: micros_u32(render_start.elapsed()),
            state: Some(ProducerFrameState::Fresh),
        }
    }
}

pub(crate) async fn execute_frame(
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
    scheduled_start: Instant,
    skip_decision: SkipDecision,
) -> FrameExecution {
    let frame_scheduler = &mut runtime.frame_scheduler;
    let frame_loop = &mut runtime.frame_loop;
    let render = &mut runtime.render;
    let frame_start = Instant::now();
    let frame_interval = frame_start.saturating_duration_since(frame_loop.last_tick);
    let delta_secs = frame_interval.as_secs_f32();
    frame_loop.last_tick = frame_start;
    let frame_interval_us = micros_u32(frame_interval);
    let wake_late_us = micros_u32(frame_start.saturating_duration_since(scheduled_start));
    let reused_inputs = matches!(
        skip_decision,
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas
    );
    let reused_canvas = matches!(skip_decision, SkipDecision::ReuseCanvas);

    render
        .render_scene_state
        .apply_transactions(&state.scene_transactions);
    let scene_snapshot = build_frame_scene_snapshot(
        state,
        frame_scheduler,
        &render.render_scene_state,
        delta_secs,
    )
    .await;
    let output_power = scene_snapshot.output_power;
    let effect_demand = scene_snapshot.effect_demand;
    reconcile_audio_capture(
        state,
        !output_power.sleeping && effect_demand.audio_capture_active,
        &mut frame_loop.last_audio_capture_active,
    )
    .await;
    reconcile_screen_capture(
        state,
        !output_power.sleeping && effect_demand.screen_capture_active,
        &mut frame_loop.last_screen_capture_active,
    )
    .await;
    let scene_snapshot_done_us = micros_u32(frame_start.elapsed());
    if let Some(frame) = maybe_sleep_throttle(
        state,
        &scene_snapshot,
        frame_start,
        scene_snapshot_done_us,
        &mut render.static_surface_cache,
        &mut render.recycled_frame,
        &mut frame_loop.sleep_black_pushed,
        &mut frame_loop.last_audio_level_update_ms,
    )
    .await
    {
        return frame;
    }

    if let Some(frame) = maybe_idle_throttle(
        state,
        effect_demand.effect_running,
        effect_demand.screen_capture_active,
        &mut frame_loop.idle_black_pushed,
    )
    .await
    {
        return frame;
    }

    let input_start = Instant::now();
    let inputs = match skip_decision {
        SkipDecision::None => {
            frame_loop.cached_inputs = sample_inputs(state, delta_secs).await;
            &frame_loop.cached_inputs
        }
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas => &frame_loop.cached_inputs,
    };
    let input_us = micros_u32(input_start.elapsed());
    let input_done_us = micros_u32(frame_start.elapsed());

    let mut render_stage = ComposeContext {
        state,
        render,
        scene_snapshot: &scene_snapshot,
        skip_decision,
        inputs,
        delta_secs,
    }
    .compose()
    .await;
    let render_us = render_stage.total_us;

    let layout = if let Some(sampled_layout) = render_stage.sampled_layout.clone() {
        render.recycled_frame.zones = render_stage.sampled_zones.take().unwrap_or_default();
        sampled_layout
    } else {
        let sample_start = Instant::now();
        scene_snapshot.spatial_engine.sample_into(
            &render_stage.composed_frame.sampling_canvas,
            &mut render.recycled_frame.zones,
        );
        render_stage.sampled_us = micros_u32(sample_start.elapsed());
        scene_snapshot.spatial_engine.layout()
    };
    let zone_colors = &render.recycled_frame.zones;
    let sample_us = render_stage.sampled_us;
    let sample_done_us = micros_u32(frame_start.elapsed());

    let push_start = Instant::now();
    let (write_stats, async_failures) = {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager
            .write_frame_with_brightness(
                zone_colors,
                layout.as_ref(),
                output_power.effective_brightness(),
                None,
            )
            .await;
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let push_us = micros_u32(push_start.elapsed());
    let output_done_us = micros_u32(frame_start.elapsed());

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let postprocess_us = 0;
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = micros_u32(frame_start.elapsed());
    let screen_watch_surface = if !scene_snapshot.effect_demand.effect_running
        && scene_snapshot.effect_demand.screen_capture_active
    {
        render_stage
            .composed_frame
            .preview_surface
            .clone()
            .or_else(|| render_stage.composed_frame.sampling_surface.clone())
            .or_else(|| inputs.screen_preview_surface.clone())
    } else {
        render_stage
            .composed_frame
            .preview_surface
            .clone()
            .or_else(|| inputs.screen_preview_surface.clone())
    };
    let ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface: _,
        bypassed: _,
    } = render_stage.composed_frame;
    let publish_stats = publish_frame_updates(
        state,
        &mut render.recycled_frame,
        &inputs.audio,
        sampling_canvas,
        sampling_surface,
        screen_watch_surface,
        frame_num_u32,
        scene_snapshot.elapsed_ms,
        &mut frame_loop.last_audio_level_update_ms,
        FrameTiming {
            producer_us: render_stage.producer_us,
            composition_us: render_stage.composition_us,
            render_us,
            sample_us,
            push_us,
            total_us: timing_total_us,
            budget_us: scene_snapshot.budget_us,
        },
    );
    let publish_us = publish_stats.elapsed_us;
    let publish_done_us = micros_u32(frame_start.elapsed());
    full_frame_copy_count =
        full_frame_copy_count.saturating_add(publish_stats.full_frame_copy_count);
    full_frame_copy_bytes =
        full_frame_copy_bytes.saturating_add(publish_stats.full_frame_copy_bytes);
    let total_us = micros_u32(frame_start.elapsed());
    let known_stage_us = input_us
        .saturating_add(render_us)
        .saturating_add(sample_us)
        .saturating_add(push_us)
        .saturating_add(postprocess_us)
        .saturating_add(publish_us);
    let overhead_us = total_us.saturating_sub(known_stage_us);
    let jitter_us = frame_interval_us.abs_diff(scene_snapshot.budget_us);

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: scene_snapshot.elapsed_ms,
            input_us,
            producer_us: render_stage.producer_us,
            composition_us: render_stage.composition_us,
            render_us,
            sample_us,
            push_us,
            postprocess_us,
            publish_us,
            overhead_us,
            total_us,
            wake_late_us,
            jitter_us,
            reused_inputs,
            reused_canvas,
            retained_effect: render_stage.effect_retained,
            retained_screen: render_stage.screen_retained,
            composition_bypassed: render_stage.composition_bypassed,
            logical_layer_count: render_stage.logical_layer_count,
            render_group_count: render_stage.render_group_count,
            scene_active: render_stage.scene_active,
            scene_transition_active: render_stage.scene_transition_active,
            full_frame_copy_count,
            full_frame_copy_bytes,
            output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
            timeline: FrameTimeline {
                frame_token: scene_snapshot.frame_token,
                budget_us: scene_snapshot.budget_us,
                scene_snapshot_done_us,
                input_done_us,
                producer_done_us: input_done_us.saturating_add(render_stage.producer_done_us),
                composition_done_us: input_done_us.saturating_add(render_stage.composition_done_us),
                sample_done_us,
                output_done_us,
                publish_done_us,
                frame_done_us: total_us,
            },
        });
    }

    for err in &write_stats.errors {
        warn!(error = %err, "device write error");
    }

    trace!(
        frame = scene_snapshot.frame_token,
        frame_interval_us,
        wake_late_us,
        jitter_us,
        input_us,
        render_us,
        producer_us = render_stage.producer_us,
        composition_us = render_stage.composition_us,
        logical_layers = render_stage.logical_layer_count,
        render_groups = render_stage.render_group_count,
        scene_active = render_stage.scene_active,
        scene_transition_active = render_stage.scene_transition_active,
        sample_us,
        push_us,
        postprocess_us,
        publish_us,
        overhead_us,
        total_us,
        reused_inputs,
        reused_canvas,
        full_frame_copy_count,
        full_frame_copy_bytes,
        devices = write_stats.devices_written,
        leds = write_stats.total_leds,
        "frame complete"
    );

    let (next_wake, next_skip_decision) = {
        let mut rl = state.render_loop.write().await;
        match rl.frame_complete() {
            Some(frame_stats) => (
                NextWake::Interval(rl.target_interval()),
                SkipDecision::from_frame_stats(&frame_stats),
            ),
            None => (NextWake::Delay(Duration::ZERO), SkipDecision::None),
        }
    };

    if !effect_demand.effect_running {
        frame_loop.idle_black_pushed = true;
    }

    FrameExecution {
        next_wake,
        next_skip_decision,
    }
}
