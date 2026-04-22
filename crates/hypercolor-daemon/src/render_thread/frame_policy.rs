use std::time::Duration;

use hypercolor_core::engine::{FpsTier, RenderLoop};

use super::frame_admission::{FrameAdmissionController, FrameAdmissionSample};
use super::frame_pacing::{FrameExecution, NextWake, SkipDecision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameThrottleKind {
    Idle,
    SessionSleep,
}

impl FrameThrottleKind {
    const fn delay(self) -> Duration {
        match self {
            Self::Idle => Duration::from_millis(120),
            Self::SessionSleep => Duration::from_millis(250),
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
        let admission = self.admission.record_frame(sample);
        match render_loop.frame_complete_with_max_tier(Some(admission.ceiling_tier)) {
            Some(frame_stats) => FrameExecution {
                next_wake: NextWake::Interval(render_loop.target_interval()),
                next_skip_decision: SkipDecision::from_frame_stats(&frame_stats),
            },
            None => FrameExecution {
                next_wake: NextWake::Delay(Duration::ZERO),
                next_skip_decision: SkipDecision::None,
            },
        }
    }

    pub(crate) fn complete_throttle_frame(
        &mut self,
        render_loop: &mut RenderLoop,
        throttle: FrameThrottleKind,
    ) -> FrameExecution {
        let _ = render_loop.frame_complete();
        FrameExecution {
            next_wake: NextWake::Delay(throttle.delay()),
            next_skip_decision: SkipDecision::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use hypercolor_core::engine::{FpsTier, RenderLoop};

    use super::{FramePolicy, FrameThrottleKind};
    use crate::render_thread::frame_admission::FrameAdmissionSample;
    use crate::render_thread::frame_pacing::{NextWake, SkipDecision};

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
    fn idle_throttle_completion_returns_idle_delay_without_skip() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        thread::sleep(Duration::from_millis(1));

        let mut policy = FramePolicy::new(FpsTier::Full);
        let execution = policy.complete_throttle_frame(&mut render_loop, FrameThrottleKind::Idle);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(120)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }

    #[test]
    fn session_sleep_throttle_completion_returns_sleep_delay_without_skip() {
        let mut render_loop = RenderLoop::new(60);
        render_loop.start();
        assert!(render_loop.tick());
        thread::sleep(Duration::from_millis(1));

        let mut policy = FramePolicy::new(FpsTier::Full);
        let execution =
            policy.complete_throttle_frame(&mut render_loop, FrameThrottleKind::SessionSleep);

        assert!(matches!(
            execution.next_wake,
            NextWake::Delay(delay) if delay == Duration::from_millis(250)
        ));
        assert_eq!(execution.next_skip_decision, SkipDecision::None);
    }
}
