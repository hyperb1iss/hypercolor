use std::time::{Duration, Instant};

#[cfg(not(windows))]
pub(crate) const PRECISE_WAKE_GUARD: Duration = Duration::from_millis(1);
#[cfg(not(windows))]
const PRECISE_WAKE_SPIN_THRESHOLD: Duration = Duration::from_micros(150);

pub(crate) fn advance_deadline(
    previous_deadline: Instant,
    interval: Duration,
    now: Instant,
) -> Instant {
    if interval.is_zero() {
        return now;
    }

    let Some(next_deadline) = previous_deadline.checked_add(interval) else {
        return now;
    };
    if next_deadline > now {
        return next_deadline;
    }

    let interval_nanos = interval.as_nanos();
    if interval_nanos == 0 {
        return now;
    }

    let elapsed_nanos = now.saturating_duration_since(previous_deadline).as_nanos();
    let intervals_to_advance = elapsed_nanos
        .checked_div(interval_nanos)
        .unwrap_or_default()
        .saturating_add(1);
    let advance_intervals = u32::try_from(intervals_to_advance).unwrap_or(u32::MAX);
    let advance = interval.saturating_mul(advance_intervals);

    previous_deadline
        .checked_add(advance)
        .filter(|deadline| *deadline > now)
        .or_else(|| now.checked_add(interval))
        .unwrap_or(now)
}

#[cfg(not(windows))]
pub(crate) fn coarse_sleep_deadline(deadline: Instant, now: Instant) -> Option<Instant> {
    deadline
        .checked_sub(PRECISE_WAKE_GUARD)
        .filter(|coarse_deadline| *coarse_deadline > now)
}

#[cfg(windows)]
pub(crate) async fn wait_until_deadline(deadline: Instant) {
    spin_sleep::sleep_until(deadline);
}

#[cfg(not(windows))]
pub(crate) async fn wait_until_deadline(deadline: Instant) {
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::advance_deadline;

    #[test]
    fn advance_deadline_preserves_phase_when_scheduler_wakes_late() {
        let start = Instant::now();
        let late_now = start + Duration::from_millis(18);

        let next = advance_deadline(start, Duration::from_millis(16), late_now);

        assert_eq!(next, start + Duration::from_millis(32));
    }

    #[test]
    fn advance_deadline_keeps_regular_cadence_when_on_time() {
        let start = Instant::now();
        let now = start + Duration::from_millis(8);

        let next = advance_deadline(start, Duration::from_millis(16), now);

        assert_eq!(next, start + Duration::from_millis(16));
    }

    #[test]
    fn advance_deadline_skips_missed_intervals_without_bursting() {
        let start = Instant::now();
        let late_now = start + Duration::from_millis(51);

        let next = advance_deadline(start, Duration::from_millis(16), late_now);

        assert_eq!(next, start + Duration::from_millis(64));
    }

    #[test]
    #[cfg(not(windows))]
    fn coarse_sleep_deadline_uses_guard_band_when_there_is_headroom() {
        use super::{PRECISE_WAKE_GUARD, coarse_sleep_deadline};

        let now = Instant::now();
        let deadline = now + Duration::from_millis(16);

        let coarse = coarse_sleep_deadline(deadline, now).expect("guard band should apply");

        assert_eq!(
            coarse,
            deadline
                .checked_sub(PRECISE_WAKE_GUARD)
                .expect("guard band should fit within deadline")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn coarse_sleep_deadline_skips_sleep_when_deadline_is_inside_guard_band() {
        use super::{PRECISE_WAKE_GUARD, coarse_sleep_deadline};

        let now = Instant::now();
        let deadline = now + PRECISE_WAKE_GUARD / 2;

        assert!(coarse_sleep_deadline(deadline, now).is_none());
    }
}
