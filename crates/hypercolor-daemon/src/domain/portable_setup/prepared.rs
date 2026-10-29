//! A store replacement retains its ordinary write guard until publication.

use tokio::sync::OwnedRwLockWriteGuard;

use crate::persistence::{AtomicFileWriter, AtomicWriteReservation};
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

pub(crate) struct PreparedStoreReplacement<T> {
    guard: OwnedRwLockWriteGuard<T>,
    candidate: Option<T>,
    reservation: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
}

impl<T> PreparedStoreReplacement<T> {
    pub(crate) fn new(
        guard: OwnedRwLockWriteGuard<T>,
        candidate: T,
        writer: &AtomicFileWriter,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            guard,
            candidate: Some(candidate),
            reservation: Some(writer.reserve()),
            payload,
            durable: false,
        }
    }

    pub(crate) fn candidate(&self) -> &T {
        self.candidate
            .as_ref()
            .expect("unpublished store candidate")
    }

    pub(crate) fn persist(&mut self, shutdown: &PersistenceShutdown) -> AtomicWriteWaitOutcome {
        if shutdown.is_shutdown() {
            return AtomicWriteWaitOutcome::ShutdownPending;
        }
        let outcome = self
            .reservation
            .take()
            .expect("store persisted once")
            .admit(std::mem::take(&mut self.payload))
            .commit_and_wait_durable_or_shutdown(shutdown);
        self.durable = matches!(outcome, AtomicWriteWaitOutcome::Written);
        outcome
    }

    pub(crate) fn publish(&mut self) {
        assert!(self.durable, "store candidate durable before publication");
        *self.guard = self.candidate.take().expect("store published once");
    }
}
