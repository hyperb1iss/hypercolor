//! Device lifecycle state machine with reconnect/backoff policy.
//!
//! This module enforces valid device-state transitions and keeps lightweight
//! debug history for reverse-engineering and hardware bring-up workflows.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;

use hypercolor_types::device::{DeviceError, DeviceHandle, DeviceIdentifier, DeviceState};

/// How long a reconnected device must stay up without a communication error
/// before the reconnect counts as a recovery and the backoff starts over.
///
/// A device that fails again sooner reconnected into the same fault, so its
/// next retry keeps growing from the delay that brought it back. A device
/// that is still broken reports its fault within a few frames of connecting,
/// a third of a second even at the lowest 10 fps render tier, so ten seconds
/// leaves an order of magnitude of margin. It also sits well inside the 60 s
/// backoff ceiling, so a device that really recovered returns to fast
/// retries on its next unrelated fault.
pub const RECONNECT_STABLE_AFTER: Duration = Duration::from_secs(10);

/// Unstable reconnects in a row before the device is reported as flapping.
///
/// One reconnect that fails again can be a device still settling after
/// re-enumeration, and two can coincide with a bus hiccup. Three in a row
/// (about 7 s of retries on the default policy) is a persistent fault that
/// reconnecting does not fix, and the user needs to hear about it.
pub const FLAP_ESCALATION_THRESHOLD: u32 = 3;

/// Reconnection backoff configuration.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Initial delay before first retry.
    pub initial_delay: Duration,

    /// Maximum delay between retries.
    pub max_delay: Duration,

    /// Delay multiplier after each failed attempt.
    pub backoff_factor: f64,

    /// Maximum attempts before giving up.
    /// `None` means retry indefinitely.
    pub max_attempts: Option<u32>,

    /// Jitter ratio (0.0-1.0) applied to computed backoff delay.
    pub jitter: f64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_mins(1),
            backoff_factor: 2.0,
            max_attempts: None,
            jitter: 0.1,
        }
    }
}

/// Runtime reconnection status for `DeviceState::Reconnecting`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectStatus {
    /// Timestamp of the last failed attempt.
    pub since: Instant,

    /// Number of failed attempts so far.
    pub attempt: u32,

    /// Delay before the next attempt.
    pub next_retry: Duration,
}

/// One-shot report that a device keeps failing right after reconnecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlapEscalation {
    /// Reconnects in a row that failed again inside [`RECONNECT_STABLE_AFTER`].
    pub flaps: u32,

    /// Delay before the next reconnect attempt.
    pub next_retry: Duration,
}

/// Connection stability carried across reconnects.
#[derive(Debug, Clone, Copy, Default)]
struct FlapTracker {
    /// When the current connection came up; `None` while disconnected.
    connected_at: Option<Instant>,

    /// Retry delay of the reconnect that produced the current connection,
    /// kept until the connection proves stable.
    unproven_delay: Option<Duration>,

    /// Unstable reconnects in a row.
    flaps: u32,

    /// Escalation reached but not yet taken by the executor.
    escalation_pending: bool,
}

/// Recorded state transition for diagnostics.
#[derive(Debug, Clone, Serialize)]
pub struct StateTransitionRecord {
    /// Previous state.
    pub from: String,

    /// New state.
    pub to: String,

    /// Why the transition occurred.
    pub reason: String,
}

/// Lightweight debug snapshot for tooling and API output.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceStateMachineDebugSnapshot {
    /// Device identifier summary.
    pub device: String,

    /// Current lifecycle state.
    pub state: String,

    /// Whether there is an active connection handle.
    pub has_handle: bool,

    /// Active handle ID if connected.
    pub handle_id: Option<u64>,

    /// Reconnect attempt count when in reconnect mode.
    pub reconnect_attempt: Option<u32>,

    /// Delay until next reconnect attempt, if reconnecting.
    pub next_retry_ms: Option<u64>,

    /// Reconnects in a row that failed again before proving stable.
    pub reconnect_flaps: u32,

    /// Number of recorded transition records.
    pub transition_count: usize,

    /// Recent transitions (oldest -> newest).
    pub transitions: Vec<StateTransitionRecord>,
}

/// Manages lifecycle transitions for one physical device.
pub struct DeviceStateMachine {
    state: DeviceState,
    device_id: DeviceIdentifier,
    handle: Option<DeviceHandle>,
    reconnect: Option<ReconnectStatus>,
    reconnect_policy: ReconnectPolicy,
    flap: FlapTracker,
    last_transition: Instant,
    transition_history: VecDeque<StateTransitionRecord>,
    history_limit: usize,
}

impl DeviceStateMachine {
    /// Create a new machine in `Known` state.
    #[must_use]
    pub fn new(device_id: DeviceIdentifier) -> Self {
        Self::with_policy(device_id, ReconnectPolicy::default())
    }

    /// Create a new machine with a custom reconnect policy.
    #[must_use]
    pub fn with_policy(device_id: DeviceIdentifier, reconnect_policy: ReconnectPolicy) -> Self {
        Self {
            state: DeviceState::Known,
            device_id,
            handle: None,
            reconnect: None,
            reconnect_policy,
            flap: FlapTracker::default(),
            last_transition: Instant::now(),
            transition_history: VecDeque::new(),
            history_limit: 64,
        }
    }

    /// Current state.
    #[must_use]
    pub fn state(&self) -> &DeviceState {
        &self.state
    }

    /// Current active handle, if connected/active.
    #[must_use]
    pub fn handle(&self) -> Option<&DeviceHandle> {
        self.handle.as_ref()
    }

    /// Reconnect status, if reconnecting.
    #[must_use]
    pub fn reconnect_status(&self) -> Option<&ReconnectStatus> {
        self.reconnect.as_ref()
    }

    /// Timestamp of the last transition.
    #[must_use]
    pub fn last_transition(&self) -> Instant {
        self.last_transition
    }

    /// Reconnects in a row that failed again before proving stable.
    #[must_use]
    pub fn flap_count(&self) -> u32 {
        self.flap.flaps
    }

    /// Whether the current flap streak has reached [`FLAP_ESCALATION_THRESHOLD`].
    #[must_use]
    pub fn is_flapping(&self) -> bool {
        self.flap.flaps >= FLAP_ESCALATION_THRESHOLD
    }

    /// Take the escalation raised when the flap streak reached
    /// [`FLAP_ESCALATION_THRESHOLD`].
    ///
    /// Returns `Some` once per streak; the streak must end with a stable
    /// connection or a fresh start before another escalation can fire.
    pub fn take_flap_escalation(&mut self) -> Option<FlapEscalation> {
        if !std::mem::take(&mut self.flap.escalation_pending) {
            return None;
        }
        Some(FlapEscalation {
            flaps: self.flap.flaps,
            next_retry: self
                .reconnect
                .as_ref()
                .map_or(self.reconnect_policy.initial_delay, |status| {
                    status.next_retry
                }),
        })
    }

    /// Transition: `Known|Reconnecting -> Connected`.
    ///
    /// A reconnect keeps its retry delay on probation: the backoff starts
    /// over only once the connection outlives [`RECONNECT_STABLE_AFTER`].
    pub fn on_connected(&mut self, handle: DeviceHandle) -> Result<(), DeviceError> {
        self.on_connected_at(handle, Instant::now())
    }

    /// [`Self::on_connected`] with an explicit clock reading.
    pub fn on_connected_at(
        &mut self,
        handle: DeviceHandle,
        now: Instant,
    ) -> Result<(), DeviceError> {
        match self.state {
            DeviceState::Known | DeviceState::Reconnecting => {
                self.handle = Some(handle);
                self.flap.connected_at = Some(now);
                self.flap.unproven_delay = self.reconnect.take().map(|status| status.next_retry);
                self.set_state(DeviceState::Connected, "connect");
                Ok(())
            }
            _ => Err(self.invalid_transition("Connected")),
        }
    }

    /// Transition: `Known|Reconnecting -> Reconnecting` after connect failure.
    ///
    /// Returns the next retry delay.
    pub fn on_connect_failed(&mut self) -> Result<Duration, DeviceError> {
        match self.state {
            DeviceState::Known => {
                self.handle = None;
                let delay = self.reconnect_policy.initial_delay;
                self.reconnect = Some(ReconnectStatus {
                    since: Instant::now(),
                    attempt: 0,
                    next_retry: delay,
                });
                self.set_state(DeviceState::Reconnecting, "connect_failed");
                Ok(delay)
            }
            DeviceState::Reconnecting => {
                if self.reconnect.is_none() {
                    self.reconnect = Some(ReconnectStatus {
                        since: Instant::now(),
                        attempt: 0,
                        next_retry: self.reconnect_policy.initial_delay,
                    });
                }
                Ok(self
                    .reconnect
                    .as_ref()
                    .map_or(self.reconnect_policy.initial_delay, |status| {
                        status.next_retry
                    }))
            }
            _ => Err(self.invalid_transition("Reconnecting")),
        }
    }

    /// Clear reconnect state when a failed connect should not be retried.
    ///
    /// This transitions `Reconnecting -> Known`; `Known` remains a no-op.
    pub fn on_connect_abandoned(&mut self) {
        self.handle = None;
        self.reconnect = None;
        self.flap = FlapTracker::default();
        if self.state == DeviceState::Reconnecting {
            self.set_state(DeviceState::Known, "connect_abandoned");
        }
    }

    /// Transition: `Connected -> Active`. Repeated calls in `Active` are no-op.
    pub fn on_frame_success(&mut self) -> Result<(), DeviceError> {
        match self.state {
            DeviceState::Connected => {
                self.set_state(DeviceState::Active, "first_frame");
                Ok(())
            }
            DeviceState::Active => Ok(()),
            _ => Err(self.invalid_transition("Active")),
        }
    }

    /// Transition: `Connected|Active -> Reconnecting`.
    ///
    /// A connection that failed inside [`RECONNECT_STABLE_AFTER`] of a
    /// reconnect is a flap: the next retry grows from the delay that brought
    /// the device back instead of restarting at the initial delay.
    pub fn on_comm_error(&mut self) -> Result<(), DeviceError> {
        self.on_comm_error_at(Instant::now())
    }

    /// [`Self::on_comm_error`] with an explicit clock reading.
    pub fn on_comm_error_at(&mut self, now: Instant) -> Result<(), DeviceError> {
        match self.state {
            DeviceState::Connected | DeviceState::Active => {
                // An escalation is reported for the failure that raised it;
                // one the caller never took does not outlive that failure.
                self.flap.escalation_pending = false;
                let unstable_reconnect_delay = self.flap.unproven_delay.filter(|_| {
                    self.flap.connected_at.is_some_and(|connected_at| {
                        now.saturating_duration_since(connected_at) < RECONNECT_STABLE_AFTER
                    })
                });
                let next_retry = if let Some(previous) = unstable_reconnect_delay {
                    self.flap.flaps = self.flap.flaps.saturating_add(1);
                    if self.flap.flaps == FLAP_ESCALATION_THRESHOLD {
                        self.flap.escalation_pending = true;
                    }
                    self.grow_delay(previous, self.flap.flaps)
                } else {
                    self.flap = FlapTracker::default();
                    self.reconnect_policy.initial_delay
                };
                self.flap.connected_at = None;
                self.flap.unproven_delay = None;
                self.handle = None;
                self.reconnect = Some(ReconnectStatus {
                    since: now,
                    attempt: 0,
                    next_retry,
                });
                self.set_state(DeviceState::Reconnecting, "comm_error");
                Ok(())
            }
            _ => Err(self.invalid_transition("Reconnecting")),
        }
    }

    /// Advance reconnect attempt state.
    ///
    /// Returns the next retry delay, or `None` if max attempts are exhausted
    /// and the machine falls back to `Known`.
    pub fn on_reconnect_failed(&mut self) -> Option<Duration> {
        let reconnect = self.reconnect.as_mut()?;

        reconnect.attempt = reconnect.attempt.saturating_add(1);
        reconnect.since = Instant::now();

        if self
            .reconnect_policy
            .max_attempts
            .is_some_and(|max| reconnect.attempt >= max)
        {
            self.handle = None;
            self.reconnect = None;
            self.flap = FlapTracker::default();
            self.set_state(DeviceState::Known, "reconnect_exhausted");
            return None;
        }

        let (current, attempt) = (reconnect.next_retry, reconnect.attempt);
        let next = self.grow_delay(current, attempt);
        if let Some(reconnect) = self.reconnect.as_mut() {
            reconnect.next_retry = next;
        }

        Some(next)
    }

    /// Apply one backoff step to `current`, capped and jittered.
    fn grow_delay(&self, current: Duration, step: u32) -> Duration {
        let base_secs = (current.as_secs_f64() * self.reconnect_policy.backoff_factor)
            .min(self.reconnect_policy.max_delay.as_secs_f64());

        // Deterministic +/- jitter keeps retries spread without requiring RNG.
        let centered = if step.is_multiple_of(2) { 1.0 } else { -1.0 };
        let jitter = centered * self.reconnect_policy.jitter;

        Duration::from_secs_f64((base_secs * (1.0 + jitter)).max(0.1))
    }

    /// Transition to `Disabled` from any state.
    pub fn on_user_disable(&mut self) {
        self.handle = None;
        self.reconnect = None;
        self.flap = FlapTracker::default();
        self.set_state(DeviceState::Disabled, "user_disable");
    }

    /// Transition `Disabled -> Known`.
    pub fn on_user_enable(&mut self) {
        if self.state == DeviceState::Disabled {
            self.set_state(DeviceState::Known, "user_enable");
        }
    }

    /// Transition to `Known` after hot-unplug or teardown.
    pub fn on_hot_unplug(&mut self) {
        self.handle = None;
        self.reconnect = None;
        self.flap = FlapTracker::default();
        self.set_state(DeviceState::Known, "hot_unplug");
    }

    /// Build a debug snapshot for tooling/API use.
    #[must_use]
    pub fn debug_snapshot(&self) -> DeviceStateMachineDebugSnapshot {
        DeviceStateMachineDebugSnapshot {
            device: self.device_id.display_short(),
            state: self.state.variant_name().to_owned(),
            has_handle: self.handle.is_some(),
            handle_id: self.handle.as_ref().map(DeviceHandle::id),
            reconnect_attempt: self.reconnect.as_ref().map(|r| r.attempt),
            next_retry_ms: self.reconnect.as_ref().map(|r| {
                let ms = r.next_retry.as_millis();
                u64::try_from(ms).unwrap_or(u64::MAX)
            }),
            reconnect_flaps: self.flap.flaps,
            transition_count: self.transition_history.len(),
            transitions: self.transition_history.iter().cloned().collect(),
        }
    }

    fn set_state(&mut self, next: DeviceState, reason: &str) {
        let from = self.state.variant_name().to_owned();
        let to = next.variant_name().to_owned();
        self.state = next;
        self.last_transition = Instant::now();

        if self.transition_history.len() >= self.history_limit {
            self.transition_history.pop_front();
        }
        self.transition_history.push_back(StateTransitionRecord {
            from,
            to,
            reason: reason.to_owned(),
        });
    }

    fn invalid_transition(&self, to: &str) -> DeviceError {
        DeviceError::InvalidTransition {
            device: self.device_id.display_short(),
            from: self.state.variant_name().to_owned(),
            to: to.to_owned(),
        }
    }
}
