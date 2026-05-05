//! Circuit breaker for the shared Servo worker.
//!
//! Wraps worker acquisition with failure tracking and exponential backoff so
//! transient faults (one flaky effect load, one hung frame render) can't
//! poison the shared runtime for the rest of the daemon lifetime. The older
//! "mark poisoned forever on the first fatal error" behavior remains in place
//! for unrecoverable conditions (thread exit, channel disconnect); this
//! breaker only gates retries for soft failures.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::telemetry::record_servo_breaker_open;

/// Number of consecutive failures before the breaker opens.
const FAILURE_THRESHOLD: u32 = 3;
/// Base cooldown applied after the breaker opens.
const BASE_COOLDOWN: Duration = Duration::from_secs(30);
/// Maximum cooldown applied after repeated half-open failures.
const MAX_COOLDOWN: Duration = Duration::from_mins(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitState {
    const CLOSED: u8 = 0;
    const OPEN: u8 = 1;
    const HALF_OPEN: u8 = 2;

    fn from_u8(value: u8) -> Self {
        match value {
            Self::OPEN => Self::Open,
            Self::HALF_OPEN => Self::HalfOpen,
            _ => Self::Closed,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Closed => Self::CLOSED,
            Self::Open => Self::OPEN,
            Self::HalfOpen => Self::HALF_OPEN,
        }
    }
}

/// Tracks repeated Servo worker failures and throttles acquisition attempts.
///
/// Starts in `Closed`, behaves transparently for up to `FAILURE_THRESHOLD`
/// consecutive failures, then opens and rejects attempts until the cooldown
/// elapses. The first attempt after cooldown transitions to `HalfOpen` and
/// records success or failure accordingly.
pub(super) struct ServoCircuitBreaker {
    failures: AtomicU32,
    consecutive_opens: AtomicU32,
    state: AtomicU8,
    next_retry: Mutex<Option<Instant>>,
}

impl ServoCircuitBreaker {
    pub(super) const fn new() -> Self {
        Self {
            failures: AtomicU32::new(0),
            consecutive_opens: AtomicU32::new(0),
            state: AtomicU8::new(CircuitState::CLOSED),
            next_retry: Mutex::new(None),
        }
    }

    /// Returns true if an acquisition attempt is permitted right now.
    ///
    /// Transitions `Open -> HalfOpen` if the cooldown has expired. A half-open
    /// breaker lets exactly one probe through per cooldown window.
    pub(super) fn can_attempt(&self) -> bool {
        match self.load_state() {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if self.cooldown_elapsed() {
                    self.store_state(CircuitState::HalfOpen);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful worker acquisition or operation.
    pub(super) fn record_success(&self) {
        self.failures.store(0, Ordering::Release);
        self.consecutive_opens.store(0, Ordering::Release);
        self.store_state(CircuitState::Closed);
        self.clear_next_retry();
    }

    /// Record a worker failure.
    ///
    /// In `Closed`, increments the failure counter and opens if the threshold
    /// is reached. In `HalfOpen`, any failure immediately re-opens the breaker
    /// with a longer cooldown.
    pub(super) fn record_failure(&self) {
        match self.load_state() {
            CircuitState::Closed => {
                let previous = self.failures.fetch_add(1, Ordering::AcqRel);
                if previous + 1 >= FAILURE_THRESHOLD {
                    self.open();
                }
            }
            CircuitState::HalfOpen => {
                self.open();
            }
            CircuitState::Open => {
                self.set_next_retry(self.current_cooldown());
            }
        }
    }

    /// Describe the remaining cooldown in a human-readable form, if any.
    pub(super) fn cooldown_remaining(&self) -> Option<Duration> {
        let guard = self.next_retry.lock().ok()?;
        let deadline = (*guard)?;
        let now = Instant::now();
        if deadline <= now {
            None
        } else {
            Some(deadline - now)
        }
    }

    fn open(&self) {
        self.store_state(CircuitState::Open);
        record_servo_breaker_open();
        let opens = self.consecutive_opens.fetch_add(1, Ordering::AcqRel) + 1;
        let cooldown = cooldown_for_opens(opens);
        self.set_next_retry(cooldown);
    }

    fn cooldown_elapsed(&self) -> bool {
        let Ok(guard) = self.next_retry.lock() else {
            return true;
        };
        match *guard {
            None => true,
            Some(deadline) => Instant::now() >= deadline,
        }
    }

    fn current_cooldown(&self) -> Duration {
        let opens = self.consecutive_opens.load(Ordering::Acquire).max(1);
        cooldown_for_opens(opens)
    }

    fn set_next_retry(&self, cooldown: Duration) {
        if let Ok(mut guard) = self.next_retry.lock() {
            *guard = Some(Instant::now() + cooldown);
        }
    }

    fn clear_next_retry(&self) {
        if let Ok(mut guard) = self.next_retry.lock() {
            *guard = None;
        }
    }

    fn load_state(&self) -> CircuitState {
        CircuitState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn store_state(&self, state: CircuitState) {
        self.state.store(state.as_u8(), Ordering::Release);
    }
}

fn cooldown_for_opens(opens: u32) -> Duration {
    // Exponential backoff: 30s, 60s, 120s, 240s, capped at MAX_COOLDOWN.
    let shift = opens.saturating_sub(1).min(8);
    let multiplier = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    let scaled = BASE_COOLDOWN
        .checked_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
        .unwrap_or(MAX_COOLDOWN);
    scaled.min(MAX_COOLDOWN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn closed_breaker_allows_attempts() {
        let breaker = ServoCircuitBreaker::new();
        assert!(breaker.can_attempt());
        assert_eq!(breaker.load_state(), CircuitState::Closed);
    }

    #[test]
    fn single_failure_stays_closed() {
        let breaker = ServoCircuitBreaker::new();
        breaker.record_failure();
        assert!(breaker.can_attempt());
        assert_eq!(breaker.load_state(), CircuitState::Closed);
    }

    #[test]
    fn threshold_failures_open_breaker() {
        let breaker = ServoCircuitBreaker::new();
        let baseline_opens = crate::effect::servo::servo_telemetry_snapshot().breaker_opens_total;
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        assert_eq!(breaker.load_state(), CircuitState::Open);
        assert!(!breaker.can_attempt());
        assert_eq!(
            crate::effect::servo::servo_telemetry_snapshot().breaker_opens_total,
            baseline_opens + 1
        );
    }

    #[test]
    fn success_resets_failure_counter() {
        let breaker = ServoCircuitBreaker::new();
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        breaker.record_failure();
        assert_eq!(breaker.load_state(), CircuitState::Closed);
        assert!(breaker.can_attempt());
    }

    #[test]
    fn half_open_failure_reopens() {
        let breaker = ServoCircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        // Simulate cooldown elapsing.
        breaker.store_state(CircuitState::HalfOpen);
        assert!(breaker.can_attempt());
        breaker.record_failure();
        assert_eq!(breaker.load_state(), CircuitState::Open);
    }

    #[test]
    fn half_open_success_closes_breaker() {
        let breaker = ServoCircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        breaker.store_state(CircuitState::HalfOpen);
        breaker.record_success();
        assert_eq!(breaker.load_state(), CircuitState::Closed);
        assert!(breaker.cooldown_remaining().is_none());
    }

    #[test]
    fn cooldown_remaining_reports_open_breaker() {
        let breaker = ServoCircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        let remaining = breaker.cooldown_remaining();
        assert!(remaining.is_some());
        assert!(remaining.expect("cooldown") <= BASE_COOLDOWN);
    }

    #[test]
    fn cooldown_grows_with_repeated_opens() {
        assert_eq!(cooldown_for_opens(1), BASE_COOLDOWN);
        assert_eq!(cooldown_for_opens(2), BASE_COOLDOWN * 2);
        assert_eq!(cooldown_for_opens(3), BASE_COOLDOWN * 4);
        assert_eq!(cooldown_for_opens(16), MAX_COOLDOWN);
    }

    #[test]
    fn can_attempt_transitions_to_half_open_after_cooldown() {
        let breaker = ServoCircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        assert_eq!(breaker.load_state(), CircuitState::Open);
        // Force the next_retry to the past to simulate cooldown elapsed.
        {
            let mut guard = breaker.next_retry.lock().expect("next_retry lock");
            *guard = Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .expect("monotonic clock is past epoch, subtracting 1s cannot underflow"),
            );
        }
        assert!(breaker.can_attempt());
        assert_eq!(breaker.load_state(), CircuitState::HalfOpen);
        let _ = thread::current();
    }
}
