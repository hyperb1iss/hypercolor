use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::RenderThreadState;
use super::frame_executor::execute_frame;
use super::frame_pacing::{NextWake, advance_deadline, wait_until_frame_deadline};
use super::frame_policy::SkipDecision;
use super::frame_state::{reconcile_audio_capture, reconcile_screen_capture};
use super::pipeline_runtime::PipelineRuntime;

const PAUSED_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) async fn run_pipeline(state: RenderThreadState, mut runtime: PipelineRuntime) {
    info!(
        mode = ?state.render_acceleration_mode,
        "render pipeline started"
    );
    let mut skip_decision = SkipDecision::None;
    let mut next_frame_at = Instant::now();

    loop {
        let scheduled_start = next_frame_at;
        wait_until_frame_deadline(scheduled_start).await;

        let should_render = {
            let mut render_loop = state.render_loop.write().await;
            render_loop.tick()
        };

        if !should_render {
            if handle_inactive_render_loop(&state, &mut runtime).await {
                next_frame_at = Instant::now();
                tokio::time::sleep(PAUSED_POLL_INTERVAL).await;
                continue;
            }

            debug!("render loop not running, exiting pipeline");
            break;
        }

        let frame = execute_frame(&state, &mut runtime, scheduled_start, skip_decision).await;
        skip_decision = frame.next_skip_decision;
        next_frame_at = match frame.next_wake {
            NextWake::Interval(interval) => {
                advance_deadline(scheduled_start, interval, Instant::now())
            }
            NextWake::Delay(delay) => Instant::now()
                .checked_add(delay)
                .unwrap_or_else(Instant::now),
        };
    }

    info!("render pipeline exited");
}

async fn handle_inactive_render_loop(
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
) -> bool {
    let loop_state = {
        let render_loop = state.render_loop.read().await;
        render_loop.state()
    };

    clear_capture_demand(state, runtime).await;
    loop_state == hypercolor_core::engine::RenderLoopState::Paused
}

async fn clear_capture_demand(state: &RenderThreadState, runtime: &mut PipelineRuntime) {
    reconcile_audio_capture(
        state,
        false,
        &mut runtime.frame_loop.last_audio_capture_active,
    )
    .await;
    reconcile_screen_capture(
        state,
        false,
        &mut runtime.frame_loop.last_screen_capture_active,
    )
    .await;
}
