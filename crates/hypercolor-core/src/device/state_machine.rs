//! Device lifecycle state machine with reconnect/backoff policy.
//!
//! This module enforces valid device-state transitions and keeps lightweight
//! debug history for reverse-engineering and hardware bring-up workflows.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::types::device::{DeviceError, DeviceHandle, DeviceIdentifier, DeviceState};

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

    /// Transition: `Known|Reconnecting -> Connected`.
    pub fn on_connected(&mut self, handle: DeviceHandle) -> Result<(), DeviceError> {
        match self.state {
            DeviceState::Known | DeviceState::Reconnecting => {
                self.handle = Some(handle);
                self.reconnect = None;
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
    pub fn on_comm_error(&mut self) -> Result<(), DeviceError> {
        match self.state {
            DeviceState::Connected | DeviceState::Active => {
                self.handle = None;
                self.reconnect = Some(ReconnectStatus {
                    since: Instant::now(),
                    attempt: 0,
                    next_retry: self.reconnect_policy.initial_delay,
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
            self.set_state(DeviceState::Known, "reconnect_exhausted");
            return None;
        }

        let current_secs = reconnect.next_retry.as_secs_f64();
        let base_secs = (current_secs * self.reconnect_policy.backoff_factor)
            .min(self.reconnect_policy.max_delay.as_secs_f64());

        // Deterministic +/- jitter keeps retries spread without requiring RNG.
        let centered = if reconnect.attempt % 2 == 0 {
            1.0
        } else {
            -1.0
        };
        let jitter = centered * self.reconnect_policy.jitter;

        let jittered_secs = (base_secs * (1.0 + jitter)).max(0.1);
        let next = Duration::from_secs_f64(jittered_secs);
        reconnect.next_retry = next;

        Some(next)
    }

    /// Transition to `Disabled` from any state.
    pub fn on_user_disable(&mut self) {
        self.handle = None;
        self.reconnect = None;
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
