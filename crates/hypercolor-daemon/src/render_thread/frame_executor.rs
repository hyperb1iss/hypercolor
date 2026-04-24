use std::future::{Future, poll_fn};
use std::task::Poll;
use std::time::Instant;

use tracing::info;

use hypercolor_core::types::event::FrameTiming;

use super::frame_composer::{ComposeRequest, compose_frame};
use super::frame_io::{FramePublicationRequest, FramePublicationSurfaces, publish_frame_updates};
use super::frame_metrics::{ActiveFrameMetricsInput, summarize_active_frame};
use super::frame_policy::{FrameExecution, SkipDecision};
use super::frame_reporting::{FrameCompletionReport, report_active_frame_completion};
use super::frame_sampling::{LedSamplingOutcome, resolve_led_sampling};
use super::frame_throttle::{maybe_idle_throttle, maybe_sleep_throttle};
use super::pipeline_runtime::{
    OutputFrameSource, OutputReuseKey, PendingSamplingWork, PipelineRuntime,
};
use super::scene_snapshot::{build_frame_scene_snapshot, refresh_effect_scene_snapshot};
use super::sparkleflinger::ComposedFrameSet;
use super::{RenderThreadState, micros_between, micros_u32, u64_to_u32};
use crate::discovery::handle_async_write_failures;
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
    let scene = &mut runtime.scene;
    let frame_loop = &mut runtime.frame_loop;
    let render = &mut runtime.render;
    let frame_start = Instant::now();
    let frame_tick = frame_loop.clock.advance(frame_start);
    let delta_secs = frame_tick.delta_secs;
    let frame_interval_us = frame_tick.frame_interval_us;
    let wake_late_us = micros_u32(frame_start.saturating_duration_since(scheduled_start));
    let reused_inputs = matches!(
        skip_decision,
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas
    );
    let reused_canvas = matches!(skip_decision, SkipDecision::ReuseCanvas);

    let pending_resize = scene
        .render_state
        .apply_transactions(&state.scene_transactions);
    if let Some((width, height)) = pending_resize {
        info!(width, height, "Applying live canvas resize");
        state.canvas_dims.set(width, height);
        render.apply_canvas_resize(width, height);
        frame_loop.throttle.reset_for_canvas_resize();
    }
    let mut scene_snapshot = build_frame_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        delta_secs,
    )
    .await;
    refresh_effect_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await;
    let output_power = scene_snapshot.output_power;
    frame_loop
        .capture_demand
        .reconcile_effect_demand(state, output_power.sleeping, scene_snapshot.effect_demand)
        .await;
    let scene_snapshot_done_us = micros_u32(frame_start.elapsed());
    if output_power.sleeping {
        let sleep_render_surfaces =
            render.render_surface_snapshot(state.published_canvas_receiver_count());
        if let Some(frame) = maybe_sleep_throttle(
            state,
            &mut runtime.frame_policy,
            &scene_snapshot,
            frame_start,
            scene_snapshot_done_us,
            sleep_render_surfaces,
            &mut render.output_artifacts,
            &mut frame_loop.throttle,
            &mut frame_loop.publication_cadence,
        )
        .await
        {
            return frame;
        }
    } else {
        frame_loop.throttle.clear_sleep();
    }

    if refresh_effect_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await
    {
        let refreshed_demand = scene_snapshot.effect_demand;
        frame_loop
            .capture_demand
            .reconcile_effect_demand(state, output_power.sleeping, refreshed_demand)
            .await;
    }

    if let Some(frame) = maybe_idle_throttle(
        state,
        &mut runtime.frame_policy,
        scene_snapshot.effect_demand.effect_running,
        scene_snapshot.effect_demand.screen_capture_active,
        &mut frame_loop.throttle,
    )
    .await
    {
        return frame;
    }

    let input_start = Instant::now();
    let inputs = frame_loop
        .inputs
        .inputs_for_frame(state, skip_decision, delta_secs)
        .await;
    let input_done_at = Instant::now();
    let input_us = micros_between(input_start, input_done_at);
    let input_done_us = micros_between(frame_start, input_done_at);
    let PendingSamplingWork {
        completed: completed_deferred_sampling,
        stale: stale_deferred_sampling,
    } = {
        let mut sampling = render.sampling_runtime();
        sampling.prepare_pending_work(
            "Retired GPU spatial sampling finalize failed; dropping stale deferred sample result",
            "Deferred GPU spatial sampling finalize failed; dropping deferred sample result",
        )
    };
    let canvas_preview_due = frame_loop.publication_cadence.canvas_preview_due(
        scene_snapshot.elapsed_ms,
        state.preview_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_demand().max_fps,
    );
    let screen_canvas_preview_due = frame_loop.publication_cadence.screen_canvas_preview_due(
        scene_snapshot.elapsed_ms,
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
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await;

    let mut render_stage = compose_frame(ComposeRequest {
        state,
        compose: render.compose_runtime(),
        scene_snapshot: &scene_snapshot,
        publish_canvas_preview: canvas_preview_due,
        publish_screen_canvas_preview: screen_canvas_preview_due,
        skip_decision,
        inputs,
        delta_secs,
    })
    .await;
    let render_us = render_stage.total_us;

    {
        let mut preview = render.preview_runtime();
        preview.advance_gpu_preview(&mut render_stage);
    }
    let LedSamplingOutcome {
        layout,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        cpu_sampling_late_readback,
        refresh_reused_frame_metadata,
        reuses_published_frame,
    } = {
        let mut sampling = render.sampling_runtime();
        resolve_led_sampling(
            state,
            &mut sampling,
            &scene_snapshot,
            &mut render_stage,
            completed_deferred_sampling,
            stale_deferred_sampling,
        )
    };

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
        let output_reuse_key =
            OutputReuseKey::new(global_brightness_bits, device_brightness_generation);
        let output_reuse_decision = frame_loop.output_reuse.decide_frame_source(
            reuses_published_frame,
            output_reuse_key,
            || manager.can_reuse_routed_frame_outputs(layout.as_ref()),
        );
        let write_stats = match output_reuse_decision.source() {
            OutputFrameSource::RoutedReuse => manager.reuse_routed_frame_outputs(layout.as_ref()),
            OutputFrameSource::PublishedFrame => {
                let published_frame = state.event_bus.frame_sender().borrow();
                manager
                    .write_frame_with_brightness(
                        &published_frame.zones,
                        layout.as_ref(),
                        global_brightness,
                        None,
                    )
                    .await
            }
            OutputFrameSource::CurrentFrame => {
                manager
                    .write_frame_with_brightness(
                        render.output_artifacts.zones(),
                        layout.as_ref(),
                        global_brightness,
                        None,
                    )
                    .await
            }
        };
        frame_loop
            .output_reuse
            .record_decision(output_reuse_decision);
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let output_done_at = Instant::now();
    let push_us = micros_between(push_start, output_done_at);
    let output_done_us = micros_between(frame_start, output_done_at);

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }
    {
        let mut sampling = render.sampling_runtime();
        sampling.finish_sampling_cleanup(
            "Deferred GPU spatial sampling late finalize failed; dropping deferred sample result",
            "Retired GPU spatial sampling late finalize failed; dropping stale deferred sample result",
        );
    }

    let postprocess_start = Instant::now();
    let cpu_readback_skipped = matches!(
        render_stage.composed_frame.backend,
        crate::performance::CompositorBackendKind::Gpu
    ) && render_stage.composed_frame.sampling_canvas.is_none();
    {
        let mut preview = render.preview_runtime();
        preview.finalize_gpu_preview(&mut render_stage);
    }
    let postprocess_us = micros_between(postprocess_start, Instant::now());

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = micros_u32(frame_start.elapsed());
    let ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: _,
        backend: compositor_backend,
    } = render_stage.composed_frame;
    let publish_stats = publish_frame_updates(
        state,
        &mut frame_loop.publication_cadence,
        FramePublicationRequest {
            recycled_frame: render.output_artifacts.frame_mut(),
            audio: &inputs.audio,
            surfaces: FramePublicationSurfaces {
                canvas: sampling_canvas,
                frame_surface: sampling_surface,
                preview_surface,
                screen_capture_surface: inputs
                    .screen_data
                    .as_ref()
                    .and_then(|data| data.canvas_downscale.clone()),
                web_viewport_preview_surface: render_stage.web_viewport_preview,
                effect_running: scene_snapshot.effect_demand.effect_running,
                screen_capture_active: scene_snapshot.effect_demand.screen_capture_active,
            },
            group_canvases: &render_stage.group_canvases,
            active_group_canvas_ids: &render_stage.active_group_canvas_ids,
            frame_number: frame_num_u32,
            elapsed_ms: scene_snapshot.elapsed_ms,
            reuse_existing_frame: reuses_published_frame,
            refresh_existing_frame_metadata: refresh_reused_frame_metadata,
            timing: FrameTiming {
                producer_us: render_stage.producer_us,
                composition_us: render_stage.composition_us,
                render_us,
                sample_us,
                push_us,
                total_us: timing_total_us,
                budget_us: scene_snapshot.budget_us,
            },
        },
    );
    let publish_us = publish_stats.elapsed_us;
    let publish_done_at = Instant::now();
    let publish_done_us = micros_between(frame_start, publish_done_at);
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
    let output_errors = u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX);
    let frame_summary = summarize_active_frame(ActiveFrameMetricsInput {
        scene_snapshot: &scene_snapshot,
        render_surfaces: &render_surfaces,
        publish_stats: &publish_stats,
        input_us,
        producer_us: render_stage.producer_us,
        producer_render_us: render_stage.producer_render_us,
        producer_scene_compose_us: render_stage.producer_scene_compose_us,
        composition_us: render_stage.composition_us,
        producer_done_us: render_stage.producer_done_us,
        composition_done_us: render_stage.composition_done_us,
        render_us,
        sample_us,
        push_us,
        postprocess_us,
        total_us,
        wake_late_us,
        jitter_us,
        overhead_us,
        reused_inputs,
        reused_canvas,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        cpu_sampling_late_readback,
        cpu_readback_skipped,
        compositor_backend,
        output_errors,
        logical_layer_count: render_stage.logical_layer_count,
        render_group_count: render_stage.render_group_count,
        scene_active: render_stage.scene_active,
        scene_transition_active: render_stage.scene_transition_active,
        effect_retained: render_stage.effect_retained,
        screen_retained: render_stage.screen_retained,
        composition_bypassed: render_stage.composition_bypassed,
        scene_snapshot_done_us,
        input_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    });
    let frame_metrics = frame_summary.metrics;

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(frame_metrics);
    }

    report_active_frame_completion(
        FrameCompletionReport::new(frame_interval_us, frame_metrics, &write_stats),
        &write_stats.errors,
    );

    let (next_wake, next_skip_decision) = {
        let mut rl = state.render_loop.write().await;
        let execution = runtime
            .frame_policy
            .complete_render_frame(&mut rl, frame_summary.admission);
        (execution.next_wake, execution.next_skip_decision)
    };

    if !scene_snapshot.effect_demand.effect_running {
        frame_loop.throttle.note_idle_frame_without_effect();
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

    use crate::performance::CompositorBackendKind;
    use crate::render_thread::frame_composer::RenderStageStats;
    use crate::render_thread::frame_sampling::LedSamplingStrategy;
    use crate::render_thread::frame_sampling::{
        blend_scene_zone_frames, build_transition_layout,
        can_hold_published_frame_for_deferred_sampling, can_hold_zone_colors_for_deferred_sampling,
        can_reuse_published_frame_for_deferred_sampling,
    };
    use crate::render_thread::pipeline_runtime::SceneTransitionKey;
    use crate::render_thread::pipeline_runtime::needs_gpu_preview_advance;
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
        assert!(!needs_gpu_preview_advance(&render_stage));
    }

    #[test]
    fn gpu_preview_does_not_advance_when_surface_is_ready() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, true);
        assert!(!needs_gpu_preview_advance(&render_stage));
    }

    #[test]
    fn gpu_preview_advances_when_requested_and_unresolved() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, false);
        assert!(needs_gpu_preview_advance(&render_stage));
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

    fn layout_with_display_zone() -> SpatialLayout {
        let mut layout = sample_layout(&["left", "display", "right"]);
        layout.zones[1].zone_name = Some("Display".to_owned());
        layout
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
    fn completed_deferred_zone_colors_can_drive_matching_layout() {
        let layout = sample_layout(&["left", "right"]);
        let zones = vec![zone("left", &[[255, 0, 0]]), zone("right", &[[0, 0, 255]])];

        assert!(can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
        assert!(!can_hold_zone_colors_for_deferred_sampling(
            &layout,
            &[zone("left", &[[255, 0, 0]])]
        ));
    }

    #[test]
    fn gpu_zone_sampling_ignores_display_only_zones_when_reusing_published_frame() {
        let layout = layout_with_display_zone();

        assert!(can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "right"])
        ));
        assert!(!can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "display", "right"])
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
