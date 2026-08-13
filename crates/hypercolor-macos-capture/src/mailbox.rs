use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
    wake_generation: u64,
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
        self.wait_latest_while(timeout, || true)
    }

    pub fn wait_latest_while(
        &self,
        timeout: Duration,
        keep_waiting: impl Fn() -> bool,
    ) -> Option<Result<MacosFrameEvent, MacosCaptureError>> {
        self.wait_latest_while_with_hook(timeout, keep_waiting, || {})
    }

    fn wait_latest_while_with_hook(
        &self,
        timeout: Duration,
        keep_waiting: impl Fn() -> bool,
        mut before_wait: impl FnMut(),
    ) -> Option<Result<MacosFrameEvent, MacosCaptureError>> {
        let started = Instant::now();
        let mut state = self.lock();
        let wake_generation = state.wake_generation;
        while state.latest.is_none() && state.wake_generation == wake_generation && keep_waiting() {
            before_wait();
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let (next, timeout_result) = self
                .inner
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if timeout_result.timed_out() {
                break;
            }
        }
        state.latest.take()
    }

    pub fn wake(&self) {
        let mut state = self.lock();
        state.wake_generation = state.wake_generation.wrapping_add(1);
        drop(state);
        self.inner.ready.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::MacosFrameMailbox;

    #[test]
    fn wake_generation_closes_the_post_predicate_wait_window() {
        let mailbox = MacosFrameMailbox::new();
        let worker_mailbox = mailbox.clone();
        let (predicate_tx, predicate_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut paused = false;
            let delivery = worker_mailbox.wait_latest_while_with_hook(
                Duration::from_secs(5),
                || true,
                || {
                    if !paused {
                        paused = true;
                        predicate_tx
                            .send(())
                            .expect("post-predicate pause should be observable");
                        resume_rx
                            .recv()
                            .expect("condition wait setup should resume");
                    }
                },
            );
            done_tx
                .send(delivery.is_none())
                .expect("wait result should be observable");
        });

        predicate_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter should pause after its external predicate returns true");
        let wake_mailbox = mailbox.clone();
        let (wake_done_tx, wake_done_rx) = mpsc::channel();
        let waker = std::thread::spawn(move || {
            wake_mailbox.wake();
            wake_done_tx.send(()).expect("wake should finish");
        });
        assert_eq!(
            wake_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        resume_tx
            .send(())
            .expect("condition wait setup should resume");
        wake_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("wake should advance the generation after acquiring the mailbox lock");
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("generation change should release the waiter immediately")
        );
        waker.join().expect("waker should join");
        worker.join().expect("waiter should join");
    }
}
