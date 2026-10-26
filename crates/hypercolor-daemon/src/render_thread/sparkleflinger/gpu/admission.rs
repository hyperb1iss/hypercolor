//! Bounded retries for GPU allocations that failed admission.
//!
//! A buffer or texture that runs out of memory comes back invalid while the
//! device stays alive. Whatever needed it is not cached, so the next use
//! tries again, and under sustained memory pressure that would rebuild the
//! allocation on every render tick. An [`AdmissionRetry`] spaces those
//! attempts out with a doubling delay. It only exists after a failure, and
//! it is only consulted when the allocation is missing, so a healthy frame
//! does no extra work.

use std::time::{Duration, Instant};

/// Wait after the first failed admission before trying again.
pub(super) const ADMISSION_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(100);

/// Longest wait between attempts while admission keeps failing.
pub(super) const ADMISSION_RETRY_MAX_DELAY: Duration = Duration::from_secs(2);

/// When an allocation of `shape` that failed admission may be tried again.
///
/// A different shape (size or format) is a different allocation, so it is
/// tried at once and starts its own schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AdmissionRetry<S> {
    shape: S,
    next_attempt: Instant,
    delay: Duration,
}

impl<S: Copy + PartialEq> AdmissionRetry<S> {
    /// Whether an allocation of `shape` may be attempted at `now`.
    pub(super) fn allows(retry: Option<&Self>, shape: S, now: Instant) -> bool {
        retry.is_none_or(|retry| retry.shape != shape || now >= retry.next_attempt)
    }

    /// The schedule after an allocation of `shape` failed at `now`.
    pub(super) fn after_failure(retry: Option<&Self>, shape: S, now: Instant) -> Self {
        let delay = match retry {
            Some(retry) if retry.shape == shape => {
                retry.delay.saturating_mul(2).min(ADMISSION_RETRY_MAX_DELAY)
            }
            _ => ADMISSION_RETRY_INITIAL_DELAY,
        };
        Self {
            shape,
            next_attempt: now + delay,
            delay,
        }
    }

    #[cfg(test)]
    pub(super) const fn delay(&self) -> Duration {
        self.delay
    }
}

/// The time admission retries are measured against. Production reads the
/// monotonic clock, and only on the miss path; tests can pin it.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AdmissionClock {
    #[cfg(test)]
    pinned: Option<Instant>,
}

impl AdmissionClock {
    pub(super) fn now(self) -> Instant {
        #[cfg(test)]
        if let Some(pinned) = self.pinned {
            return pinned;
        }
        Instant::now()
    }

    #[cfg(test)]
    pub(super) fn pin(&mut self, at: Instant) {
        self.pinned = Some(at);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{ADMISSION_RETRY_INITIAL_DELAY, ADMISSION_RETRY_MAX_DELAY, AdmissionRetry};

    #[test]
    fn admission_retry_doubles_to_its_cap_and_restarts_for_a_new_shape() {
        let start = Instant::now();
        let mut retry: Option<AdmissionRetry<u32>> = None;
        let mut delays = Vec::new();
        for _ in 0..7 {
            let next = AdmissionRetry::after_failure(retry.as_ref(), 1, start);
            delays.push(next.delay().as_millis());
            retry = Some(next);
        }
        assert_eq!(delays, [100, 200, 400, 800, 1600, 2000, 2000]);
        assert_eq!(ADMISSION_RETRY_MAX_DELAY, Duration::from_secs(2));

        let retry = AdmissionRetry::after_failure(None, 1, start);
        assert!(!AdmissionRetry::allows(Some(&retry), 1, start));
        assert!(!AdmissionRetry::allows(
            Some(&retry),
            1,
            start + Duration::from_millis(99)
        ));
        assert!(AdmissionRetry::allows(
            Some(&retry),
            1,
            start + ADMISSION_RETRY_INITIAL_DELAY
        ));
        assert!(
            AdmissionRetry::allows(Some(&retry), 2, start),
            "a new shape is a new allocation"
        );
        assert_eq!(
            AdmissionRetry::after_failure(Some(&retry), 2, start).delay(),
            ADMISSION_RETRY_INITIAL_DELAY
        );
    }
}
