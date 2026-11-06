//! Drain-progress watchdog for the serial output queue (Spec 91 §7.2).
//!
//! Before each frame the backend asks the OS driver how many bytes it still
//! holds. A backlog of at least one whole frame skips the write so the
//! newest frame goes out once the line catches up. That is ordinary
//! coalescing as long as the queue keeps shrinking. An adapter that
//! accepted bytes and then stopped draining would report the same backlog
//! forever, so the watchdog turns a backlog with no progress for a bounded
//! time into a stall the lifecycle can recover from.

use std::time::{Duration, Instant};

/// What the backend should do with the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainVerdict {
    /// Less than one frame is queued: send.
    Clear,
    /// At least one frame is queued and the queue is still draining (or
    /// has only just backed up): skip this frame.
    Backlogged,
    /// The queue has held at least one frame without shrinking for the
    /// whole budget: fail the delivery.
    Stalled {
        /// Bytes still queued.
        queued: u32,
        /// How long the queue has gone without draining.
        stalled_for: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backlog {
    /// When the queue last shrank, or first backed up.
    since: Instant,
    /// Queue depth at that moment.
    queued: u32,
}

/// Tracks whether a backed-up output queue is still making progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainWatch {
    backlog: Option<Backlog>,
}

impl DrainWatch {
    /// Create a watch with no backlog.
    #[must_use]
    pub const fn new() -> Self {
        Self { backlog: None }
    }

    /// Record a queue-depth reading taken at `now` and decide the frame.
    ///
    /// `frame_bytes` is the size of one frame and `budget` the longest a
    /// backlog may go without shrinking before it counts as stalled.
    pub fn observe(
        &mut self,
        queued: u32,
        frame_bytes: usize,
        budget: Duration,
        now: Instant,
    ) -> DrainVerdict {
        if (queued as usize) < frame_bytes {
            self.backlog = None;
            return DrainVerdict::Clear;
        }
        match self.backlog {
            Some(backlog) if queued >= backlog.queued => {
                let stalled_for = now.saturating_duration_since(backlog.since);
                if stalled_for >= budget {
                    DrainVerdict::Stalled {
                        queued,
                        stalled_for,
                    }
                } else {
                    DrainVerdict::Backlogged
                }
            }
            // First backup, or the queue shrank since the last reading:
            // progress, so restart the clock.
            _ => {
                self.backlog = Some(Backlog { since: now, queued });
                DrainVerdict::Backlogged
            }
        }
    }

    /// Forget any backlog, for example after the queue was flushed.
    pub fn reset(&mut self) {
        self.backlog = None;
    }
}
