//! Guaranteed cleanup service for Raw Input workers that outlive bounded shutdown.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const REAPER_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct RetainedWorker {
    worker: JoinHandle<()>,
    reason: Arc<str>,
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
                .name("hypercolor-raw-input-reaper".to_owned())
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

    fn submit(&self, worker: JoinHandle<()>, reason: Arc<str>) {
        self.queue
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(RetainedWorker { worker, reason });
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
            reason = %retained.reason,
            ?panic,
            "raw input pump reaper observed a panic"
        );
    }
}

fn worker_reaper() -> io::Result<&'static WorkerReaper> {
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
    REAPER.get().ok_or_else(|| {
        io::Error::other("Raw Input cleanup service was not retained after initialization")
    })
}

pub(super) fn spawn_raw_input_worker(
    builder: thread::Builder,
    worker: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    spawn_worker_after(worker_reaper().map(|_| ()), builder, worker)
}

fn spawn_worker_after(
    cleanup_ready: io::Result<()>,
    builder: thread::Builder,
    worker: impl FnOnce() + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    cleanup_ready?;
    builder.spawn(worker)
}

pub(super) fn retain_raw_input_worker(worker: JoinHandle<()>, reason: impl Into<Arc<str>>) {
    worker_reaper()
        .expect("Raw Input cleanup service is initialized before its worker starts")
        .submit(worker, reason.into());
}

#[cfg(test)]
mod tests;
