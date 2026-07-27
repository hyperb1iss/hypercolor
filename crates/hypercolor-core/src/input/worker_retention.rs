//! Process-owned retention for input workers that outlive bounded shutdown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct RetainedInputWorker {
    id: u64,
    worker: Mutex<Option<JoinHandle<()>>>,
    observed: Mutex<Option<mpsc::SyncSender<()>>>,
    completed: AtomicBool,
    context: Arc<str>,
}

impl RetainedInputWorker {
    fn reap_if_finished(&self) -> bool {
        let mut worker_guard = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker_guard
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return false;
        }
        let worker = worker_guard.take();
        drop(worker_guard);
        if let Some(worker) = worker {
            self.observe(worker);
        }
        true
    }

    fn reap(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            self.observe(worker);
        }
    }

    fn observe(&self, worker: JoinHandle<()>) {
        if let Err(panic) = worker.join() {
            tracing::warn!(
                worker = %self.context,
                ?panic,
                "retained input worker reaper observed a panic"
            );
        }
        self.completed.store(true, Ordering::Release);
        if let Some(observed) = self
            .observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = observed.send(());
        }
    }
}

fn retained_workers() -> &'static Mutex<Vec<Arc<RetainedInputWorker>>> {
    static RETAINED_WORKERS: OnceLock<Mutex<Vec<Arc<RetainedInputWorker>>>> = OnceLock::new();
    RETAINED_WORKERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn next_worker_id() -> u64 {
    static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_WORKER_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
enum ReaperSpawn {
    Thread,
    InjectFailure,
}

struct RetentionReceipt {
    id: u64,
    observed: mpsc::Receiver<()>,
}

/// Transfer a still-running input worker into process-owned retention.
pub(crate) fn retain_input_worker(
    worker: JoinHandle<()>,
    reaper_name: &'static str,
    context: impl Into<Arc<str>>,
) {
    let _ = retain_input_worker_with(worker, reaper_name, context.into(), ReaperSpawn::Thread);
}

fn retain_input_worker_with(
    worker: JoinHandle<()>,
    reaper_name: &'static str,
    context: Arc<str>,
    spawn: ReaperSpawn,
) -> RetentionReceipt {
    reap_retained_input_workers();
    let id = next_worker_id();
    let (observed_tx, observed) = mpsc::sync_channel(1);
    let retained = Arc::new(RetainedInputWorker {
        id,
        worker: Mutex::new(Some(worker)),
        observed: Mutex::new(Some(observed_tx)),
        completed: AtomicBool::new(false),
        context,
    });
    retained_workers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Arc::clone(&retained));

    if retained.reap_if_finished() {
        prune_completed_workers();
        return RetentionReceipt { id, observed };
    }
    if matches!(spawn, ReaperSpawn::InjectFailure) {
        tracing::error!(
            worker = %retained.context,
            "injected input worker reaper spawn failure"
        );
        return RetentionReceipt { id, observed };
    }

    let reaper = Arc::clone(&retained);
    if let Err(error) = thread::Builder::new()
        .name(reaper_name.to_owned())
        .spawn(move || {
            reaper.reap();
            prune_completed_workers();
        })
    {
        tracing::error!(
            worker = %retained.context,
            %error,
            "failed to start input worker join reaper"
        );
    }
    RetentionReceipt { id, observed }
}

fn reap_retained_input_workers() {
    let workers = retained_workers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for worker in workers {
        worker.reap_if_finished();
    }
    prune_completed_workers();
}

fn prune_completed_workers() {
    retained_workers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|worker| !worker.completed.load(Ordering::Acquire));
}

fn registry_contains(id: u64) -> bool {
    retained_workers()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|worker| worker.id == id)
}

/// Prove an injected reaper-spawn failure remains process-owned and reapable.
#[doc(hidden)]
#[must_use]
pub fn test_reaper_spawn_failure_is_reapable(timeout: Duration) -> bool {
    let (release_tx, release_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = release_rx.recv();
    });
    let receipt = retain_input_worker_with(
        worker,
        "hypercolor-test-input-reaper",
        Arc::from("injected retention test worker"),
        ReaperSpawn::InjectFailure,
    );
    if !registry_contains(receipt.id) || release_tx.send(()).is_err() {
        return false;
    }

    let deadline = Instant::now() + timeout;
    loop {
        reap_retained_input_workers();
        if receipt.observed.try_recv().is_ok() {
            return !registry_contains(receipt.id);
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::yield_now();
    }
}
