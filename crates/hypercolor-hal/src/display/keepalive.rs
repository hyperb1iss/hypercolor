//! Interval tracking for wire-level display keepalives.

use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Tracks when a protocol last sent a wire keepalive.
///
/// Display panels that idle out of direct mode need periodic traffic that is
/// separate from frame data. This type only answers "is one due"; the
/// protocol decides what the keepalive command is and where it goes in the
/// command stream.
#[derive(Debug)]
pub struct WireKeepalive {
    interval: Duration,
    last_sent_at: RwLock<Option<Instant>>,
}

impl WireKeepalive {
    /// Track keepalives spaced at least `interval` apart.
    #[must_use]
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_sent_at: RwLock::new(None),
        }
    }

    /// Whether a keepalive is due: none has been sent, or the interval has
    /// elapsed since the last one.
    #[must_use]
    pub fn due(&self) -> bool {
        self.last_sent_at
            .read()
            .expect("wire keepalive lock should not be poisoned")
            .is_none_or(|last| last.elapsed() >= self.interval)
    }

    /// Record that a keepalive has just been queued for the wire.
    pub fn mark_sent(&self) {
        *self
            .last_sent_at
            .write()
            .expect("wire keepalive lock should not be poisoned") = Some(Instant::now());
    }
}
