use std::time::Instant;

use tracing::{debug, info};

use crate::deadline::wait_until_deadline;
use hypercolor_core::engine::RenderLoopState;

use super::frame_executor::{execute_frame, service_scene_transactions};
use super::frame_policy::SkipDecision;
use super::output_gate::{RenderWakeReason, wait_for_output_demand};
use super::pipeline_runtime::PipelineRuntime;
use super::supervision::RenderSupervision;
use super::{RenderThreadState, u64_to_u32};

pub(crate) async fn run_pipeline(
    state: RenderThreadState,
    mut runtime: PipelineRuntime,
    supervision: RenderSupervision,
) {
    info!(
        mode = ?state.render_acceleration_mode,
        "render pipeline started"
    );
    clear_stale_park(&state).await;
    let mut skip_decision = SkipDecision::None;
    let mut next_frame_at = Instant::now();
    let mut rebase_clock_on_resume = false;
    let mut observed_pause_generation = 0;

    loop {
        let scheduled_start = next_frame_at;
        wait_until_deadline(scheduled_start).await;
        #[cfg(feature = "persistence-test-hooks")]
        raise_injected_fault(&supervision, &mut runtime);

        let tick = {
            let mut render_loop = state.render_loop.write().await;
            RenderTickSnapshot {
                should_render: render_loop.tick(),
                state: render_loop.state(),
                pause_generation: render_loop.pause_generation(),
            }
        };
        let should_rebase_clock = observe_render_tick(
            &mut rebase_clock_on_resume,
            &mut observed_pause_generation,
            tick,
        );

        if !tick.should_render {
            if let Some(execution) =
                handle_inactive_render_loop(&state, &mut runtime, tick.state).await
            {
                let now = Instant::now();
                supervision.heartbeat(now);
                next_frame_at = execution.resolve_deadline(scheduled_start, now);
                continue;
            }

            debug!("render loop not running, exiting pipeline");
            break;
        }

        if should_rebase_clock {
            runtime.frame_loop.clock.rebase(scheduled_start);
        }
        let frame = execute_frame(&state, &mut runtime, scheduled_start, skip_decision).await;
        if frame.parks() {
            let reason =
                wait_for_output_demand(&state, &mut runtime.frame_loop, &supervision).await;
            resume_from_park(&state, &mut runtime, reason).await;
            skip_decision = SkipDecision::None;
            next_frame_at = Instant::now();
            continue;
        }
        let now = Instant::now();
        supervision.frame_completed(now);
        skip_decision = frame.next_skip_decision;
        next_frame_at = frame.resolve_deadline(scheduled_start, now);
    }

    info!("render pipeline exited");
}

/// Leave a park that a previous pipeline entered and never left.
///
/// The park flag and its hold live on the shared render loop, but the
/// output gate that would end them belongs to the pipeline. A pipeline that
/// replaces one that failed while parked starts with a fresh gate, so it
/// would otherwise render with effect time still held.
async fn clear_stale_park(state: &RenderThreadState) {
    let was_parked = {
        let mut render_loop = state.render_loop.write().await;
        let was_parked = render_loop.is_parked();
        render_loop.unpark();
        was_parked
    };
    if was_parked {
        state.performance.write().await.note_output_unparked();
    }
}

/// Pick the loop back up after a park without a jump in effect time.
///
/// Effects advance by the frame clock's delta, so the clock restarts one
/// target interval back: the first frame after the wake steps effect time
/// forward by exactly one frame instead of by the whole parked span, and no
/// catch-up frames follow. Capture freshness skips the wake frame itself,
/// whose asynchronous producers still show output requested before the
/// park.
async fn resume_from_park(
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
    reason: RenderWakeReason,
) {
    let now = Instant::now();
    let (interval, resume_frame) = {
        let mut render_loop = state.render_loop.write().await;
        render_loop.unpark();
        (render_loop.target_interval(), render_loop.frame_number())
    };
    if reason == RenderWakeReason::RenderLoopState {
        // A paused or stopped loop is idle for its own reason, not for
        // lack of consumers; the gate parks it again if it resumes with
        // nothing to render for.
        runtime.frame_loop.output_gate.leave_park();
        state.performance.write().await.note_output_unparked();
    }
    runtime
        .frame_loop
        .clock
        .rebase(now.checked_sub(interval).unwrap_or(now));
    state
        .preview_runtime
        .note_render_resume(u64_to_u32(resume_frame));
    state
        .performance
        .write()
        .await
        .note_output_woken(reason.as_str());
    debug!(reason = reason.as_str(), "Render loop woke from a park");
}

/// Raise a fault a test queued, on the render thread, exactly where a real
/// one would surface: between frames, with the pipeline's state live.
#[cfg(feature = "persistence-test-hooks")]
fn raise_injected_fault(supervision: &RenderSupervision, runtime: &mut PipelineRuntime) {
    use super::supervision::InjectedRenderFault;

    match supervision.take_injected_fault() {
        None => {}
        Some(InjectedRenderFault::Panic(message)) => panic!("{message}"),
        Some(InjectedRenderFault::Stall(duration)) => std::thread::sleep(duration),
        Some(InjectedRenderFault::GpuValidationError) => {
            let raised = runtime
                .render
                .sparkleflinger
                .raise_uncaptured_validation_error();
            if !raised {
                tracing::warn!(
                    "injected GPU validation error skipped: the compositor runs on the CPU"
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderTickSnapshot {
    should_render: bool,
    state: RenderLoopState,
    pause_generation: u64,
}

fn observe_render_tick(
    pending_rebase: &mut bool,
    observed_pause_generation: &mut u64,
    tick: RenderTickSnapshot,
) -> bool {
    let pause_generation_changed = tick.pause_generation != *observed_pause_generation;
    *observed_pause_generation = tick.pause_generation;
    *pending_rebase |= pause_generation_changed;

    if !tick.should_render {
        *pending_rebase |= tick.state == RenderLoopState::Paused;
        return false;
    }

    std::mem::take(pending_rebase)
}

async fn handle_inactive_render_loop(
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
    loop_state: RenderLoopState,
) -> Option<super::frame_policy::FrameExecution> {
    // A paused loop still accepts layout edits, and whoever submitted one is
    // blocked on it holding the layout update lock. Skipping the queue here
    // wedges that caller, and then every path that reconciles connectivity
    // behind the same lock. Idle ticks stay untouched when nothing is queued.
    if state.scene_transactions.has_pending() || runtime.render.pending_layout_activation.is_some()
    {
        service_scene_transactions(
            state,
            &mut runtime.scene,
            &mut runtime.frame_loop,
            &mut runtime.render,
        )
        .await;
        runtime.render.sparkleflinger.end_gpu_frame();
    }

    runtime.frame_loop.clear_input_demands();
    clear_inactive_render_zones(state, runtime).await;
    runtime.frame_policy.inactive_loop_execution(loop_state)
}

async fn clear_inactive_render_zones(state: &RenderThreadState, runtime: &mut PipelineRuntime) {
    // Prepared zone resources remain tied to the live effect-pool generation
    // until activation commits or aborts on a later tick.
    if runtime.render.pending_layout_activation.is_some() {
        return;
    }

    let active_zone_count = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .resolved_zones()
            .iter()
            .filter(|zone| zone.enabled && zone.effect_ids().next().is_some())
            .count()
    };

    if active_zone_count == 0 {
        runtime.render.clear_inactive_zones();
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::engine::RenderLoopState;

    use super::{RenderTickSnapshot, observe_render_tick};

    #[test]
    fn paused_tick_keeps_rebase_pending_when_resume_follows_snapshot() {
        let mut pending_rebase = false;
        let mut observed_pause_generation = 0;

        assert!(!observe_render_tick(
            &mut pending_rebase,
            &mut observed_pause_generation,
            RenderTickSnapshot {
                should_render: false,
                state: RenderLoopState::Paused,
                pause_generation: 1,
            },
        ));
        assert!(pending_rebase);
        assert!(observe_render_tick(
            &mut pending_rebase,
            &mut observed_pause_generation,
            RenderTickSnapshot {
                should_render: true,
                state: RenderLoopState::Running,
                pause_generation: 1,
            },
        ));
        assert!(!pending_rebase);
    }

    #[test]
    fn pause_and_resume_between_ticks_still_rebases_clock() {
        let mut pending_rebase = false;
        let mut observed_pause_generation = 0;

        assert!(observe_render_tick(
            &mut pending_rebase,
            &mut observed_pause_generation,
            RenderTickSnapshot {
                should_render: true,
                state: RenderLoopState::Running,
                pause_generation: 1,
            },
        ));
        assert!(!pending_rebase);
    }
}
