use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{MacosCaptureError, MacosFrameEvent, MacosFrameStatus};

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
    control: Option<PendingDelivery>,
    frame: Option<PendingDelivery>,
    diagnostic: Option<PendingDelivery>,
    superseded: u64,
    wake_generation: u64,
    delivery_revision: u64,
    invalidation_generation: u64,
}

#[derive(Debug)]
struct PendingDelivery {
    revision: u64,
    invalidation_generation: u64,
    delivery: Result<MacosFrameEvent, MacosCaptureError>,
}

impl MacosFrameMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, delivery: Result<MacosFrameEvent, MacosCaptureError>) {
        let mut state = self.lock();
        state.delivery_revision = state
            .delivery_revision
            .checked_add(1)
            .expect("macOS mailbox delivery revision must remain monotonic");
        let terminal = matches!(
            &delivery,
            Err(_)
                | Ok(MacosFrameEvent::Lifecycle(
                    MacosFrameStatus::Suspended | MacosFrameStatus::Stopped
                ))
        );
        if terminal {
            state.invalidation_generation = state
                .invalidation_generation
                .checked_add(1)
                .expect("macOS mailbox invalidation generation must remain monotonic");
        }
        let pending = PendingDelivery {
            revision: state.delivery_revision,
            invalidation_generation: state.invalidation_generation,
            delivery,
        };
        let lane = match &pending.delivery {
            Ok(MacosFrameEvent::Frame(_)) => &mut state.frame,
            Ok(MacosFrameEvent::RecoverableError(_)) => &mut state.diagnostic,
            Ok(MacosFrameEvent::Lifecycle(_)) | Err(_) => &mut state.control,
        };
        if lane.replace(pending).is_some() {
            state.superseded = state.superseded.saturating_add(1);
        }
        drop(state);
        self.inner.ready.notify_one();
    }

    pub fn take_latest(&self) -> Option<Result<MacosFrameEvent, MacosCaptureError>> {
        Self::take_next(&mut self.lock()).map(|(_, _, delivery)| delivery)
    }

    pub fn take_latest_with_generation(
        &self,
    ) -> Option<(u64, u64, Result<MacosFrameEvent, MacosCaptureError>)> {
        Self::take_next(&mut self.lock())
    }

    pub fn has_pending(&self) -> bool {
        Self::has_pending_state(&self.lock())
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
        self.wait_latest_with_generation_while(timeout, keep_waiting)
            .map(|(_, _, delivery)| delivery)
    }

    pub fn wait_latest_with_generation_while(
        &self,
        timeout: Duration,
        keep_waiting: impl Fn() -> bool,
    ) -> Option<(u64, u64, Result<MacosFrameEvent, MacosCaptureError>)> {
        self.wait_latest_while_with_hook(timeout, keep_waiting, || {})
    }

    fn wait_latest_while_with_hook(
        &self,
        timeout: Duration,
        keep_waiting: impl Fn() -> bool,
        mut before_wait: impl FnMut(),
    ) -> Option<(u64, u64, Result<MacosFrameEvent, MacosCaptureError>)> {
        let started = Instant::now();
        let mut state = self.lock();
        let wake_generation = state.wake_generation;
        while !Self::has_pending_state(&state)
            && state.wake_generation == wake_generation
            && keep_waiting()
        {
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
        Self::take_next(&mut state)
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

    fn has_pending_state(state: &MailboxState) -> bool {
        state.control.is_some() || state.frame.is_some() || state.diagnostic.is_some()
    }

    fn take_next(
        state: &mut MailboxState,
    ) -> Option<(u64, u64, Result<MacosFrameEvent, MacosCaptureError>)> {
        state
            .control
            .take()
            .or_else(|| match (&state.frame, &state.diagnostic) {
                (Some(frame), Some(diagnostic)) if diagnostic.revision < frame.revision => {
                    state.diagnostic.take()
                }
                (Some(_), _) => state.frame.take(),
                (None, Some(_)) => state.diagnostic.take(),
                (None, None) => None,
            })
            .map(|pending| {
                (
                    pending.revision,
                    pending.invalidation_generation,
                    pending.delivery,
                )
            })
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
