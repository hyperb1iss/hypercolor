use std::time::{Duration, Instant};

use hypercolor_core::engine::FrameStats;
use hypercolor_core::engine::{FpsTier, RenderLoop, RenderLoopState};

use super::frame_admission::FrameAdmissionController;
use crate::deadline::advance_deadline;

pub(crate) use super::frame_admission::FrameAdmissionSample;

const PAUSED_POLL_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_THROTTLE_DELAY: Duration = Duration::from_millis(120);
const SESSION_SLEEP_THROTTLE_DELAY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipDecision {
    None,
    ReuseInputs,
    ReuseCanvas,
}

impl SkipDecision {
    pub(crate) fn from_frame_stats(stats: &FrameStats) -> Self {
        if !stats.budget_exceeded {
            return Self::None;
        }

        if stats.consecutive_misses >= 2 {
            Self::ReuseCanvas
        } else {
            Self::ReuseInputs
        }
    }
}

pub(crate) struct FrameExecution {
    pub(crate) next_wake: NextWake,
    pub(crate) next_skip_decision: SkipDecision,
}

impl FrameExecution {
    pub(crate) const fn throttle(delay: Duration) -> Self {
        Self {
            next_wake: NextWake::Delay(delay),
            next_skip_decision: SkipDecision::None,
        }
    }

    pub(crate) fn resolve_deadline(self, scheduled_start: Instant, now: Instant) -> Instant {
        self.next_wake.resolve_deadline(scheduled_start, now)
    }
}

/// Scheduler decision for when the next render iteration should begin.
pub(crate) enum NextWake {
    /// Continue on the regular render cadence using the current FPS interval.
    Interval(Duration),
    /// Hold the loop for a fixed delay before checking again.
    Delay(Duration),
}

impl NextWake {
    pub(crate) fn resolve_deadline(self, scheduled_start: Instant, now: Instant) -> Instant {
        match self {
            Self::Interval(interval) => advance_deadline(scheduled_start, interval, now),
            Self::Delay(delay) => now.checked_add(delay).unwrap_or(now),
        }
    }
}

pub(crate) struct FramePolicy {
    admission: FrameAdmissionController,
}

impl FramePolicy {
    pub(crate) fn new(configured_max_tier: FpsTier) -> Self {
        Self {
            admission: FrameAdmissionController::new(configured_max_tier),
        }
    }

    pub(crate) fn complete_render_frame(
        &mut self,
        render_loop: &mut RenderLoop,
        sample: FrameAdmissionSample,
    ) -> FrameExecution {
        let ceiling_tier = self.admission.record_frame(sample);
        match render_loop.frame_complete_with_max_tier(Some(ceiling_tier)) {
            Some(frame_stats) => FrameExecution {
                next_wake: NextWake::Interval(render_loop.target_interval()),
                next_skip_decision: SkipDecision::from_frame_stats(&frame_stats),
            },
            None => FrameExecution::throttle(Duration::ZERO),
        }
    }

    pub(crate) fn complete_throttled_frame(
        &mut self,
        render_loop: &mut RenderLoop,
        delay: Duration,
    ) -> FrameExecution {
        let _ = render_loop.frame_complete();
        FrameExecution::throttle(delay)
    }

    pub(crate) const fn should_idle_throttle(
        &self,
        effect_running: bool,
        screen_capture_active: bool,
    ) -> bool {
        !effect_running && !screen_capture_active
    }

    pub(crate) fn idle_throttle_execution(
        &mut self,
        render_loop: &mut RenderLoop,
    ) -> FrameExecution {
        self.complete_throttled_frame(render_loop, IDLE_THROTTLE_DELAY)
    }

    pub(crate) fn sleep_throttle_execution(
        &mut self,
        render_loop: &mut RenderLoop,
    ) -> FrameExecution {
        self.complete_throttled_frame(render_loop, SESSION_SLEEP_THROTTLE_DELAY)
    }

    pub(crate) fn inactive_loop_execution(
        &self,
        loop_state: RenderLoopState,
    ) -> Option<FrameExecution> {
        (loop_state == RenderLoopState::Paused)
            .then_some(FrameExecution::throttle(PAUSED_POLL_INTERVAL))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use hypercolor_core::engine::{FpsTier, RenderLoop, RenderLoopState};

    use super::{FrameAdmissionSample, FrameExecution, FramePolicy, NextWake, SkipDecision};

    fn clean_sample() -> FrameAdmissionSample {
        FrameAdmissionSample {
            total_us: 8_000,
            producer_us: 4_000,
            composition_us: 800,
            push_us: 500,
            publish_us: 100,
            wake_late_us: 0,
            jitter_us: 0,
            full_frame_copy_count: 0,
            output_errors: 0,
        }
    }

    #[test]
    fn render_frame_completion_applies_admission_ceiling() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());

        let mut policy = FramePolicy::new(FpsTier::Full);
        let first = policy.complete_render_frame(
            &mut render_loop,
            FrameAdmissionSample {
                output_errors: 1,
                ..clean_sample()
            },
        );
        assert!(matches!(first.next_wake, NextWake::Interval(_)));
        assert_eq!(first.next_skip_decision, SkipDecision::None);

        assert!(render_loop.tick());
        let second = policy.complete_render_frame(
            &mut render_loop,
            FrameAdmissionSample {
                output_errors: 1,
                ..clean_sample()
            },
        );

        assert!(matches!(second.next_wake, NextWake::Interval(_)));
        assert_eq!(second.next_skip_decision, SkipDecision::None);
        assert_eq!(render_loop.stats().max_tier, FpsTier::High);
    }

    #[test]
    fn next_wake_interval_resolution_catches_up_to_now_when_late() {
        let scheduled_start = Instant::now();
        let late_now = scheduled_start + Duration::from_millis(50);

        let next = NextWake::Interval(Duration::from_millis(16))
            .resolve_deadline(scheduled_start, late_now);

        assert_eq!(next, late_now);
    }

    #[test]
    fn next_wake_delay_resolution_resets_from_current_time() {
        let scheduled_start = Instant::now();
        let now = scheduled_start + Duration::from_millis(50);

        let next =
            NextWake::Delay(Duration::from_millis(120)).resolve_deadline(scheduled_start, now);

        assert_eq!(next, now + Duration::from_millis(120));
    }

    #[test]
    fn frame_execution_delay_constructor_clears_skip_decision() {
        let execution = FrameExecution::throttle(Duration::from_millis(120));

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(120)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn throttled_frame_completion_uses_delay_execution() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());

        let mut policy = FramePolicy::new(FpsTier::Full);
        let execution =
            policy.complete_throttled_frame(&mut render_loop, Duration::from_millis(120));

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(120)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn inactive_loop_execution_uses_paused_poll_delay() {
        let policy = FramePolicy::new(FpsTier::Full);
        let execution = policy
            .inactive_loop_execution(RenderLoopState::Paused)
            .expect("paused loop should poll again");

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(50)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn inactive_loop_execution_ignores_stopped_state() {
        let policy = FramePolicy::new(FpsTier::Full);

        assert!(
            policy
                .inactive_loop_execution(RenderLoopState::Stopped)
                .is_none()
        );
    }

    #[test]
    fn idle_throttle_predicate_requires_no_effect_or_capture() {
        let policy = FramePolicy::new(FpsTier::Full);

        assert!(policy.should_idle_throttle(false, false));
        assert!(!policy.should_idle_throttle(true, false));
        assert!(!policy.should_idle_throttle(false, true));
    }

    #[test]
    fn idle_throttle_execution_uses_idle_delay() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        let mut policy = FramePolicy::new(FpsTier::Full);

        let execution = policy.idle_throttle_execution(&mut render_loop);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(120)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn sleep_throttle_execution_uses_sleep_delay() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        let mut policy = FramePolicy::new(FpsTier::Full);

        let execution = policy.sleep_throttle_execution(&mut render_loop);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(250)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }
}
