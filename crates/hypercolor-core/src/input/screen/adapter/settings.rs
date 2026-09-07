use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, mpsc};
use std::time::Duration;

use super::super::ScreenCaptureDemand;

pub(in crate::input::screen) struct VersionedCaptureSettings<C> {
    config: Mutex<C>,
    demand: Mutex<ScreenCaptureDemand>,
    revision: AtomicU64,
}

pub(in crate::input::screen) struct CaptureSettingsSnapshot<C> {
    pub(in crate::input::screen) config: C,
    pub(in crate::input::screen) demand: ScreenCaptureDemand,
}

pub(in crate::input::screen) struct CaptureSettingsGuard<'a, C> {
    config: MutexGuard<'a, C>,
    demand: MutexGuard<'a, ScreenCaptureDemand>,
    revision: &'a AtomicU64,
}

impl<C> VersionedCaptureSettings<C> {
    pub(in crate::input::screen) fn new(config: C, demand: ScreenCaptureDemand) -> Self {
        Self {
            config: Mutex::new(config),
            demand: Mutex::new(demand),
            revision: AtomicU64::new(0),
        }
    }

    pub(in crate::input::screen) fn snapshot(&self) -> CaptureSettingsSnapshot<C>
    where
        C: Clone,
    {
        let values = self.lock();
        CaptureSettingsSnapshot {
            config: values.config.clone(),
            demand: *values.demand,
        }
    }

    pub(in crate::input::screen) fn demand(&self) -> ScreenCaptureDemand {
        *self
            .demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(in crate::input::screen) fn lock(&self) -> CaptureSettingsGuard<'_, C> {
        CaptureSettingsGuard {
            config: self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            demand: self
                .demand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            revision: &self.revision,
        }
    }

    pub(in crate::input::screen) fn lock_config(&self) -> MutexGuard<'_, C> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(in crate::input::screen) fn lock_demand(&self) -> MutexGuard<'_, ScreenCaptureDemand> {
        self.demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(in crate::input::screen) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(in crate::input::screen) fn bump_revision(&self) -> u64 {
        self.revision
            .fetch_add(1, Ordering::Release)
            .wrapping_add(1)
    }

    pub(in crate::input::screen) fn commit_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}

impl<C> CaptureSettingsGuard<'_, C> {
    pub(in crate::input::screen) fn config(&self) -> &C {
        &self.config
    }

    pub(in crate::input::screen) fn config_mut(&mut self) -> &mut C {
        &mut self.config
    }

    pub(in crate::input::screen) fn demand_mut(&mut self) -> &mut ScreenCaptureDemand {
        &mut self.demand
    }

    pub(in crate::input::screen) fn commit(self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}

/// The only decision a source can hand a worker mid-adoption.
///
/// Cancellation is expressed by dropping the adopter (the worker observes a
/// closed decision channel), never by a second variant, so a worker that
/// passed the rendezvous has exactly one path: commit.
pub(in crate::input::screen) enum CaptureSettingsDecision {
    Commit,
}

/// Worker half of a two-phase settings adoption.
///
/// The worker announces readiness, waits for the source's decision, and only
/// then applies `prepared` and reports through `done`.
pub(in crate::input::screen) struct CaptureSettingsAdoption<P, D = ()> {
    prepared: P,
    ready: mpsc::SyncSender<()>,
    decision: mpsc::Receiver<CaptureSettingsDecision>,
    done: mpsc::SyncSender<D>,
}

/// Source half of a two-phase settings adoption.
pub(in crate::input::screen) struct CaptureSettingsAdopter<D = ()> {
    ready: mpsc::Receiver<()>,
    decision: mpsc::SyncSender<CaptureSettingsDecision>,
    done: mpsc::Receiver<D>,
}

/// A rendezvous the source committed; the worker now owns `prepared`.
pub(in crate::input::screen) struct CommittedCaptureSettingsAdoption<P, D = ()> {
    pub(in crate::input::screen) prepared: P,
    done: mpsc::SyncSender<D>,
}

/// Opens one adoption: the worker half travels in a command, the adopter
/// stays with the source.
pub(in crate::input::screen) fn begin_capture_settings_adoption<P, D>(
    prepared: P,
) -> (CaptureSettingsAdoption<P, D>, CaptureSettingsAdopter<D>) {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (decision_tx, decision_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    (
        CaptureSettingsAdoption {
            prepared,
            ready: ready_tx,
            decision: decision_rx,
            done: done_tx,
        },
        CaptureSettingsAdopter {
            ready: ready_rx,
            decision: decision_tx,
            done: done_rx,
        },
    )
}

impl<P, D> CaptureSettingsAdoption<P, D> {
    /// Announces readiness and waits for the commit decision.
    ///
    /// Returns `None` when the source went away or declined (dropped its
    /// adopter, or the decision did not arrive within `decision_timeout`),
    /// in which case `prepared` is discarded unapplied.
    pub(in crate::input::screen) fn rendezvous(
        self,
        decision_timeout: Option<Duration>,
    ) -> Option<CommittedCaptureSettingsAdoption<P, D>> {
        if self.ready.send(()).is_err() {
            return None;
        }
        let decision = match decision_timeout {
            Some(timeout) => self.decision.recv_timeout(timeout).ok(),
            None => self.decision.recv().ok(),
        };
        match decision? {
            CaptureSettingsDecision::Commit => Some(CommittedCaptureSettingsAdoption {
                prepared: self.prepared,
                done: self.done,
            }),
        }
    }
}

impl<P, D> CommittedCaptureSettingsAdoption<P, D> {
    /// Splits the prepared payload from the completion sender. Workers apply
    /// the payload and report through the sender; a source that stopped
    /// waiting simply drops the report.
    pub(in crate::input::screen) fn into_parts(self) -> (P, mpsc::SyncSender<D>) {
        (self.prepared, self.done)
    }
}

impl<D> CaptureSettingsAdopter<D> {
    /// Waits for the worker to reach the rendezvous.
    ///
    /// # Errors
    ///
    /// Returns the receive error when the worker exits first or `timeout`
    /// elapses.
    pub(in crate::input::screen) fn wait_ready(
        &self,
        timeout: Duration,
    ) -> Result<(), mpsc::RecvTimeoutError> {
        self.ready.recv_timeout(timeout)
    }

    /// Tells the worker to apply the prepared settings; `false` when the
    /// worker already left.
    pub(in crate::input::screen) fn commit(&self) -> bool {
        self.decision.send(CaptureSettingsDecision::Commit).is_ok()
    }

    /// The completion channel, for sources that wait with their own policy.
    pub(in crate::input::screen) const fn done(&self) -> &mpsc::Receiver<D> {
        &self.done
    }

    /// Blocks until the worker reports the outcome.
    ///
    /// # Errors
    ///
    /// Returns the receive error when the worker exits before reporting.
    pub(in crate::input::screen) fn wait_done(&self) -> Result<D, mpsc::RecvError> {
        self.done.recv()
    }
}
