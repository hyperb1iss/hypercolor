//! Output-demand signalling for the latest-value bus lanes.
//!
//! A producer that publishes onto watch lanes can stop producing while
//! nothing consumes its output, but watch channels never announce a new
//! subscriber. [`OutputDemandSignal`] closes that gap: every lane
//! subscription, every [`OutputDemandHold`], and every discrete event
//! published on the bus notifies it, so an idle producer can sleep on the
//! signal and re-evaluate the moment a consumer appears or observable state
//! changes, instead of polling for either.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::Notify;
use tokio::sync::futures::Notified;

/// Wake signal for producers that idle while nothing consumes their output.
#[derive(Debug, Default)]
pub struct OutputDemandSignal {
    epoch: AtomicU64,
    holds: AtomicUsize,
    notify: Notify,
}

impl OutputDemandSignal {
    /// Monotonic count of notifications, for observing that one happened.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Number of live [`OutputDemandHold`]s.
    #[must_use]
    pub fn hold_count(&self) -> usize {
        self.holds.load(Ordering::Acquire)
    }

    /// Future that resolves on the next notification.
    ///
    /// Pin it and call [`Notified::enable`] before re-checking demand: a
    /// notification that lands between the check and the await then still
    /// wakes the waiter.
    pub fn notified(&self) -> Notified<'_> {
        self.notify.notified()
    }

    /// Wake every current waiter.
    pub fn notify(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.notify.notify_waiters();
    }

    pub(super) fn acquire_hold(self: &Arc<Self>) -> OutputDemandHold {
        self.holds.fetch_add(1, Ordering::AcqRel);
        self.notify();
        OutputDemandHold {
            signal: Arc::clone(self),
        }
    }
}

/// Keeps output demand registered for consumers that read the bus without
/// holding a lane receiver, such as a borrow of a per-zone sender.
///
/// Dropping the hold withdraws the demand. Release needs no notification:
/// a producer only loses demand on its own next evaluation.
#[derive(Debug)]
#[must_use = "dropping the hold withdraws the output demand it registers"]
pub struct OutputDemandHold {
    signal: Arc<OutputDemandSignal>,
}

impl Drop for OutputDemandHold {
    fn drop(&mut self) {
        let previous = self.signal.holds.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "output demand hold underflow");
    }
}
