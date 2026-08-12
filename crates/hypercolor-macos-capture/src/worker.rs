use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SamplePublishOutcome {
    Accepted,
    Superseded,
    Closed,
}

#[derive(Debug)]
pub(crate) struct LatestSampleInput<T> {
    inner: Arc<LatestSampleInner<T>>,
}

#[derive(Debug)]
struct LatestSampleInner<T> {
    state: Mutex<LatestSampleState<T>>,
    ready: Condvar,
}

#[derive(Debug)]
struct LatestSampleState<T> {
    latest: Option<T>,
    closed: bool,
}

pub(crate) struct LatestSampleWorker<T> {
    input: LatestSampleInput<T>,
    worker: Option<JoinHandle<()>>,
}

impl<T> Clone for LatestSampleInput<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> LatestSampleInput<T> {
    fn new() -> Self {
        Self {
            inner: Arc::new(LatestSampleInner {
                state: Mutex::new(LatestSampleState {
                    latest: None,
                    closed: false,
                }),
                ready: Condvar::new(),
            }),
        }
    }

    pub(crate) fn publish(&self, sample: T) -> SamplePublishOutcome {
        let mut state = self.lock();
        if state.closed {
            return SamplePublishOutcome::Closed;
        }
        let outcome = if state.latest.replace(sample).is_some() {
            SamplePublishOutcome::Superseded
        } else {
            SamplePublishOutcome::Accepted
        };
        drop(state);
        self.inner.ready.notify_one();
        outcome
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.latest = None;
        drop(state);
        self.inner.ready.notify_all();
    }

    fn take_next(&self) -> Option<T> {
        let state = self.lock();
        let mut state = self
            .inner
            .ready
            .wait_while(state, |state| state.latest.is_none() && !state.closed)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return None;
        }
        state.latest.take()
    }

    fn lock(&self) -> MutexGuard<'_, LatestSampleState<T>> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<T: Send + 'static> LatestSampleWorker<T> {
    pub(crate) fn spawn<O: Send + 'static>(
        thread_name: &str,
        mut decode: impl FnMut(T) -> O + Send + 'static,
        mut publish: impl FnMut(O) + Send + 'static,
    ) -> io::Result<Self> {
        let input = LatestSampleInput::new();
        let worker_input = input.clone();
        let worker = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                while let Some(sample) = worker_input.take_next() {
                    publish(decode(sample));
                }
            })?;
        Ok(Self {
            input,
            worker: Some(worker),
        })
    }

    pub(crate) fn input(&self) -> LatestSampleInput<T> {
        self.input.clone()
    }

    pub(crate) fn close(&self) {
        self.input.close();
    }

    pub(crate) fn join(&mut self) -> thread::Result<()> {
        self.worker.take().map_or(Ok(()), JoinHandle::join)
    }
}

impl<T> Drop for LatestSampleWorker<T> {
    fn drop(&mut self) {
        self.input.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::{LatestSampleWorker, SamplePublishOutcome};

    #[test]
    fn callback_handoff_stays_bounded_while_decode_is_blocked() {
        let (decode_started_tx, decode_started_rx) = mpsc::channel();
        let (release_decode_tx, release_decode_rx) = mpsc::channel();
        let (published_tx, published_rx) = mpsc::channel();
        let mut first = true;
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-bounded-callback-test",
            move |sample| {
                if first {
                    first = false;
                    decode_started_tx
                        .send(())
                        .expect("decode start should be observable");
                    release_decode_rx.recv().expect("decode should be released");
                }
                sample
            },
            move |sample| {
                published_tx
                    .send(sample)
                    .expect("decoded sample should publish");
            },
        )
        .expect("worker should start");
        let input = worker.input();

        assert_eq!(input.publish(1), SamplePublishOutcome::Accepted);
        decode_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should begin decoding");
        assert_eq!(input.publish(2), SamplePublishOutcome::Accepted);
        assert_eq!(input.publish(3), SamplePublishOutcome::Superseded);
        release_decode_tx
            .send(())
            .expect("blocked decode should resume");

        assert_eq!(
            published_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("first sample should publish"),
            1
        );
        assert_eq!(
            published_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("latest sample should publish"),
            3
        );
        worker.close();
        worker.join().expect("worker should join");
    }

    #[test]
    fn decode_errors_are_emitted_from_the_worker_thread() {
        let caller = thread::current().id();
        let (published_tx, published_rx) = mpsc::channel();
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-decode-error-test",
            move |sample: Result<(), &'static str>| (thread::current().id(), sample),
            move |result| {
                published_tx
                    .send(result)
                    .expect("decode result should publish");
            },
        )
        .expect("worker should start");

        assert_eq!(
            worker.input().publish(Err("malformed frame")),
            SamplePublishOutcome::Accepted
        );
        let (decoder, result) = published_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should emit the decode error");
        assert_ne!(decoder, caller);
        assert_eq!(result, Err("malformed frame"));
        worker.close();
        worker.join().expect("worker should join");
    }

    #[test]
    fn teardown_wakes_and_joins_an_idle_worker() {
        struct ExitMarker(Arc<AtomicBool>);

        impl Drop for ExitMarker {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let exited = Arc::new(AtomicBool::new(false));
        let marker = ExitMarker(Arc::clone(&exited));
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-teardown-test",
            |sample: ()| sample,
            move |_| {
                let _ = &marker;
            },
        )
        .expect("worker should start");

        worker.close();
        worker.join().expect("idle worker should join");
        assert!(exited.load(Ordering::Acquire));
        assert_eq!(worker.input().publish(()), SamplePublishOutcome::Closed);
    }
}
