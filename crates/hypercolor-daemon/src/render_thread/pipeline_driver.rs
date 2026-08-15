use std::time::Instant;

use tracing::{debug, info};

use crate::deadline::wait_until_deadline;
use hypercolor_core::engine::RenderLoopState;

use super::RenderThreadState;
use super::frame_executor::{execute_frame, service_scene_transactions};
use super::frame_policy::SkipDecision;
use super::pipeline_runtime::PipelineRuntime;

pub(crate) async fn run_pipeline(state: RenderThreadState, mut runtime: PipelineRuntime) {
    info!(
        mode = ?state.render_acceleration_mode,
        "render pipeline started"
    );
    let mut skip_decision = SkipDecision::None;
    let mut next_frame_at = Instant::now();
    let mut rebase_clock_on_resume = false;
    let mut observed_pause_generation = 0;

    loop {
        let scheduled_start = next_frame_at;
        wait_until_deadline(scheduled_start).await;

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
                next_frame_at = execution.resolve_deadline(scheduled_start, Instant::now());
                continue;
            }

            debug!("render loop not running, exiting pipeline");
            break;
        }

        if should_rebase_clock {
            runtime.frame_loop.clock.rebase(scheduled_start);
        }
        let frame = execute_frame(&state, &mut runtime, scheduled_start, skip_decision).await;
        skip_decision = frame.next_skip_decision;
        next_frame_at = frame.resolve_deadline(scheduled_start, Instant::now());
    }

    info!("render pipeline exited");
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
    }

    runtime.frame_loop.clear_input_demands();
    clear_inactive_render_groups(state, runtime).await;
    runtime.frame_policy.inactive_loop_execution(loop_state)
}

async fn clear_inactive_render_groups(state: &RenderThreadState, runtime: &mut PipelineRuntime) {
    let active_group_count = {
        let manager = state.scene_manager.read().await;
        manager
            .active_render_groups()
            .iter()
            .filter(|group| group.enabled && group.effect_id.is_some())
            .count()
    };

    if active_group_count == 0 {
        runtime.render.clear_inactive_groups();
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
