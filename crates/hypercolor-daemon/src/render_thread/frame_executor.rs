use std::time::{Duration, Instant};

use tracing::{info, trace, warn};

use hypercolor_core::types::event::FrameTiming;

use super::frame_admission::FrameAdmissionSample;
use super::frame_composer::{ComposeRequest, compose_frame};
use super::frame_io::{publish_frame_updates, sample_inputs};
use super::frame_pacing::{FrameExecution, NextWake, SkipDecision};
use super::frame_state::{
    build_frame_scene_snapshot, reconcile_audio_capture, reconcile_screen_capture,
};
use super::frame_throttle::{maybe_idle_throttle, maybe_sleep_throttle};
use super::pipeline_runtime::PipelineRuntime;
use super::sparkleflinger::ComposedFrameSet;
use super::{RenderThreadState, micros_between, micros_u32, u64_to_u32};
use crate::discovery::handle_async_write_failures;
use crate::performance::{FrameTimeline, LatestFrameMetrics};

#[expect(
    clippy::too_many_lines,
    reason = "frame execution intentionally keeps the full pipeline in one ordered async function"
)]
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

    let pending_resize = render
        .render_scene_state
        .apply_transactions(&state.scene_transactions);
    if let Some((width, height)) = pending_resize {
        info!(width, height, "Applying live canvas resize");
        state.canvas_dims.set(width, height);
        render.apply_canvas_resize(width, height);
        frame_loop.idle_black_pushed = false;
        frame_loop.sleep_black_pushed = false;
        let mut engine = state.effect_engine.lock().await;
        engine.set_canvas_size(width, height);
    }
    let scene_snapshot = build_frame_scene_snapshot(
        state,
        frame_scheduler,
        &render.render_scene_state,
        &mut frame_loop.last_render_group_demand,
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
    if output_power.sleeping {
        let sleep_render_surfaces =
            render.render_surface_snapshot(state.preview_canvas_receiver_count());
        if let Some(frame) = maybe_sleep_throttle(
            state,
            &scene_snapshot,
            frame_start,
            scene_snapshot_done_us,
            sleep_render_surfaces,
            &mut render.static_surface_cache,
            &mut render.recycled_frame,
            &mut frame_loop.sleep_black_pushed,
            &mut frame_loop.last_audio_level_update_ms,
        )
        .await
        {
            return frame;
        }
    } else {
        frame_loop.sleep_black_pushed = false;
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
            &mut frame_loop.cached_inputs
        }
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas => &mut frame_loop.cached_inputs,
    };
    let input_done_at = Instant::now();
    let input_us = micros_between(input_start, input_done_at);
    let input_done_us = micros_between(frame_start, input_done_at);

    let mut render_stage = compose_frame(ComposeRequest {
        state,
        render,
        scene_snapshot: &scene_snapshot,
        skip_decision,
        inputs,
        delta_secs,
    })
    .await;
    let render_us = render_stage.total_us;

    let mut gpu_zone_sampling = false;
    let layout = if let Some(sampled_layout) = render_stage.sampled_layout.take() {
        if let Some(sampled_zones) = render_stage.sampled_zones.take() {
            render.recycled_frame.zones = sampled_zones;
        }
        sampled_layout
    } else {
        let sample_start = Instant::now();
        gpu_zone_sampling = if matches!(
            render_stage.composed_frame.backend,
            crate::performance::CompositorBackendKind::Gpu
        ) {
            match render.sparkleflinger.sample_zone_plan_into(
                scene_snapshot.spatial_engine.sampling_plan().as_ref(),
                &mut render.recycled_frame.zones,
            ) {
                Ok(sampled) => sampled,
                Err(error) => {
                    warn!(%error, "GPU spatial sampling failed; falling back to CPU");
                    false
                }
            }
        } else {
            false
        };
        if !gpu_zone_sampling {
            scene_snapshot.spatial_engine.sample_into(
                render_stage
                    .composed_frame
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU spatial sampling requires a materialized canvas"),
                &mut render.recycled_frame.zones,
            );
        }
        let sample_done_at = Instant::now();
        render_stage.sampled_us = micros_between(sample_start, sample_done_at);
        scene_snapshot.spatial_engine.layout()
    };
    let retained_frame = render_stage
        .reuse_published_frame
        .then(|| state.event_bus.frame_sender().borrow());
    let zone_colors = retained_frame
        .as_ref()
        .map_or(render.recycled_frame.zones.as_slice(), |frame| {
            frame.zones.as_slice()
        });
    let sample_us = render_stage.sampled_us;
    let sample_done_at = Instant::now();
    let sample_done_us = micros_between(frame_start, sample_done_at);

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
    let output_done_at = Instant::now();
    let push_us = micros_between(push_start, output_done_at);
    let output_done_us = micros_between(frame_start, output_done_at);

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let postprocess_us = 0;
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    let cpu_readback_skipped = matches!(
        render_stage.composed_frame.backend,
        crate::performance::CompositorBackendKind::Gpu
    ) && render_stage.composed_frame.sampling_canvas.is_none();

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = micros_u32(frame_start.elapsed());
    let screen_canvas_receivers = state.event_bus.screen_canvas_receiver_count();
    let ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: _,
        backend: compositor_backend,
    } = render_stage.composed_frame;
    let screen_watch_surface = if screen_canvas_receivers == 0 {
        None
    } else if !scene_snapshot.effect_demand.effect_running
        && scene_snapshot.effect_demand.screen_capture_active
    {
        preview_surface
            .clone()
            .or_else(|| sampling_surface.clone())
            .or_else(|| {
                inputs
                    .screen_data
                    .as_ref()
                    .and_then(|data| data.canvas_downscale.clone())
            })
    } else {
        preview_surface.clone().or_else(|| {
            inputs
                .screen_data
                .as_ref()
                .and_then(|data| data.canvas_downscale.clone())
        })
    };
    let publish_stats = publish_frame_updates(
        state,
        &mut render.recycled_frame,
        &inputs.audio,
        sampling_canvas,
        &render_stage.group_canvases,
        sampling_surface,
        preview_surface,
        screen_watch_surface,
        frame_num_u32,
        scene_snapshot.elapsed_ms,
        &mut frame_loop.last_audio_level_update_ms,
        render_stage.reuse_published_frame,
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
    let publish_done_at = Instant::now();
    let publish_done_us = micros_between(frame_start, publish_done_at);
    full_frame_copy_count =
        full_frame_copy_count.saturating_add(publish_stats.full_frame_copy_count);
    full_frame_copy_bytes =
        full_frame_copy_bytes.saturating_add(publish_stats.full_frame_copy_bytes);
    let render_surfaces = render.render_surface_snapshot(state.preview_canvas_receiver_count());
    let total_us = publish_done_us;
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
            gpu_zone_sampling,
            cpu_readback_skipped,
            compositor_backend,
            logical_layer_count: render_stage.logical_layer_count,
            render_group_count: render_stage.render_group_count,
            scene_active: render_stage.scene_active,
            scene_transition_active: render_stage.scene_transition_active,
            render_surface_slot_count: render_surfaces.slot_count,
            render_surface_free_slots: render_surfaces.free_slots,
            render_surface_published_slots: render_surfaces.published_slots,
            render_surface_dequeued_slots: render_surfaces.dequeued_slots,
            canvas_receiver_count: render_surfaces.canvas_receivers,
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
        compositor_backend = compositor_backend.as_str(),
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
        let admission = runtime.frame_admission.record_frame(FrameAdmissionSample {
            total_us,
            producer_us: render_stage.producer_us,
            composition_us: render_stage.composition_us,
            full_frame_copy_count,
        });
        match rl.frame_complete_with_max_tier(Some(admission.ceiling_tier)) {
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
