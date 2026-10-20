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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};

use tokio::sync::Notify;
use tokio::sync::futures::Notified;

/// Wake signal for producers that idle while nothing consumes their output.
///
/// Every event publication notifies it, so notification skips the waiter
/// list entirely unless a producer has armed a wait.
#[derive(Debug, Default)]
pub struct OutputDemandSignal {
    epoch: AtomicU64,
    holds: AtomicUsize,
    armed: AtomicUsize,
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

    /// Arm a wait. Notifications reach waiters only while at least one
    /// [`ArmedOutputDemand`] lives, so arm before re-checking demand.
    pub fn arm(&self) -> ArmedOutputDemand<'_> {
        self.armed.fetch_add(1, Ordering::SeqCst);
        // Orders the arm before every demand read the waiter makes next.
        fence(Ordering::SeqCst);
        ArmedOutputDemand { signal: self }
    }

    /// Wake every current waiter.
    pub fn notify(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        // The state change that prompted this call (a new receiver, a hold,
        // an event) may use weaker orderings, so a full fence orders it
        // before the armed check. Paired with the fence in `arm`, either
        // this load sees the waiter and wakes it, or the waiter's demand
        // read sees the state change.
        fence(Ordering::SeqCst);
        if self.armed.load(Ordering::SeqCst) > 0 {
            self.notify.notify_waiters();
        }
    }

    pub(super) fn acquire_hold(self: &Arc<Self>) -> OutputDemandHold {
        self.holds.fetch_add(1, Ordering::AcqRel);
        self.notify();
        OutputDemandHold {
            signal: Arc::clone(self),
        }
    }
}

/// An armed wait on an [`OutputDemandSignal`].
#[derive(Debug)]
pub struct ArmedOutputDemand<'a> {
    signal: &'a OutputDemandSignal,
}

impl ArmedOutputDemand<'_> {
    /// Future that resolves on the next notification.
    ///
    /// Pin it and call [`Notified::enable`] before re-checking demand: a
    /// notification that lands between the check and the await then still
    /// wakes the waiter.
    pub fn notified(&self) -> Notified<'_> {
        self.signal.notify.notified()
    }
}

impl Drop for ArmedOutputDemand<'_> {
    fn drop(&mut self) {
        self.signal.armed.fetch_sub(1, Ordering::SeqCst);
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
