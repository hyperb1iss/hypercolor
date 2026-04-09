use std::time::{Duration, Instant};

use hypercolor_core::engine::FrameStats;

pub(crate) const PRECISE_WAKE_GUARD: Duration = Duration::from_micros(1_000);
const PRECISE_WAKE_SPIN_THRESHOLD: Duration = Duration::from_micros(150);

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

/// Scheduler decision for when the next render iteration should begin.
pub(crate) enum NextWake {
    /// Continue on the regular render cadence using the current FPS interval.
    Interval(Duration),
    /// Hold the loop for a fixed delay before checking again.
    Delay(Duration),
}

pub(crate) fn advance_deadline(
    previous_deadline: Instant,
    interval: Duration,
    now: Instant,
) -> Instant {
    previous_deadline
        .checked_add(interval)
        .unwrap_or(now)
        .max(now)
}

pub(crate) fn coarse_sleep_deadline(deadline: Instant, now: Instant) -> Option<Instant> {
    deadline
        .checked_sub(PRECISE_WAKE_GUARD)
        .filter(|coarse_deadline| *coarse_deadline > now)
}

pub(crate) async fn wait_until_frame_deadline(deadline: Instant) {
    let now = Instant::now();
    if let Some(coarse_deadline) = coarse_sleep_deadline(deadline, now) {
        tokio::time::sleep_until(tokio::time::Instant::from_std(coarse_deadline)).await;
    }

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }

        if deadline.duration_since(now) > PRECISE_WAKE_SPIN_THRESHOLD {
            std::thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}
