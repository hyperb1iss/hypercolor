//! Per-device message budget guard (Spec 88 §9.3).
//!
//! The host's latest-value output queue already spaces frames by the
//! cadence the backend reports. This pacer is the second guard inside the
//! device session: it tracks when the budget next allows a frame and makes
//! a write that arrives early wait for that moment. It never buffers frames.

use std::time::{Duration, Instant};

/// Virtual-schedule pacer over a messages-per-second budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePacer {
    per_message: Duration,
    next_allowed_at: Option<Instant>,
}

impl MessagePacer {
    /// Pacer for a budget of `max_messages_per_second` (at least one).
    #[must_use]
    pub fn new(max_messages_per_second: u16) -> Self {
        Self {
            per_message: per_message_interval(max_messages_per_second),
            next_allowed_at: None,
        }
    }

    /// How long a frame arriving at `now` must wait.
    #[must_use]
    pub fn delay_at(&self, now: Instant) -> Duration {
        self.next_allowed_at
            .map_or(Duration::ZERO, |next| next.saturating_duration_since(now))
    }

    /// Record that `messages` datagrams went out at `sent_at`.
    pub fn record(&mut self, sent_at: Instant, messages: u32) {
        let start = self
            .next_allowed_at
            .map_or(sent_at, |next| next.max(sent_at));
        self.next_allowed_at = Some(start + self.per_message.saturating_mul(messages));
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

/// Minimum interval between frames that each cost `messages`.
#[must_use]
pub fn frame_interval(max_messages_per_second: u16, messages: u32) -> Duration {
    per_message_interval(max_messages_per_second).saturating_mul(messages.max(1))
}
