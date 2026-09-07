//! Process-wide cleanup for workers that outlive bounded shutdown.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const REAPER_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct RetainedWorker {
    worker: JoinHandle<()>,
    context: Arc<str>,
}

#[derive(Default)]
struct ReaperQueue {
    pending: Mutex<Vec<RetainedWorker>>,
    wake: Condvar,
    shutdown: AtomicBool,
}

struct WorkerReaper {
    queue: Arc<ReaperQueue>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    reaped_count: Arc<AtomicU64>,
}

impl WorkerReaper {
    fn start() -> io::Result<Self> {
        Self::start_with(|queue, reaped_count| {
            thread::Builder::new()
                .name("hypercolor-worker-reaper".to_owned())
                .spawn(move || reaper_loop(&queue, &reaped_count))
        })
    }

    fn start_with(
        spawn: impl FnOnce(Arc<ReaperQueue>, Arc<AtomicU64>) -> io::Result<JoinHandle<()>>,
    ) -> io::Result<Self> {
        let queue = Arc::new(ReaperQueue::default());
        let reaped_count = Arc::new(AtomicU64::new(0));
        let worker = spawn(Arc::clone(&queue), Arc::clone(&reaped_count))?;
        Ok(Self {
            queue,
            worker: Some(worker),
            #[cfg(test)]
            reaped_count,
        })
    }

    fn submit(&self, worker: JoinHandle<()>, context: Arc<str>) {
        self.queue
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(RetainedWorker { worker, context });
        self.queue.wake.notify_one();
    }

    #[cfg(test)]
    fn reaped_count(&self) -> u64 {
        self.reaped_count.load(Ordering::Acquire)
    }
}

impl Drop for WorkerReaper {
    fn drop(&mut self) {
        self.queue.shutdown.store(true, Ordering::Release);
        self.queue.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn reaper_loop(queue: &ReaperQueue, reaped_count: &AtomicU64) {
    let mut retained = Vec::new();
    loop {
        collect_pending(queue, &mut retained);
        reap_finished_workers(&mut retained, reaped_count);
        if queue.shutdown.load(Ordering::Acquire) && retained.is_empty() {
            break;
        }
    }
}

fn collect_pending(queue: &ReaperQueue, retained: &mut Vec<RetainedWorker>) {
    let mut pending = queue
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if pending.is_empty() {
        if retained.is_empty() && !queue.shutdown.load(Ordering::Acquire) {
            pending = queue
                .wake
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        } else if !retained.is_empty() {
            let (next, _) = queue
                .wake
                .wait_timeout(pending, REAPER_POLL_INTERVAL)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending = next;
        }
    }
    retained.append(&mut pending);
}

fn reap_finished_workers(retained: &mut Vec<RetainedWorker>, reaped_count: &AtomicU64) {
    let mut index = 0;
    while index < retained.len() {
        if retained[index].worker.is_finished() {
            observe_worker(retained.swap_remove(index));
            reaped_count.fetch_add(1, Ordering::Release);
        } else {
            index += 1;
        }
    }
}

fn observe_worker(retained: RetainedWorker) {
    if let Err(panic) = retained.worker.join() {
        tracing::warn!(
            worker = %retained.context,
            ?panic,
            "retained worker reaper observed a panic"
        );
    }
}

fn retention_service() -> io::Result<&'static WorkerReaper> {
    static REAPER: OnceLock<WorkerReaper> = OnceLock::new();
    static INITIALIZE: Mutex<()> = Mutex::new(());

    if let Some(reaper) = REAPER.get() {
        return Ok(reaper);
    }
    let _initialize = INITIALIZE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(reaper) = REAPER.get() {
        return Ok(reaper);
    }
    let reaper = WorkerReaper::start()?;
    let _ = REAPER.set(reaper);
    REAPER
        .get()
        .ok_or_else(|| io::Error::other("worker cleanup service was not retained after startup"))
}

/// Opaque identity for the process-wide cleanup service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct RetentionServiceIdentity(usize);

/// Return the identity of the process-wide cleanup service.
///
/// # Errors
///
/// Returns the cleanup-service spawn error when it cannot be initialized.
#[doc(hidden)]
pub fn retention_service_identity() -> io::Result<RetentionServiceIdentity> {
    retention_service()
        .map(|service| RetentionServiceIdentity(std::ptr::from_ref(service) as usize))
}

/// Spawn a worker only after the process cleanup service is guaranteed live.
///
/// # Errors
///
/// Returns the cleanup-service or worker-thread spawn error without running
/// the worker when cleanup capacity cannot be established.
pub fn spawn_worker(
    builder: thread::Builder,
    worker: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    spawn_worker_after(retention_service().map(|_| ()), builder, worker)
}

fn spawn_worker_after(
    cleanup_ready: io::Result<()>,
    builder: thread::Builder,
    worker: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    cleanup_ready?;
    builder.spawn(worker)
}

/// Transfer a still-running worker into guaranteed process-owned cleanup.
///
/// # Panics
///
/// Panics only when a caller bypassed [`spawn_worker`] and the process cleanup
/// service cannot be initialized while accepting the worker.
pub fn retain_worker(worker: JoinHandle<()>, context: impl Into<Arc<str>>) {
    retention_service()
        .expect("cleanup service is initialized before any retained worker starts")
        .submit(worker, context.into());
}

#[cfg(test)]
mod tests;
