//! Per-device message budget (Spec 88 §9).
//!
//! LIFX's guideline is a message rate, so every datagram is charged against
//! the budget as it is sent, whatever frame it belongs to. The budget is a
//! token bucket in its virtual-scheduling form (GCRA): each message pushes a
//! theoretical arrival time forward by one message interval, and a message
//! may go out once that time, less a small burst tolerance, has arrived. In
//! any window of length `L` at most `burst + L / interval` messages leave,
//! and over time the rate converges on the budget.

use std::time::{Duration, Instant};

/// Messages that may leave back to back before pacing applies: one full
/// frame of a five-tile chain, so every tile changes in the same instant.
pub const DEFAULT_BURST_MESSAGES: u32 = 5;

/// Token bucket over a messages-per-second budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageBudget {
    per_message: Duration,
    tolerance: Duration,
    theoretical_arrival: Option<Instant>,
}

impl MessageBudget {
    /// Budget of `max_messages_per_second` with the default burst, capped at
    /// one second's worth of messages.
    #[must_use]
    pub fn new(max_messages_per_second: u16) -> Self {
        let burst = DEFAULT_BURST_MESSAGES.min(u32::from(max_messages_per_second.max(1)));
        Self::with_burst(max_messages_per_second, burst)
    }

    /// Budget with an explicit burst allowance (at least one message).
    #[must_use]
    pub fn with_burst(max_messages_per_second: u16, burst: u32) -> Self {
        let per_message = per_message_interval(max_messages_per_second);
        Self {
            per_message,
            tolerance: per_message.saturating_mul(burst.max(1) - 1),
            theoretical_arrival: None,
        }
    }

    /// How long the next message must wait at `now`.
    #[must_use]
    pub fn delay_at(&self, now: Instant) -> Duration {
        self.theoretical_arrival
            .and_then(|arrival| arrival.checked_sub(self.tolerance))
            .map_or(Duration::ZERO, |allowed| {
                allowed.saturating_duration_since(now)
            })
    }

    /// Whether a message may leave at `now`.
    #[must_use]
    pub fn ready_at(&self, now: Instant) -> bool {
        self.delay_at(now).is_zero()
    }

    /// Charge one message sent at `sent_at`.
    pub fn record(&mut self, sent_at: Instant) {
        let base = self
            .theoretical_arrival
            .map_or(sent_at, |arrival| arrival.max(sent_at));
        self.theoretical_arrival = Some(base + self.per_message);
    }

    /// Budget spacing for one message.
    #[must_use]
    pub const fn per_message(&self) -> Duration {
        self.per_message
    }
}

/// Time one message costs under a budget.
#[must_use]
pub fn per_message_interval(max_messages_per_second: u16) -> Duration {
    Duration::from_secs(1) / u32::from(max_messages_per_second.max(1))
}

/// Time a frame of `messages` datagrams costs under a budget.
#[must_use]
pub fn frame_interval(max_messages_per_second: u16, messages: u32) -> Duration {
    per_message_interval(max_messages_per_second).saturating_mul(messages.max(1))
}
