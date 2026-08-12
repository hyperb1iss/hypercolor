use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::{MacosCaptureError, MacosFrameEvent};

#[derive(Debug, Clone, Default)]
pub struct MacosFrameMailbox {
    inner: Arc<MailboxInner>,
}

#[derive(Debug, Default)]
struct MailboxInner {
    state: Mutex<MailboxState>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct MailboxState {
    latest: Option<Result<MacosFrameEvent, MacosCaptureError>>,
    superseded: u64,
}

impl MacosFrameMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, delivery: Result<MacosFrameEvent, MacosCaptureError>) {
        let mut state = self.lock();
        if state.latest.replace(delivery).is_some() {
            state.superseded = state.superseded.saturating_add(1);
        }
        drop(state);
        self.inner.ready.notify_one();
    }

    pub fn take_latest(&self) -> Option<Result<MacosFrameEvent, MacosCaptureError>> {
        self.lock().latest.take()
    }

    pub fn has_pending(&self) -> bool {
        self.lock().latest.is_some()
    }

    pub fn superseded_count(&self) -> u64 {
        self.lock().superseded
    }

    pub fn wait_latest(
        &self,
        timeout: Duration,
    ) -> Option<Result<MacosFrameEvent, MacosCaptureError>> {
        let state = self.lock();
        let mut state = self
            .inner
            .ready
            .wait_timeout_while(state, timeout, |state| state.latest.is_none())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        state.latest.take()
    }

    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
