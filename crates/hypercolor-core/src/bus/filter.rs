//! Event filtering — selective subscription to the broadcast channel.
//!
//! [`EventFilter`] defines which events a subscriber cares about.
//! [`FilteredEventReceiver`] wraps a broadcast receiver and silently
//! discards events that don't match the filter.

use tokio::sync::broadcast;

use crate::types::event::{EventCategory, EventPriority};

use super::TimestampedEvent;

// ── EventFilter ──────────────────────────────────────────────────────────

/// Filter for selective event subscription.
///
/// All fields are optional. `None` matches everything.
/// Multiple fields combine with AND logic.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only receive events in these categories.
    pub categories: Option<Vec<EventCategory>>,

    /// Minimum priority level. Events below this priority are dropped.
    pub min_priority: Option<EventPriority>,
}

impl EventFilter {
    /// Create an empty filter that matches all events.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Only receive events in the specified categories.
    #[must_use]
    pub fn categories(mut self, cats: Vec<EventCategory>) -> Self {
        self.categories = Some(cats);
        self
    }

    /// Only receive events at or above the specified priority.
    #[must_use]
    pub fn min_priority(mut self, priority: EventPriority) -> Self {
        self.min_priority = Some(priority);
        self
    }

    /// Test whether an event passes this filter.
    #[must_use]
    pub fn matches(&self, event: &TimestampedEvent) -> bool {
        // Category filter
        if let Some(ref cats) = self.categories
            && !cats.contains(&event.event.category())
        {
            return false;
        }

        // Priority filter
        if let Some(min) = self.min_priority
            && event.event.priority() < min
        {
            return false;
        }

        true
    }
}

// ── FilteredEventReceiver ────────────────────────────────────────────────

/// A broadcast receiver that applies an [`EventFilter`] to incoming events.
///
/// Non-matching events are silently consumed and discarded. Lagged errors
/// are propagated to the caller so they can request a state snapshot.
pub struct FilteredEventReceiver {
    inner: broadcast::Receiver<TimestampedEvent>,
    filter: EventFilter,
}

impl FilteredEventReceiver {
    /// Wrap a broadcast receiver with a filter.
    pub(super) fn new(inner: broadcast::Receiver<TimestampedEvent>, filter: EventFilter) -> Self {
        Self { inner, filter }
    }

    /// Receive the next event that passes the filter.
    ///
    /// Blocks until a matching event arrives, the channel closes, or
    /// the receiver lags behind the sender.
    ///
    /// # Errors
    ///
    /// Returns `RecvError::Lagged(n)` if the receiver fell behind by `n`
    /// events, or `RecvError::Closed` if the bus has been dropped.
    pub async fn recv(&mut self) -> Result<TimestampedEvent, broadcast::error::RecvError> {
        loop {
            let event = self.inner.recv().await?;
            if self.filter.matches(&event) {
                return Ok(event);
            }
            // Non-matching events are silently consumed.
        }
    }
}
