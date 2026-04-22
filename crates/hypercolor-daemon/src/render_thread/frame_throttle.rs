use std::time::{Duration, Instant};

use tracing::{debug, trace};

use hypercolor_core::engine::RenderLoop;
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_core::types::event::FrameTiming;
use hypercolor_types::session::OffOutputBehavior;

use super::frame_io::publish_frame_updates;
use super::frame_policy::{FrameExecution, NextWake, SkipDecision};
use super::pipeline_runtime::{
    OutputArtifactsState, PublicationCadenceState, RenderSurfaceSnapshot, ThrottleState,
};
use super::scene_snapshot::FrameSceneSnapshot;
use super::{RenderThreadState, micros_between, u64_to_u32};
use crate::discovery::handle_async_write_failures;
use crate::performance::{CompositorBackendKind, FrameTimeline, LatestFrameMetrics};

const IDLE_THROTTLE_DELAY: Duration = Duration::from_millis(120);
const SESSION_SLEEP_THROTTLE_DELAY: Duration = Duration::from_millis(250);

pub(crate) async fn maybe_idle_throttle(
    state: &RenderThreadState,
    effect_running: bool,
    screen_capture_active: bool,
    throttle: &mut ThrottleState,
) -> Option<FrameExecution> {
    let can_idle_throttle = can_idle_throttle(effect_running, screen_capture_active);

    if effect_running {
        throttle.note_effect_running();
        return None;
    }

    if can_idle_throttle && !throttle.idle_black_pushed() {
        debug!(
            "No active effect or capture input; layout changes render black until an effect or input source starts"
        );
    }

    if can_idle_throttle && throttle.idle_black_pushed() {
        let mut render_loop = state.render_loop.write().await;
        return Some(complete_throttle_frame(
            &mut render_loop,
            IDLE_THROTTLE_DELAY,
        ));
    }

    None
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "sleep-throttle execution is easier to audit when frame synthesis, output, and telemetry stay in one async block"
)]
pub(crate) async fn maybe_sleep_throttle(
    state: &RenderThreadState,
    scene_snapshot: &FrameSceneSnapshot,
    frame_start: Instant,
    scene_snapshot_done_us: u32,
    render_surfaces: RenderSurfaceSnapshot,
    output_artifacts: &mut OutputArtifactsState,
    throttle: &mut ThrottleState,
    publication_cadence: &mut PublicationCadenceState,
) -> Option<FrameExecution> {
    let power_state = scene_snapshot.output_power;
    if throttle.sleep_black_pushed() {
        let mut render_loop = state.render_loop.write().await;
        return Some(complete_throttle_frame(
            &mut render_loop,
            SESSION_SLEEP_THROTTLE_DELAY,
        ));
    }

    if power_state.off_output_behavior == OffOutputBehavior::Release {
        output_artifacts.clear_zones();
        let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
        let surface = output_artifacts.static_surface(
            state.canvas_dims.width(),
            state.canvas_dims.height(),
            [0, 0, 0],
        );
        let publish_stats = publish_frame_updates(
            state,
            output_artifacts.frame_mut(),
            &AudioData::silence(),
            Some(Canvas::from_published_surface(&surface)),
            &[],
            &[],
            Some(surface),
            None,
            None,
            None,
            frame_num_u32,
            scene_snapshot.elapsed_ms,
            publication_cadence,
            false,
            false,
            FrameTiming {
                producer_us: 0,
                composition_us: 0,
                render_us: 0,
                sample_us: 0,
                push_us: 0,
                total_us: 0,
                budget_us: scene_snapshot.budget_us,
            },
        );
        let publish_us = publish_stats.elapsed_us;
        trace!(
            publish_us,
            "published cleared frame/canvas for release sleep"
        );
        throttle.note_sleep_frame_published();
        let mut render_loop = state.render_loop.write().await;
        return Some(complete_throttle_frame(
            &mut render_loop,
            SESSION_SLEEP_THROTTLE_DELAY,
        ));
    }

    let surface = output_artifacts.static_surface(
        state.canvas_dims.width(),
        state.canvas_dims.height(),
        power_state.off_output_color,
    );
    let canvas = Canvas::from_published_surface(&surface);
    let sample_start = Instant::now();
    scene_snapshot
        .spatial_engine
        .sample_into(&canvas, output_artifacts.zones_mut());
    let zone_colors = output_artifacts.zones();
    let layout = scene_snapshot.spatial_engine.layout();
    let sample_done_at = Instant::now();
    let sample_us = micros_between(sample_start, sample_done_at);
    let sample_done_us = micros_between(frame_start, sample_done_at);

    let push_start = Instant::now();
    let (write_stats, async_failures) = {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager.write_frame(zone_colors, layout.as_ref()).await;
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    let output_done_at = Instant::now();
    let push_us = micros_between(push_start, output_done_at);
    let output_done_us = micros_between(frame_start, output_done_at);

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures).await;
    }

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = sample_us.saturating_add(push_us);
    let publish_stats = publish_frame_updates(
        state,
        output_artifacts.frame_mut(),
        &AudioData::silence(),
        Some(canvas),
        &[],
        &[],
        Some(surface),
        None,
        None,
        None,
        frame_num_u32,
        scene_snapshot.elapsed_ms,
        publication_cadence,
        false,
        false,
        FrameTiming {
            producer_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us,
            push_us,
            total_us: timing_total_us,
            budget_us: scene_snapshot.budget_us,
        },
    );
    let publish_us = publish_stats.elapsed_us;
    let publish_done_at = Instant::now();
    let publish_done_us = micros_between(frame_start, publish_done_at);
    let total_us = publish_done_us;
    let overhead_us =
        total_us.saturating_sub(sample_us.saturating_add(push_us).saturating_add(publish_us));

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(LatestFrameMetrics {
            timestamp_ms: scene_snapshot.elapsed_ms,
            input_us: 0,
            producer_us: 0,
            producer_render_us: 0,
            producer_scene_compose_us: 0,
            composition_us: 0,
            render_us: 0,
            sample_us,
            push_us,
            postprocess_us: 0,
            publish_us,
            publish_frame_data_us: publish_stats.frame_data_us,
            publish_group_canvas_us: publish_stats.group_canvas_us,
            publish_preview_us: publish_stats.preview_us,
            publish_events_us: publish_stats.events_us,
            overhead_us,
            total_us,
            wake_late_us: 0,
            jitter_us: 0,
            reused_inputs: false,
            reused_canvas: false,
            retained_effect: false,
            retained_screen: false,
            composition_bypassed: false,
            gpu_zone_sampling: false,
            gpu_sample_deferred: false,
            gpu_sample_retry_hit: false,
            gpu_sample_queue_saturated: false,
            gpu_sample_wait_blocked: false,
            cpu_readback_skipped: false,
            compositor_backend: CompositorBackendKind::Cpu,
            logical_layer_count: 0,
            render_group_count: scene_snapshot.scene_runtime.active_render_group_count(),
            scene_active: scene_snapshot.scene_runtime.active_scene_id.is_some(),
            scene_transition_active: scene_snapshot.scene_runtime.active_transition.is_some(),
            render_surface_slot_count: render_surfaces.slot_count,
            render_surface_free_slots: render_surfaces.free_slots,
            render_surface_published_slots: render_surfaces.published_slots,
            render_surface_dequeued_slots: render_surfaces.dequeued_slots,
            scene_pool_saturation_reallocs: render_surfaces.scene_pool_saturation_reallocs,
            direct_pool_saturation_reallocs: render_surfaces.direct_pool_saturation_reallocs,
            scene_pool_grown_slots: render_surfaces.scene_pool_grown_slots,
            direct_pool_grown_slots: render_surfaces.direct_pool_grown_slots,
            canvas_receiver_count: render_surfaces.canvas_receivers,
            full_frame_copy_count: publish_stats.full_frame_copy_count,
            full_frame_copy_bytes: publish_stats.full_frame_copy_bytes,
            output_errors: u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX),
            timeline: FrameTimeline {
                frame_token: scene_snapshot.frame_token,
                budget_us: scene_snapshot.budget_us,
                scene_snapshot_done_us,
                input_done_us: scene_snapshot_done_us,
                producer_done_us: scene_snapshot_done_us,
                composition_done_us: scene_snapshot_done_us,
                sample_done_us,
                output_done_us,
                publish_done_us,
                frame_done_us: total_us,
            },
        });
    }

    throttle.note_sleep_frame_published();
    let mut render_loop = state.render_loop.write().await;
    Some(complete_throttle_frame(
        &mut render_loop,
        SESSION_SLEEP_THROTTLE_DELAY,
    ))
}

const fn can_idle_throttle(effect_running: bool, screen_capture_active: bool) -> bool {
    !effect_running && !screen_capture_active
}

fn complete_throttle_frame(render_loop: &mut RenderLoop, delay: Duration) -> FrameExecution {
    let _ = render_loop.frame_complete();
    FrameExecution {
        next_wake: NextWake::Delay(delay),
        next_skip_decision: SkipDecision::None,
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use hypercolor_core::engine::RenderLoop;

    use super::{
        IDLE_THROTTLE_DELAY, SESSION_SLEEP_THROTTLE_DELAY, can_idle_throttle,
        complete_throttle_frame,
    };
    use crate::render_thread::frame_policy::{NextWake, SkipDecision};

    #[test]
    fn idle_throttle_completion_returns_idle_delay_without_skip() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        thread::sleep(std::time::Duration::from_millis(1));

        let execution = complete_throttle_frame(&mut render_loop, IDLE_THROTTLE_DELAY);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == IDLE_THROTTLE_DELAY
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn session_sleep_throttle_completion_returns_sleep_delay_without_skip() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        thread::sleep(std::time::Duration::from_millis(1));

        let execution = complete_throttle_frame(&mut render_loop, SESSION_SLEEP_THROTTLE_DELAY);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == SESSION_SLEEP_THROTTLE_DELAY
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn idle_throttle_predicate_requires_no_effect_and_no_screen_capture() {
        assert!(can_idle_throttle(false, false));
        assert!(!can_idle_throttle(true, false));
        assert!(!can_idle_throttle(false, true));
        assert!(!can_idle_throttle(true, true));
    }
}
