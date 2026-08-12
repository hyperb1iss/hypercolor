use std::sync::{Arc, Mutex, MutexGuard};

use crate::{MacosCaptureError, MacosFrameEvent};

#[derive(Debug, Clone, Default)]
pub struct MacosFrameMailbox {
    state: Arc<Mutex<MailboxState>>,
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

    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
