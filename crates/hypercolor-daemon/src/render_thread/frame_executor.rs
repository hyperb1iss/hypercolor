use std::future::{Future, poll_fn};
use std::task::Poll;
use std::time::Instant;

use tracing::{info, trace, warn};

use hypercolor_core::types::event::FrameTiming;

use super::frame_composer::{ComposeRequest, compose_frame};
use super::frame_io::{preview_publication_due, publish_frame_updates, sample_inputs};
use super::frame_policy::{FrameAdmissionSample, FrameExecution, SkipDecision};
use super::frame_sampling::{
    LedSamplingOutcome, resolve_led_sampling, try_finish_deferred_zone_sampling,
    try_finish_retired_zone_sampling,
};
use super::frame_state::{
    build_frame_scene_snapshot, reconcile_audio_capture, reconcile_screen_capture,
    refresh_effect_scene_snapshot,
};
use super::frame_throttle::{maybe_idle_throttle, maybe_sleep_throttle};
use super::pipeline_runtime::PipelineRuntime;
use super::sparkleflinger::ComposedFrameSet;
use super::{RenderThreadState, micros_between, micros_u32, u64_to_u32};
use crate::discovery::handle_async_write_failures;
use crate::performance::{FrameTimeline, LatestFrameMetrics};

fn should_advance_gpu_preview(render_stage: &super::frame_composer::RenderStageStats) -> bool {
    render_stage.preview_requested
        && render_stage.composed_frame.preview_surface.is_none()
        && matches!(
            render_stage.composed_frame.backend,
            crate::performance::CompositorBackendKind::Gpu
        )
}

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
    let scene_snapshot_cache = &mut runtime.scene_snapshot_cache;
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
    }
    let mut scene_snapshot = build_frame_scene_snapshot(
        state,
        scene_snapshot_cache,
        &render.render_scene_state,
        delta_secs,
    )
    .await;
    refresh_effect_scene_snapshot(
        state,
        scene_snapshot_cache,
        &render.render_scene_state,
        &mut scene_snapshot,
    )
    .await;
    let output_power = scene_snapshot.output_power;
    reconcile_audio_capture(
        state,
        !output_power.sleeping && scene_snapshot.effect_demand.audio_capture_active,
        &mut frame_loop.last_audio_capture_active,
    )
    .await;
    reconcile_screen_capture(
        state,
        !output_power.sleeping && scene_snapshot.effect_demand.screen_capture_active,
        &mut frame_loop.last_screen_capture_active,
    )
    .await;
    let scene_snapshot_done_us = micros_u32(frame_start.elapsed());
    if output_power.sleeping {
        let sleep_render_surfaces =
            render.render_surface_snapshot(state.published_canvas_receiver_count());
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
            &mut frame_loop.last_canvas_preview_publish_ms,
            &mut frame_loop.last_screen_canvas_preview_publish_ms,
            &mut frame_loop.last_web_viewport_preview_publish_ms,
        )
        .await
        {
            return frame;
        }
    } else {
        frame_loop.sleep_black_pushed = false;
    }

    if refresh_effect_scene_snapshot(
        state,
        scene_snapshot_cache,
        &render.render_scene_state,
        &mut scene_snapshot,
    )
    .await
    {
        let refreshed_demand = scene_snapshot.effect_demand;
        reconcile_audio_capture(
            state,
            !output_power.sleeping && refreshed_demand.audio_capture_active,
            &mut frame_loop.last_audio_capture_active,
        )
        .await;
        reconcile_screen_capture(
            state,
            !output_power.sleeping && refreshed_demand.screen_capture_active,
            &mut frame_loop.last_screen_capture_active,
        )
        .await;
    }

    if let Some(frame) = maybe_idle_throttle(
        state,
        scene_snapshot.effect_demand.effect_running,
        scene_snapshot.effect_demand.screen_capture_active,
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
    try_finish_retired_zone_sampling(
        render,
        "Retired GPU spatial sampling finalize failed; dropping stale deferred sample result",
    );
    let mut stale_deferred_sampling = None;
    let mut completed_deferred_sampling = None;
    if let Some(mut deferred_sampling) = render.deferred_zone_sampling.take() {
        match render.sparkleflinger.try_finish_pending_zone_sampling(
            &mut deferred_sampling,
            &mut render.deferred_zone_sampling_scratch,
        ) {
            Ok(true) => {
                completed_deferred_sampling = Some(deferred_sampling);
            }
            Ok(false) => {
                stale_deferred_sampling = Some(deferred_sampling);
            }
            Err(error) => {
                warn!(%error, "Deferred GPU spatial sampling finalize failed; dropping deferred sample result");
            }
        }
    }
    let canvas_preview_due = preview_publication_due(
        scene_snapshot.elapsed_ms,
        frame_loop.last_canvas_preview_publish_ms,
        state.preview_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_demand().max_fps,
    );
    let screen_canvas_preview_due = preview_publication_due(
        scene_snapshot.elapsed_ms,
        frame_loop.last_screen_canvas_preview_publish_ms,
        state.event_bus.screen_canvas_receiver_count(),
        state.preview_runtime.screen_canvas_receiver_count(),
        state.preview_runtime.screen_canvas_demand().max_fps,
    );
    let manager_lock = state.backend_manager.lock();
    tokio::pin!(manager_lock);
    let primed_backend_manager_lock = poll_fn(|cx| match manager_lock.as_mut().poll(cx) {
        Poll::Ready(manager) => {
            drop(manager);
            Poll::Ready(false)
        }
        Poll::Pending => Poll::Ready(true),
    })
    .await;
    let _ = refresh_effect_scene_snapshot(
        state,
        scene_snapshot_cache,
        &render.render_scene_state,
        &mut scene_snapshot,
    )
    .await;

    let mut render_stage = compose_frame(ComposeRequest {
        state,
        render,
        scene_snapshot: &scene_snapshot,
        publish_canvas_preview: canvas_preview_due,
        publish_screen_canvas_preview: screen_canvas_preview_due,
        skip_decision,
        inputs,
        delta_secs,
    })
    .await;
    let render_us = render_stage.total_us;

    if should_advance_gpu_preview(&render_stage)
        && let Err(error) = render.sparkleflinger.submit_pending_preview_work()
    {
        warn!(%error, "GPU preview submit failed; continuing without an overlapped preview finalize");
    }
    if should_advance_gpu_preview(&render_stage)
        && render_stage.composed_frame.preview_surface.is_none()
    {
        match render.sparkleflinger.resolve_preview_surface() {
            Ok(Some(preview_surface)) => {
                render_stage.composed_frame.preview_surface = Some(preview_surface);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(%error, "GPU preview early finalize failed; continuing without an early preview surface");
            }
        }
    }
    let LedSamplingOutcome {
        layout,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        refresh_reused_frame_metadata,
        reuses_published_frame,
    } = resolve_led_sampling(
        state,
        render,
        &scene_snapshot,
        &mut render_stage,
        completed_deferred_sampling,
        stale_deferred_sampling,
    );

    let sample_us = render_stage.sampled_us;
    let sample_done_at = Instant::now();
    let sample_done_us = micros_between(frame_start, sample_done_at);
    let push_start = Instant::now();
    let global_brightness = output_power.effective_brightness();
    let global_brightness_bits = global_brightness.to_bits();
    let (write_stats, async_failures) = {
        let mut manager = if primed_backend_manager_lock {
            manager_lock.await
        } else {
            state.backend_manager.lock().await
        };
        let device_brightness_generation = manager.output_brightness_generation();
        let can_reuse_routed_outputs = reuses_published_frame
            && frame_loop.last_output_brightness_bits == Some(global_brightness_bits)
            && frame_loop.last_device_output_brightness_generation
                == Some(device_brightness_generation)
            && manager.can_reuse_routed_frame_outputs(layout.as_ref());
        let write_stats = if can_reuse_routed_outputs {
            manager.reuse_routed_frame_outputs(layout.as_ref())
        } else if reuses_published_frame {
            let published_frame = state.event_bus.frame_sender().borrow();
            manager
                .write_frame_with_brightness(
                    &published_frame.zones,
                    layout.as_ref(),
                    global_brightness,
                    None,
                )
                .await
        } else {
            manager
                .write_frame_with_brightness(
                    &render.recycled_frame.zones,
                    layout.as_ref(),
                    global_brightness,
                    None,
                )
                .await
        };
        frame_loop.last_output_brightness_bits = Some(global_brightness_bits);
        frame_loop.last_device_output_brightness_generation = Some(device_brightness_generation);
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let output_done_at = Instant::now();
    let push_us = micros_between(push_start, output_done_at);
    let output_done_us = micros_between(frame_start, output_done_at);

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }
    try_finish_deferred_zone_sampling(
        render,
        "Deferred GPU spatial sampling late finalize failed; dropping deferred sample result",
    );
    try_finish_retired_zone_sampling(
        render,
        "Retired GPU spatial sampling late finalize failed; dropping stale deferred sample result",
    );

    let postprocess_start = Instant::now();
    let mut full_frame_copy_count = 0_u32;
    let mut full_frame_copy_bytes = 0_u32;
    let cpu_readback_skipped = matches!(
        render_stage.composed_frame.backend,
        crate::performance::CompositorBackendKind::Gpu
    ) && render_stage.composed_frame.sampling_canvas.is_none();
    if should_advance_gpu_preview(&render_stage)
        && render_stage.composed_frame.preview_surface.is_none()
    {
        match render.sparkleflinger.resolve_preview_surface() {
            Ok(Some(preview_surface)) => {
                render_stage.composed_frame.preview_surface = Some(preview_surface);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(%error, "GPU preview finalize failed; continuing without a preview surface");
            }
        }
    }
    let postprocess_us = micros_between(postprocess_start, Instant::now());

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
    let screen_watch_surface = if !screen_canvas_preview_due || screen_canvas_receivers == 0 {
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
        &render_stage.active_group_canvas_ids,
        sampling_surface,
        preview_surface,
        screen_watch_surface,
        render_stage.web_viewport_preview,
        frame_num_u32,
        scene_snapshot.elapsed_ms,
        &mut frame_loop.last_audio_level_update_ms,
        &mut frame_loop.last_canvas_preview_publish_ms,
        &mut frame_loop.last_screen_canvas_preview_publish_ms,
        &mut frame_loop.last_web_viewport_preview_publish_ms,
        reuses_published_frame,
        refresh_reused_frame_metadata,
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
    let render_surfaces = render.render_surface_snapshot(state.published_canvas_receiver_count());
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
            producer_render_us: render_stage.producer_render_us,
            producer_scene_compose_us: render_stage.producer_scene_compose_us,
            composition_us: render_stage.composition_us,
            render_us,
            sample_us,
            push_us,
            postprocess_us,
            publish_us,
            publish_frame_data_us: publish_stats.frame_data_us,
            publish_group_canvas_us: publish_stats.group_canvas_us,
            publish_preview_us: publish_stats.preview_us,
            publish_events_us: publish_stats.events_us,
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
            gpu_sample_deferred,
            gpu_sample_retry_hit,
            gpu_sample_queue_saturated,
            gpu_sample_wait_blocked,
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
            scene_pool_saturation_reallocs: render_surfaces.scene_pool_saturation_reallocs,
            direct_pool_saturation_reallocs: render_surfaces.direct_pool_saturation_reallocs,
            scene_pool_grown_slots: render_surfaces.scene_pool_grown_slots,
            direct_pool_grown_slots: render_surfaces.direct_pool_grown_slots,
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
        producer_render_us = render_stage.producer_render_us,
        producer_scene_compose_us = render_stage.producer_scene_compose_us,
        composition_us = render_stage.composition_us,
        compositor_backend = compositor_backend.as_str(),
        logical_layers = render_stage.logical_layer_count,
        render_groups = render_stage.render_group_count,
        scene_active = render_stage.scene_active,
        scene_transition_active = render_stage.scene_transition_active,
        gpu_sample_wait_blocked,
        gpu_sample_queue_saturated,
        sample_us,
        push_us,
        postprocess_us,
        publish_us,
        publish_frame_data_us = publish_stats.frame_data_us,
        publish_group_canvas_us = publish_stats.group_canvas_us,
        publish_preview_us = publish_stats.preview_us,
        publish_events_us = publish_stats.events_us,
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
        let execution = runtime.frame_policy.complete_render_frame(
            &mut rl,
            FrameAdmissionSample {
                total_us,
                producer_us: render_stage.producer_us,
                composition_us: render_stage.composition_us,
                push_us,
                publish_us,
                wake_late_us,
                jitter_us,
                full_frame_copy_count,
                output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
            },
        );
        (execution.next_wake, execution.next_skip_decision)
    };

    if !scene_snapshot.effect_demand.effect_running {
        frame_loop.idle_black_pushed = true;
    }

    FrameExecution {
        next_wake,
        next_skip_decision,
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
    use hypercolor_core::types::event::{FrameData, ZoneColors};
    use hypercolor_types::scene::{ColorInterpolation, SceneId};
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    };

    use super::should_advance_gpu_preview;
    use crate::performance::CompositorBackendKind;
    use crate::render_thread::frame_composer::RenderStageStats;
    use crate::render_thread::frame_sampling::LedSamplingStrategy;
    use crate::render_thread::frame_sampling::{
        blend_scene_zone_frames, build_transition_layout,
        can_hold_published_frame_for_deferred_sampling,
        can_reuse_published_frame_for_deferred_sampling,
    };
    use crate::render_thread::pipeline_runtime::SceneTransitionKey;
    use crate::render_thread::sparkleflinger::ComposedFrameSet;

    fn render_stage(
        backend: CompositorBackendKind,
        preview_requested: bool,
        preview_surface_present: bool,
    ) -> RenderStageStats {
        let mut composed_frame = ComposedFrameSet {
            sampling_canvas: None,
            sampling_surface: None,
            preview_surface: None,
            bypassed: false,
            backend,
        };
        if preview_surface_present {
            composed_frame.preview_surface =
                Some(PublishedSurface::from_owned_canvas(Canvas::new(1, 1), 0, 0));
        }
        RenderStageStats {
            composed_frame,
            preview_requested,
            web_viewport_preview: None,
            group_canvases: Vec::new(),
            active_group_canvas_ids: Vec::new(),
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(SpatialEngine::new(
                sample_layout(&[]),
            )),
            producer_render_us: 0,
            producer_scene_compose_us: 0,
            sampled_us: 0,
            producer_us: 0,
            producer_done_us: 0,
            composition_us: 0,
            composition_done_us: 0,
            total_us: 0,
            logical_layer_count: 0,
            render_group_count: 0,
            scene_active: false,
            scene_transition_active: false,
            effect_retained: false,
            screen_retained: false,
            composition_bypassed: false,
        }
    }

    #[test]
    fn gpu_preview_advances_only_when_requested() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, false, false);
        assert!(!should_advance_gpu_preview(&render_stage));
    }

    #[test]
    fn gpu_preview_does_not_advance_when_surface_is_ready() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, true);
        assert!(!should_advance_gpu_preview(&render_stage));
    }

    #[test]
    fn gpu_preview_advances_when_requested_and_unresolved() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, false);
        assert!(should_advance_gpu_preview(&render_stage));
    }

    fn sample_layout(zone_ids: &[&str]) -> SpatialLayout {
        SpatialLayout {
            id: "layout".to_owned(),
            name: "layout".to_owned(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: zone_ids
                .iter()
                .map(|zone_id| DeviceZone {
                    id: (*zone_id).to_owned(),
                    name: (*zone_id).to_owned(),
                    device_id: "device".to_owned(),
                    zone_name: None,
                    position: NormalizedPosition::new(0.5, 0.5),
                    size: NormalizedPosition::new(1.0, 1.0),
                    rotation: 0.0,
                    scale: 1.0,
                    display_order: 0,
                    orientation: None,
                    topology: LedTopology::Point,
                    led_positions: vec![NormalizedPosition::new(0.5, 0.5)],
                    led_mapping: None,
                    sampling_mode: Some(SamplingMode::Nearest),
                    edge_behavior: Some(EdgeBehavior::Clamp),
                    shape: None,
                    shape_preset: None,
                    attachment: None,
                    brightness: None,
                })
                .collect(),
            default_sampling_mode: SamplingMode::Nearest,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn published_frame(zone_ids: &[&str]) -> FrameData {
        FrameData::new(
            zone_ids
                .iter()
                .map(|zone_id| ZoneColors {
                    zone_id: (*zone_id).to_owned(),
                    colors: vec![[0, 0, 0]],
                })
                .collect(),
            1,
            1,
        )
    }

    fn zone(zone_id: &str, colors: &[[u8; 3]]) -> ZoneColors {
        ZoneColors {
            zone_id: zone_id.to_owned(),
            colors: colors.to_vec(),
        }
    }

    #[test]
    fn gpu_zone_sampling_reuses_retained_frame_only_when_layout_matches_without_backlog() {
        let mut render_stage = render_stage(CompositorBackendKind::Gpu, false, false);
        render_stage.screen_retained = true;
        let layout = sample_layout(&["left", "right"]);
        let retained_frame = published_frame(&["left", "right"]);

        assert!(can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &retained_frame
        ));
        assert!(!can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &published_frame(&["left", "other"])
        ));
        render_stage.screen_retained = false;
        assert!(!can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &retained_frame
        ));
    }

    #[test]
    fn gpu_zone_sampling_can_hold_previous_frame_when_layout_matches_without_backlog() {
        let layout = sample_layout(&["left", "right"]);
        let retained_frame = published_frame(&["left", "right"]);

        assert!(can_hold_published_frame_for_deferred_sampling(
            &layout,
            &retained_frame
        ));
        assert!(!can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "other"])
        ));
    }

    #[test]
    fn transition_layout_keeps_current_order_and_appends_base_only_zones() {
        let base_layout = sample_layout(&["left", "legacy"]);
        let current_layout = sample_layout(&["left", "right"]);
        let transition_layout = build_transition_layout(
            &base_layout,
            &current_layout,
            SceneTransitionKey {
                from_scene: SceneId::new(),
                to_scene: SceneId::new(),
            },
        );

        let zone_ids = transition_layout
            .zones
            .iter()
            .map(|zone| zone.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(zone_ids, vec!["left", "right", "legacy"]);
    }

    #[test]
    fn zone_transition_blend_unions_shared_and_missing_zone_outputs() {
        let transition_layout = sample_layout(&["shared", "entering", "leaving"]);
        let from = vec![
            zone("shared", &[[255, 0, 0]]),
            zone("leaving", &[[0, 255, 0]]),
        ];
        let to = vec![
            zone("shared", &[[0, 0, 255]]),
            zone("entering", &[[255, 255, 255]]),
        ];
        let mut blended = Vec::new();

        blend_scene_zone_frames(
            &from,
            &to,
            &transition_layout,
            0.5,
            &ColorInterpolation::Srgb,
            &mut blended,
        );

        assert_eq!(blended.len(), 3);
        assert_eq!(blended[0].zone_id, "shared");
        assert_eq!(blended[1].zone_id, "entering");
        assert_eq!(blended[2].zone_id, "leaving");
        assert_ne!(blended[0].colors[0], [255, 0, 0]);
        assert_ne!(blended[0].colors[0], [0, 0, 255]);
        assert!(blended[1].colors[0][0] > 0);
        assert!(blended[1].colors[0][1] > 0);
        assert!(blended[1].colors[0][2] > 0);
        assert!(blended[2].colors[0][1] > 0);
        assert_eq!(blended[2].colors[0][0], 0);
        assert_eq!(blended[2].colors[0][2], 0);
    }
}
