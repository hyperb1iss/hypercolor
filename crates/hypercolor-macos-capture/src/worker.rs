use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
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
    pending_invalidations: Arc<AtomicU64>,
}

pub(crate) struct SamplePublication {
    #[cfg(target_os = "macos")]
    pending_invalidations: Arc<AtomicU64>,
}

struct PendingInvalidation<'a> {
    pending: &'a AtomicU64,
    ready: &'a Condvar,
}

#[derive(Debug)]
struct LatestSampleInner<T> {
    state: Mutex<LatestSampleState<T>>,
    ready: Condvar,
}

#[derive(Debug)]
struct LatestSampleState<T> {
    latest: Option<GenerationStamped<T>>,
    generation: u64,
    closed: bool,
}

#[derive(Debug)]
struct GenerationStamped<T> {
    generation: u64,
    sample: T,
}

pub(crate) struct LatestSampleWorker<T> {
    input: LatestSampleInput<T>,
    worker: Option<JoinHandle<()>>,
}

impl<T> Clone for LatestSampleInput<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            pending_invalidations: Arc::clone(&self.pending_invalidations),
        }
    }
}

impl SamplePublication {
    #[cfg(target_os = "macos")]
    pub(crate) fn is_current(&self) -> bool {
        self.pending_invalidations.load(Ordering::Acquire) == 0
    }
}

impl<'a> PendingInvalidation<'a> {
    fn begin(pending: &'a AtomicU64, ready: &'a Condvar) -> Self {
        pending.fetch_add(1, Ordering::AcqRel);
        Self { pending, ready }
    }
}

impl Drop for PendingInvalidation<'_> {
    fn drop(&mut self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
        self.ready.notify_all();
    }
}

impl<T> LatestSampleInput<T> {
    fn new() -> Self {
        Self {
            inner: Arc::new(LatestSampleInner {
                state: Mutex::new(LatestSampleState {
                    latest: None,
                    generation: 0,
                    closed: false,
                }),
                ready: Condvar::new(),
            }),
            pending_invalidations: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn publish(&self, sample: T) -> SamplePublishOutcome {
        let mut state = self.lock();
        if state.closed {
            return SamplePublishOutcome::Closed;
        }
        let generation = state.generation;
        let outcome = if state
            .latest
            .replace(GenerationStamped { generation, sample })
            .is_some()
        {
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
        Self::advance_generation(&mut state);
        state.closed = true;
        drop(state);
        self.inner.ready.notify_all();
    }

    pub(crate) fn invalidate_if(&self, invalidate: impl FnOnce() -> bool) -> bool {
        self.invalidate_if_with(|| {}, invalidate)
    }

    fn invalidate_if_with(
        &self,
        requested: impl FnOnce(),
        invalidate: impl FnOnce() -> bool,
    ) -> bool {
        let pending = PendingInvalidation::begin(&self.pending_invalidations, &self.inner.ready);
        requested();
        let mut state = self.lock();
        let invalidated = !state.closed && invalidate();
        if invalidated {
            Self::advance_generation(&mut state);
        }
        drop(state);
        drop(pending);
        invalidated
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn invalidate_if_observed(
        &self,
        requested: impl FnOnce(),
        invalidate: impl FnOnce() -> bool,
    ) -> bool {
        self.invalidate_if_with(requested, invalidate)
    }

    pub(crate) fn synchronize_if(&self, synchronize: impl FnOnce() -> bool) -> bool {
        let state = self.lock();
        let state = self
            .inner
            .ready
            .wait_while(state, |_| {
                self.pending_invalidations.load(Ordering::Acquire) != 0
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !state.closed && synchronize()
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    fn take_next(&self) -> Option<GenerationStamped<T>> {
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

    fn publish_if_current(&self, generation: u64, publish: impl FnOnce(SamplePublication)) {
        let state = self.lock();
        if !state.closed && state.generation == generation {
            publish(SamplePublication {
                #[cfg(target_os = "macos")]
                pending_invalidations: Arc::clone(&self.pending_invalidations),
            });
        }
    }

    fn advance_generation(state: &mut LatestSampleState<T>) {
        state.generation = state
            .generation
            .checked_add(1)
            .expect("macOS decode generation must remain monotonic");
        state.latest = None;
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
        mut publish: impl FnMut(O, SamplePublication) + Send + 'static,
    ) -> io::Result<Self> {
        let input = LatestSampleInput::new();
        let worker_input = input.clone();
        let worker = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                while let Some(stamped) = worker_input.take_next() {
                    let decoded = decode(stamped.sample);
                    worker_input.publish_if_current(stamped.generation, |publication| {
                        publish(decoded, publication);
                    });
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

    use super::{LatestSampleInput, LatestSampleWorker, SamplePublishOutcome};

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
            move |sample, _publication| {
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
            move |result, _publication| {
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
    fn blocked_pre_suspend_decode_cannot_publish_after_restart_generation() {
        let (decode_started_tx, decode_started_rx) = mpsc::channel();
        let (release_decode_tx, release_decode_rx) = mpsc::channel();
        let (published_tx, published_rx) = mpsc::channel();
        let mut first = true;
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-generation-fence-test",
            move |sample| {
                if first {
                    first = false;
                    decode_started_tx
                        .send(())
                        .expect("blocked decode should be observable");
                    release_decode_rx.recv().expect("decode should resume");
                }
                sample
            },
            move |sample, _publication| {
                published_tx
                    .send(sample)
                    .expect("current generation should publish");
            },
        )
        .expect("worker should start");
        let input = worker.input();

        assert_eq!(input.publish(1), SamplePublishOutcome::Accepted);
        decode_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should begin decoding the old generation");
        assert!(input.invalidate_if(|| true));
        assert!(input.invalidate_if(|| true));
        release_decode_tx
            .send(())
            .expect("old generation decode should resume");
        assert_eq!(
            published_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        assert_eq!(input.publish(2), SamplePublishOutcome::Accepted);
        assert_eq!(
            published_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("new generation should publish"),
            2
        );
        worker.close();
        worker.join().expect("worker should join");
    }

    #[test]
    fn rejected_invalidation_does_not_advance_the_decode_generation() {
        let (decode_started_tx, decode_started_rx) = mpsc::sync_channel(1);
        let (release_decode_tx, release_decode_rx) = mpsc::sync_channel(1);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-rejected-invalidation-test",
            move |sample| {
                decode_started_tx
                    .send(())
                    .expect("decode should be observable");
                release_decode_rx.recv().expect("decode should resume");
                sample
            },
            move |sample, _publication| {
                published_tx
                    .send(sample)
                    .expect("unchanged generation should publish");
            },
        )
        .expect("worker should start");
        let input = worker.input();

        assert_eq!(input.publish(1), SamplePublishOutcome::Accepted);
        decode_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("decode should start");
        assert!(!input.invalidate_if(|| false));
        release_decode_tx.send(()).expect("decode should resume");
        assert_eq!(
            published_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("rejected invalidation must retain the generation"),
            1
        );

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
            move |_, _publication| {
                let _ = &marker;
            },
        )
        .expect("worker should start");

        worker.close();
        worker.join().expect("idle worker should join");
        assert!(exited.load(Ordering::Acquire));
        assert_eq!(worker.input().publish(()), SamplePublishOutcome::Closed);
    }

    #[test]
    fn publish_wakes_the_worker_past_a_parked_synchronizer() {
        let input: LatestSampleInput<u32> = LatestSampleInput::new();

        // Hold an in-flight invalidation open so a synchronize_if waiter
        // genuinely parks on the shared condvar with its predicate unmet.
        let pending =
            super::PendingInvalidation::begin(&input.pending_invalidations, &input.inner.ready);
        let sync_input = input.clone();
        let synchronizer = thread::spawn(move || {
            sync_input.synchronize_if(|| true);
        });
        // Give the synchronizer time to park before publishing.
        thread::sleep(Duration::from_millis(50));

        let consumer_input = input.clone();
        let consumer = thread::spawn(move || {
            let mut consumed = 0_u32;
            while consumed < 50 {
                if consumer_input.take_next().is_none() {
                    break;
                }
                consumed += 1;
            }
            consumed
        });

        for sample in 0..400_u32 {
            let _ = input.publish(sample);
            thread::sleep(Duration::from_millis(1));
        }

        // A swallowed wakeup leaves the consumer parked beside a full slot;
        // close() unblocks it so a regression fails the count assertion
        // instead of hanging the suite.
        input.close();
        let consumed = consumer.join().expect("consumer thread");
        drop(pending);
        let _ = synchronizer.join();
        assert_eq!(
            consumed, 50,
            "worker must consume past a parked synchronizer"
        );
    }
}
