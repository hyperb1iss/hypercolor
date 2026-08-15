use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};

use super::transactions::{DeadlineScheduler, DeadlineTicket};

const COMPLETION_OPEN: u8 = 0;
const COMPLETION_INVOKED: u8 = 1;
const COMPLETION_DESTROYED: u8 = 2;
const STOP_PENDING: u8 = 0;
const STOP_TIMED_OUT: u8 = 1;
const STOP_COMPLETED: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompletionDisposition {
    Invoked,
    Destroyed,
}

type CompletionObserver = Box<dyn FnOnce(CompletionDisposition) + Send + 'static>;

struct CompletionFenceInner {
    disposition: AtomicU8,
    observers: Mutex<Vec<CompletionObserver>>,
}

#[derive(Clone)]
pub(super) struct CompletionFence {
    inner: Arc<CompletionFenceInner>,
}

pub(super) struct CompletionWitness {
    fence: CompletionFence,
}

impl CompletionFence {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(CompletionFenceInner {
                disposition: AtomicU8::new(COMPLETION_OPEN),
                observers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(super) fn witness(&self) -> CompletionWitness {
        CompletionWitness {
            fence: self.clone(),
        }
    }

    pub(super) fn observe(&self, observer: impl FnOnce(CompletionDisposition) + Send + 'static) {
        let disposition = self.disposition();
        if let Some(disposition) = disposition {
            observer(disposition);
            return;
        }
        let mut observers = lock(&self.inner.observers);
        if let Some(disposition) = self.disposition() {
            drop(observers);
            observer(disposition);
        } else {
            observers.push(Box::new(observer));
        }
    }

    fn disposition(&self) -> Option<CompletionDisposition> {
        match self.inner.disposition.load(Ordering::Acquire) {
            COMPLETION_OPEN => None,
            COMPLETION_INVOKED => Some(CompletionDisposition::Invoked),
            COMPLETION_DESTROYED => Some(CompletionDisposition::Destroyed),
            value => unreachable!("invalid native completion disposition {value}"),
        }
    }

    fn settle(&self, disposition: CompletionDisposition) -> bool {
        let value = match disposition {
            CompletionDisposition::Invoked => COMPLETION_INVOKED,
            CompletionDisposition::Destroyed => COMPLETION_DESTROYED,
        };
        if self
            .inner
            .disposition
            .compare_exchange(COMPLETION_OPEN, value, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let observers = std::mem::take(&mut *lock(&self.inner.observers));
        for observer in observers {
            observer(disposition);
        }
        true
    }
}

impl fmt::Debug for CompletionFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionFence")
            .field("disposition", &self.disposition())
            .finish()
    }
}

impl CompletionWitness {
    pub(super) fn complete(&self) -> bool {
        self.fence.settle(CompletionDisposition::Invoked)
    }
}

impl Drop for CompletionWitness {
    fn drop(&mut self) {
        let _ = self.fence.settle(CompletionDisposition::Destroyed);
    }
}

trait RetirementExecutor: Send + Sync {
    fn execute(&self, job: Box<dyn FnOnce() + Send + 'static>);
}

struct DispatchRetirementExecutor {
    queue: DispatchRetained<DispatchQueue>,
}

impl RetirementExecutor for DispatchRetirementExecutor {
    fn execute(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        self.queue.exec_async(job);
    }
}

trait ErasedRetirementEntry: Send + Sync {}

struct RetirementEntry<T> {
    id: u64,
    owner: Mutex<Option<T>>,
    start_completion_done: AtomicBool,
    stop_disposition: AtomicU8,
    worker_done: AtomicBool,
    stop_deadline: Mutex<Option<DeadlineTicket>>,
}

impl<T: Send> ErasedRetirementEntry for RetirementEntry<T> {}

#[derive(Default)]
struct RetirementRegistry {
    entries: Mutex<HashMap<u64, Arc<dyn ErasedRetirementEntry>>>,
}

struct NativeLifecycleInner {
    deadlines: DeadlineScheduler,
    retirements: RetirementRegistry,
    retirement_executor: Arc<dyn RetirementExecutor>,
    next_retirement_id: AtomicU64,
}

#[derive(Clone)]
pub(super) struct NativeLifecycle {
    inner: Arc<NativeLifecycleInner>,
}

impl NativeLifecycle {
    pub(super) fn start() -> io::Result<Self> {
        static DEADLINES: OnceLock<DeadlineScheduler> = OnceLock::new();
        static DEADLINE_START: Mutex<()> = Mutex::new(());
        let _start = lock(&DEADLINE_START);
        let deadlines = match DEADLINES.get() {
            Some(deadlines) => deadlines.clone(),
            None => {
                let deadlines = DeadlineScheduler::start("hypercolor-macos-native-deadlines")?;
                let _ = DEADLINES.set(deadlines.clone());
                deadlines
            }
        };
        let retirement_executor = Arc::new(DispatchRetirementExecutor {
            queue: DispatchQueue::new(
                "tech.hyperbliss.hypercolor.screen-capture-retirement",
                DispatchQueueAttr::concurrent(),
            ),
        });
        Ok(Self::with_parts(deadlines, retirement_executor))
    }

    fn with_parts(
        deadlines: DeadlineScheduler,
        retirement_executor: Arc<dyn RetirementExecutor>,
    ) -> Self {
        Self {
            inner: Arc::new(NativeLifecycleInner {
                deadlines,
                retirements: RetirementRegistry::default(),
                retirement_executor,
                next_retirement_id: AtomicU64::new(1),
            }),
        }
    }

    pub(super) fn deadlines(&self) -> &DeadlineScheduler {
        &self.inner.deadlines
    }

    pub(super) fn retire<T: Send + 'static>(
        &self,
        owner: T,
        start_completion: CompletionFence,
        stop_deadline: Instant,
        run: impl FnOnce(&mut T, CompletionWitness) + Send + 'static,
        on_stop_timeout: impl Fn() + Send + Sync + 'static,
    ) -> u64 {
        self.retire_with_timeout_dequeued(
            owner,
            start_completion,
            stop_deadline,
            run,
            on_stop_timeout,
            || {},
        )
    }

    fn retire_with_timeout_dequeued<T: Send + 'static>(
        &self,
        owner: T,
        start_completion: CompletionFence,
        stop_deadline: Instant,
        run: impl FnOnce(&mut T, CompletionWitness) + Send + 'static,
        on_stop_timeout: impl Fn() + Send + Sync + 'static,
        on_timeout_dequeued: impl FnOnce() + Send + 'static,
    ) -> u64 {
        let id = self.next_retirement_id();
        let entry = Arc::new(RetirementEntry {
            id,
            owner: Mutex::new(Some(owner)),
            start_completion_done: AtomicBool::new(false),
            stop_disposition: AtomicU8::new(STOP_PENDING),
            worker_done: AtomicBool::new(false),
            stop_deadline: Mutex::new(None),
        });
        self.insert(id, Arc::clone(&entry) as Arc<dyn ErasedRetirementEntry>);

        let start_entry = Arc::downgrade(&entry);
        let start_lifecycle = Arc::downgrade(&self.inner);
        start_completion.observe(move |_| {
            if let Some(entry) = start_entry.upgrade() {
                entry.start_completion_done.store(true, Ordering::Release);
                release_if_settled(&start_lifecycle, &entry);
            }
        });

        let timeout_entry = Arc::downgrade(&entry);
        let timeout = Arc::new(on_stop_timeout);
        let scheduled_timeout = Arc::clone(&timeout);
        match self.inner.deadlines.schedule(stop_deadline, move || {
            if let Some(entry) = timeout_entry.upgrade() {
                on_timeout_dequeued();
                if entry
                    .stop_disposition
                    .compare_exchange(
                        STOP_PENDING,
                        STOP_TIMED_OUT,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    scheduled_timeout();
                }
            }
        }) {
            Ok(ticket) => *lock(&entry.stop_deadline) = Some(ticket),
            Err(_) => {
                if entry
                    .stop_disposition
                    .compare_exchange(
                        STOP_PENDING,
                        STOP_TIMED_OUT,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    timeout();
                }
            }
        }

        let stop_completion = CompletionFence::new();
        let stop_entry = Arc::downgrade(&entry);
        let stop_lifecycle = Arc::downgrade(&self.inner);
        stop_completion.observe(move |_| {
            if let Some(entry) = stop_entry.upgrade() {
                entry
                    .stop_disposition
                    .store(STOP_COMPLETED, Ordering::Release);
                drop(lock(&entry.stop_deadline).take());
                release_if_settled(&stop_lifecycle, &entry);
            }
        });

        let worker_entry = Arc::clone(&entry);
        let worker_lifecycle = Arc::downgrade(&self.inner);
        self.inner.retirement_executor.execute(Box::new(move || {
            {
                let mut owner = lock(&worker_entry.owner);
                let owner = owner
                    .as_mut()
                    .expect("registered native retirement retains its owner");
                run(owner, stop_completion.witness());
            }
            worker_entry.worker_done.store(true, Ordering::Release);
            release_if_settled(&worker_lifecycle, &worker_entry);
        }));
        id
    }

    pub(super) fn retire_without_native_stop<T: Send + 'static>(
        &self,
        owner: T,
        start_completion: CompletionFence,
        run: impl FnOnce(&mut T) + Send + 'static,
    ) -> u64 {
        let id = self.next_retirement_id();
        let entry = Arc::new(RetirementEntry {
            id,
            owner: Mutex::new(Some(owner)),
            start_completion_done: AtomicBool::new(false),
            stop_disposition: AtomicU8::new(STOP_COMPLETED),
            worker_done: AtomicBool::new(false),
            stop_deadline: Mutex::new(None),
        });
        self.insert(id, Arc::clone(&entry) as Arc<dyn ErasedRetirementEntry>);
        let start_entry = Arc::downgrade(&entry);
        let start_lifecycle = Arc::downgrade(&self.inner);
        start_completion.observe(move |_| {
            if let Some(entry) = start_entry.upgrade() {
                entry.start_completion_done.store(true, Ordering::Release);
                release_if_settled(&start_lifecycle, &entry);
            }
        });
        let worker_entry = Arc::clone(&entry);
        let worker_lifecycle = Arc::downgrade(&self.inner);
        self.inner.retirement_executor.execute(Box::new(move || {
            {
                let mut owner = lock(&worker_entry.owner);
                run(owner
                    .as_mut()
                    .expect("registered native retirement retains its owner"));
            }
            worker_entry.worker_done.store(true, Ordering::Release);
            release_if_settled(&worker_lifecycle, &worker_entry);
        }));
        id
    }

    fn next_retirement_id(&self) -> u64 {
        self.inner
            .next_retirement_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .expect("macOS native retirement identity must remain monotonic")
    }

    fn insert(&self, id: u64, entry: Arc<dyn ErasedRetirementEntry>) {
        let replaced = lock(&self.inner.retirements.entries).insert(id, entry);
        debug_assert!(replaced.is_none(), "native retirement identity is unique");
    }

    #[cfg(test)]
    fn pending_retirements(&self) -> usize {
        lock(&self.inner.retirements.entries).len()
    }
}

fn release_if_settled<T>(lifecycle: &Weak<NativeLifecycleInner>, entry: &Arc<RetirementEntry<T>>) {
    if !entry.start_completion_done.load(Ordering::Acquire)
        || entry.stop_disposition.load(Ordering::Acquire) != STOP_COMPLETED
        || !entry.worker_done.load(Ordering::Acquire)
    {
        return;
    }
    if let Some(lifecycle) = lifecycle.upgrade() {
        lock(&lifecycle.retirements.entries).remove(&entry.id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        CompletionDisposition, CompletionFence, CompletionWitness, DeadlineScheduler,
        NativeLifecycle, RetirementExecutor, lock,
    };

    #[derive(Default)]
    struct ControlledExecutor {
        jobs: Mutex<VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    }

    impl ControlledExecutor {
        fn take(&self) -> Box<dyn FnOnce() + Send + 'static> {
            lock(&self.jobs)
                .pop_front()
                .expect("controlled retirement job is pending")
        }
    }

    impl RetirementExecutor for ControlledExecutor {
        fn execute(&self, job: Box<dyn FnOnce() + Send + 'static>) {
            lock(&self.jobs).push_back(job);
        }
    }

    struct DropProbe(Arc<AtomicU64>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    struct FenceOwner {
        _completion: CompletionFence,
        _drop: DropProbe,
    }

    fn lifecycle() -> (NativeLifecycle, Arc<ControlledExecutor>) {
        let executor = Arc::new(ControlledExecutor::default());
        (
            NativeLifecycle::with_parts(
                DeadlineScheduler::manual(),
                Arc::clone(&executor) as Arc<dyn RetirementExecutor>,
            ),
            executor,
        )
    }

    fn completed_fence() -> CompletionFence {
        let fence = CompletionFence::new();
        assert!(fence.witness().complete());
        fence
    }

    #[test]
    fn completion_witness_invokes_observers_exactly_once() {
        let fence = CompletionFence::new();
        let witness = fence.witness();
        let calls = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&calls);
        fence.observe(move |disposition| {
            assert_eq!(disposition, CompletionDisposition::Invoked);
            observed.fetch_add(1, Ordering::AcqRel);
        });

        assert!(witness.complete());
        assert!(!witness.complete());
        drop(witness);

        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[test]
    fn destroying_completion_witness_settles_the_fence() {
        let fence = CompletionFence::new();
        let witness = fence.witness();
        let disposition = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&disposition);
        fence.observe(move |value| *lock(&observed) = Some(value));

        drop(witness);

        assert_eq!(*lock(&disposition), Some(CompletionDisposition::Destroyed));
    }

    #[test]
    fn stop_timeout_keeps_owner_quarantined_until_late_completion() {
        let (lifecycle, executor) = lifecycle();
        let drops = Arc::new(AtomicU64::new(0));
        let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
        let captured_witness = Arc::clone(&stop_witness);
        let timeouts = Arc::new(AtomicU64::new(0));
        let timeout_count = Arc::clone(&timeouts);
        let deadline = Instant::now() + Duration::from_secs(5);
        lifecycle.retire(
            DropProbe(Arc::clone(&drops)),
            completed_fence(),
            deadline,
            move |_, witness| *lock(&captured_witness) = Some(witness),
            move || {
                timeout_count.fetch_add(1, Ordering::AcqRel);
            },
        );
        executor.take()();
        lifecycle.inner.deadlines.expire_through(deadline);

        assert_eq!(timeouts.load(Ordering::Acquire), 1);
        assert_eq!(drops.load(Ordering::Acquire), 0);
        assert_eq!(lifecycle.pending_retirements(), 1);

        assert!(
            lock(&stop_witness)
                .take()
                .expect("late stop completion remains owned")
                .complete()
        );

        assert_eq!(lifecycle.pending_retirements(), 0);
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn completed_stop_beats_a_dequeued_timeout_before_its_claim() {
        let (lifecycle, executor) = lifecycle();
        let drops = Arc::new(AtomicU64::new(0));
        let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
        let captured_witness = Arc::clone(&stop_witness);
        let timeouts = Arc::new(AtomicU64::new(0));
        let timeout_count = Arc::clone(&timeouts);
        let timeout_dequeued = Arc::new(Barrier::new(2));
        let timeout_observed = Arc::clone(&timeout_dequeued);
        let resume_timeout = Arc::new(Barrier::new(2));
        let timeout_resume = Arc::clone(&resume_timeout);
        let deadline = Instant::now() + Duration::from_secs(5);
        lifecycle.retire_with_timeout_dequeued(
            DropProbe(Arc::clone(&drops)),
            completed_fence(),
            deadline,
            move |_, witness| *lock(&captured_witness) = Some(witness),
            move || {
                timeout_count.fetch_add(1, Ordering::AcqRel);
            },
            move || {
                timeout_observed.wait();
                timeout_resume.wait();
            },
        );
        executor.take()();

        let deadlines = lifecycle.inner.deadlines.clone();
        let timeout = thread::spawn(move || deadlines.expire_through(deadline));
        timeout_dequeued.wait();

        let witness = lock(&stop_witness)
            .take()
            .expect("native stop completion remains owned");
        assert!(witness.complete());
        assert!(!witness.complete());
        assert_eq!(lifecycle.pending_retirements(), 0);
        assert_eq!(drops.load(Ordering::Acquire), 0);

        resume_timeout.wait();
        timeout.join().expect("dequeued timeout exits");

        assert_eq!(timeouts.load(Ordering::Acquire), 0);
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn completion_destruction_releases_timed_out_owner() {
        let (lifecycle, executor) = lifecycle();
        let drops = Arc::new(AtomicU64::new(0));
        let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
        let captured_witness = Arc::clone(&stop_witness);
        let deadline = Instant::now() + Duration::from_secs(5);
        lifecycle.retire(
            DropProbe(Arc::clone(&drops)),
            completed_fence(),
            deadline,
            move |_, witness| *lock(&captured_witness) = Some(witness),
            || {},
        );
        executor.take()();
        lifecycle.inner.deadlines.expire_through(deadline);

        drop(lock(&stop_witness).take());

        assert_eq!(lifecycle.pending_retirements(), 0);
        assert_eq!(drops.load(Ordering::Acquire), 1);
    }

    #[test]
    fn dropping_lifecycle_tears_down_a_missing_completion_quarantine() {
        let (lifecycle, executor) = lifecycle();
        let drops = Arc::new(AtomicU64::new(0));
        let start_completion = CompletionFence::new();
        let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
        let captured_witness = Arc::clone(&stop_witness);
        lifecycle.retire(
            FenceOwner {
                _completion: start_completion.clone(),
                _drop: DropProbe(Arc::clone(&drops)),
            },
            start_completion,
            Instant::now() + Duration::from_secs(5),
            move |_, witness| *lock(&captured_witness) = Some(witness),
            || {},
        );
        executor.take()();
        assert_eq!(lifecycle.pending_retirements(), 1);

        drop(lifecycle);

        assert_eq!(drops.load(Ordering::Acquire), 1);
        drop(lock(&stop_witness).take());
    }

    #[test]
    fn independent_retirements_have_no_serial_head_of_line_blocking() {
        let (lifecycle, executor) = lifecycle();
        let blocked = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first_blocked = Arc::clone(&blocked);
        let first_release = Arc::clone(&release);
        let first_done = Arc::new(AtomicU64::new(0));
        let first_result = Arc::clone(&first_done);
        lifecycle.retire_without_native_stop((), completed_fence(), move |_| {
            first_blocked.wait();
            first_release.wait();
            first_result.fetch_add(1, Ordering::AcqRel);
        });
        let second_done = Arc::new(AtomicU64::new(0));
        let second_result = Arc::clone(&second_done);
        lifecycle.retire_without_native_stop((), completed_fence(), move |_| {
            second_result.fetch_add(1, Ordering::AcqRel);
        });
        let first = executor.take();
        let second = executor.take();
        let first_thread = thread::spawn(first);
        blocked.wait();

        second();

        assert_eq!(second_done.load(Ordering::Acquire), 1);
        assert_eq!(first_done.load(Ordering::Acquire), 0);
        release.wait();
        first_thread.join().expect("blocked retirement exits");
        assert_eq!(lifecycle.pending_retirements(), 0);
    }
}
