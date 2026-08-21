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
    stop: Option<StopTransaction>,
    worker_done: AtomicBool,
}

struct StopTransaction {
    generation: u64,
    deadline: Instant,
    completion: CompletionFence,
    disposition: AtomicU8,
    deadline_ticket: Mutex<Option<DeadlineTicket>>,
}

impl StopTransaction {
    fn new(generation: u64, deadline: Instant) -> Self {
        Self {
            generation,
            deadline,
            completion: CompletionFence::new(),
            disposition: AtomicU8::new(STOP_PENDING),
            deadline_ticket: Mutex::new(None),
        }
    }

    fn cancel_at_deadline(&self) -> bool {
        self.disposition
            .compare_exchange(
                STOP_PENDING,
                STOP_TIMED_OUT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn complete(&self) {
        self.disposition.store(STOP_COMPLETED, Ordering::Release);
        drop(lock(&self.deadline_ticket).take());
    }

    fn is_complete(&self) -> bool {
        self.disposition.load(Ordering::Acquire) == STOP_COMPLETED
    }
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
            stop: Some(StopTransaction::new(id, stop_deadline)),
            worker_done: AtomicBool::new(false),
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
                    .stop
                    .as_ref()
                    .expect("native retirement has a stop transaction")
                    .cancel_at_deadline()
                {
                    scheduled_timeout();
                }
            }
        }) {
            Ok(ticket) => {
                let stop = entry
                    .stop
                    .as_ref()
                    .expect("native retirement has a stop transaction");
                debug_assert_eq!(stop.generation, id);
                debug_assert_eq!(stop.deadline, stop_deadline);
                *lock(&stop.deadline_ticket) = Some(ticket);
            }
            Err(_) => {
                if entry
                    .stop
                    .as_ref()
                    .expect("native retirement has a stop transaction")
                    .cancel_at_deadline()
                {
                    timeout();
                }
            }
        }

        let stop_completion = entry
            .stop
            .as_ref()
            .expect("native retirement has a stop transaction")
            .completion
            .clone();
        let stop_entry = Arc::downgrade(&entry);
        let stop_lifecycle = Arc::downgrade(&self.inner);
        stop_completion.observe(move |_| {
            if let Some(entry) = stop_entry.upgrade() {
                entry
                    .stop
                    .as_ref()
                    .expect("native retirement has a stop transaction")
                    .complete();
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
            stop: None,
            worker_done: AtomicBool::new(false),
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
    pub(super) fn pending_retirements(&self) -> usize {
        lock(&self.inner.retirements.entries).len()
    }
}

fn release_if_settled<T>(lifecycle: &Weak<NativeLifecycleInner>, entry: &Arc<RetirementEntry<T>>) {
    if !entry.start_completion_done.load(Ordering::Acquire)
        || entry.stop.as_ref().is_some_and(|stop| !stop.is_complete())
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
mod tests;
