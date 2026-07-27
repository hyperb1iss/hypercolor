//! Session lifecycle: the readiness handshake, the bounded join, and stop.
//!
//! `start()` blocks until the worker has created its window, taken the
//! registration, and enumerated attached devices, and it returns the worker's
//! actual initialization error rather than succeeding ahead of that work. A
//! session that reports ready is one that can genuinely see input.
//!
//! The bounded join is where the interesting hazard lives. If the pump is
//! wedged — blocked in a driver call, or folding behind a render thread that
//! itself wedged — stop must return without discarding the join handle. A live
//! session retains the handle for a later stop probe; dropping the session
//! transfers it to a join reaper that observes eventual termination.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::claim::next_generation;
use crate::probe::interactive_session_state;
use crate::pump::{Pump, WM_HYPERCOLOR_STOP};
use crate::shared::{
    RawInputBatch, RawInputConfig, RawInputError, RawInputResult, SessionState, WorkerState,
};

/// How long `start()` waits for the worker to finish initializing.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// How long `stop()` waits before detaching a wedged pump. Generous enough
/// that an ordinary teardown always completes inside it, bounded so a wedged
/// driver call cannot hang daemon shutdown.
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// A live Raw Input capture session.
///
/// Holds control, join, and snapshot handles only — never the `HWND`, which is
/// thread-affine to the worker and stays there.
pub struct RawInputSession {
    stop: Arc<AtomicBool>,
    /// The pump's window, as a raw address so it can cross threads, guarded by
    /// a lock rather than an atomic.
    ///
    /// The lock is what makes posting safe against teardown. `PostMessageW`
    /// is callable from any thread, but the worker destroys this window on its
    /// own thread, and an address read just before destruction could be posted
    /// to just after — to a handle Windows may have already reissued to an
    /// unrelated window. The worker clears this slot *under the lock* before
    /// destroying the window, so a nudge either holds the lock and posts to a
    /// window that cannot be destroyed until it releases, or sees zero and
    /// posts nothing.
    window: Arc<Mutex<isize>>,
    device_count: Arc<AtomicUsize>,
    state: Arc<Mutex<WorkerState>>,
    finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
    join_timed_out: bool,
}

impl RawInputSession {
    /// Start capturing, blocking until the worker is ready or has failed.
    ///
    /// # Errors
    ///
    /// Returns [`RawInputError::NoInteractiveSession`] when the process has no
    /// visible window station, and the worker's own initialization error when
    /// window creation or registration failed. A failed probe is deliberately
    /// *not* a silent success: Raw Input would register happily and simply
    /// never deliver a message.
    pub fn start(
        config: RawInputConfig,
        sink: impl FnMut(RawInputBatch<'_>) + Send + 'static,
    ) -> RawInputResult<Self> {
        reap_retained_workers();
        if interactive_session_state() == SessionState::NoInteractiveSession {
            return Err(RawInputError::NoInteractiveSession);
        }
        if !config.keyboard && !config.mouse {
            return Err(RawInputError::NothingToCapture);
        }

        let generation = next_generation();
        let stop = Arc::new(AtomicBool::new(false));
        let window = Arc::new(Mutex::new(0isize));
        let device_count = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(WorkerState::Running));
        let (ready_tx, ready_rx) = mpsc::sync_channel::<RawInputResult<()>>(1);
        let (finished_tx, finished_rx) = mpsc::sync_channel::<()>(1);

        let worker = thread::Builder::new()
            .name("hypercolor-raw-input".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                let window = Arc::clone(&window);
                let device_count = Arc::clone(&device_count);
                let state = Arc::clone(&state);
                move || {
                    run_worker(
                        config,
                        generation,
                        sink,
                        &stop,
                        &window,
                        &device_count,
                        &state,
                        &ready_tx,
                    );
                    let _ = finished_tx.send(());
                }
            })
            .map_err(|error| RawInputError::WorkerSpawn(error.to_string()))?;

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                stop,
                window,
                device_count,
                state,
                finished: finished_rx,
                worker: Some(worker),
                join_timed_out: false,
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                retain_worker_until_exit(worker, "readiness timeout");
                Err(RawInputError::WorkerReadyTimeout)
            }
        }
    }

    /// Devices registered, identified, and streaming.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.device_count.load(Ordering::Acquire)
    }

    /// Liveness of the pump thread.
    ///
    /// A pump whose fold panicked reports `Failed`, so core reports the source
    /// unavailable rather than silently flatlining.
    #[must_use]
    pub fn worker_state(&self) -> WorkerState {
        if self
            .worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Ok(mut state) = self.state.lock()
            && matches!(*state, WorkerState::Running)
        {
            *state = WorkerState::Failed("raw input pump exited unexpectedly".to_owned());
        }
        self.state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Stop capturing. Idempotent, and safe to call from any thread.
    ///
    /// Sets the flag *and* posts the nudge so the common path tears down
    /// immediately rather than waiting out the wake budget. If the pump does
    /// not finish inside the join timeout, the session retains its handle so a
    /// later stop or drop still observes termination.
    pub fn stop(&mut self) {
        self.stop_with_timeout(JOIN_TIMEOUT);
    }

    fn stop_with_timeout(&mut self, timeout: Duration) {
        reap_retained_workers();
        if self.worker.is_none() {
            return;
        }
        self.stop.store(true, Ordering::Release);
        self.nudge();

        match self.finished.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                self.join_timed_out = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.join_timed_out = true;
                tracing::warn!(
                    "raw input pump did not stop within the join timeout; retaining its join handle"
                );
                if let Ok(mut guard) = self.state.lock() {
                    *guard =
                        WorkerState::Failed("pump did not stop within the join timeout".to_owned());
                }
            }
        }
    }

    /// Construct a deterministic stalled worker for lifecycle contract tests.
    #[doc(hidden)]
    #[must_use]
    pub fn test_stalled_worker(exit_after: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let window = Arc::new(Mutex::new(0));
        let device_count = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(WorkerState::Running));
        let (finished_tx, finished) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            thread::sleep(exit_after);
            let _ = finished_tx.send(());
        });
        Self {
            stop,
            window,
            device_count,
            state,
            finished,
            worker: Some(worker),
            join_timed_out: false,
        }
    }

    /// Run the production bounded-stop path with a deterministic test timeout.
    #[doc(hidden)]
    pub fn test_stop_with_timeout(&mut self, timeout: Duration) {
        self.stop_with_timeout(timeout);
    }

    /// Whether bounded stop still owns a worker whose exit is not observed.
    #[doc(hidden)]
    #[must_use]
    pub fn test_retains_worker(&self) -> bool {
        self.worker.is_some()
    }

    /// Exercise the readiness-timeout reaper without relying on OS timing.
    #[doc(hidden)]
    pub fn test_readiness_timeout_reaper(worker_delay: Duration, ready_timeout: Duration) -> bool {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            thread::sleep(worker_delay);
            let _ = ready_tx.send(());
        });
        if ready_rx.recv_timeout(ready_timeout).is_ok() {
            let _ = worker.join();
            return false;
        }
        retain_worker_until_exit(worker, "test readiness timeout")
            .recv_timeout(worker_delay + worker_delay)
            .is_ok()
    }

    /// Prove a failed reaper spawn leaves the worker owned and later reapable.
    #[doc(hidden)]
    pub fn test_reaper_spawn_failure_is_reapable(worker_delay: Duration) -> bool {
        reap_retained_workers();
        let baseline = retained_worker_count();
        let worker = thread::spawn(move || thread::sleep(worker_delay));
        let observed = retain_worker_until_exit_with(
            worker,
            "test reaper spawn failure",
            ReaperSpawn::InjectFailure,
        );
        let retained = retained_worker_count() == baseline + 1;
        thread::sleep(worker_delay + worker_delay);
        reap_retained_workers();
        retained
            && observed.recv_timeout(worker_delay).is_ok()
            && retained_worker_count() == baseline
    }

    /// Number of process-owned workers awaiting observed termination.
    #[doc(hidden)]
    #[must_use]
    pub fn test_retained_worker_count() -> usize {
        retained_worker_count()
    }

    /// Observe every retained worker that has now terminated.
    #[doc(hidden)]
    pub fn test_reap_retained_workers() {
        reap_retained_workers();
    }

    /// Wake the pump out of its wait so it observes the stop flag now.
    ///
    /// Posts while holding the window lock, so the worker cannot clear the
    /// slot and destroy the window underneath an in-flight post.
    fn nudge(&self) {
        let Ok(guard) = self.window.lock() else {
            return;
        };
        if *guard == 0 {
            return;
        }
        let window = HWND(std::ptr::without_provenance_mut(guard.cast_unsigned()));
        // SAFETY: `PostMessageW` is documented as callable from any thread; it
        // queues the message rather than touching window state. A window
        // already destroyed makes this fail cleanly, which the `let _` accepts.
        let _ = unsafe { PostMessageW(Some(window), WM_HYPERCOLOR_STOP, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for RawInputSession {
    fn drop(&mut self) {
        if self.join_timed_out {
            self.stop.store(true, Ordering::Release);
            self.nudge();
            if let Some(worker) = self.worker.take() {
                retain_worker_until_exit(worker, "session drop after stop timeout");
            }
            return;
        }
        self.stop();
        if let Some(worker) = self.worker.take() {
            retain_worker_until_exit(worker, "session drop after stop timeout");
        }
    }
}

fn retain_worker_until_exit(worker: JoinHandle<()>, reason: &'static str) -> mpsc::Receiver<()> {
    retain_worker_until_exit_with(worker, reason, ReaperSpawn::Thread)
}

#[derive(Clone, Copy)]
enum ReaperSpawn {
    Thread,
    InjectFailure,
}

struct RetainedWorker {
    worker: Mutex<Option<JoinHandle<()>>>,
    observed: Mutex<Option<mpsc::SyncSender<()>>>,
    completed: AtomicBool,
    reason: &'static str,
}

impl RetainedWorker {
    fn reap_if_finished(&self) -> bool {
        let mut worker_guard = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if worker_guard
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
        {
            let worker = worker_guard.take();
            drop(worker_guard);
            if let Some(worker) = worker {
                self.observe(worker);
            }
            return true;
        }
        false
    }

    fn reap(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            self.observe(worker);
        }
    }

    fn observe(&self, worker: JoinHandle<()>) {
        if let Err(panic) = worker.join() {
            tracing::warn!(
                ?panic,
                reason = self.reason,
                "raw input pump reaper observed a panic"
            );
        }
        self.completed.store(true, Ordering::Release);
        if let Some(observed) = self
            .observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = observed.send(());
        }
    }
}

fn retained_workers() -> &'static Mutex<Vec<Arc<RetainedWorker>>> {
    static RETAINED_WORKERS: OnceLock<Mutex<Vec<Arc<RetainedWorker>>>> = OnceLock::new();
    RETAINED_WORKERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn retain_worker_until_exit_with(
    worker: JoinHandle<()>,
    reason: &'static str,
    spawn: ReaperSpawn,
) -> mpsc::Receiver<()> {
    reap_retained_workers();
    let (observed_tx, observed_rx) = mpsc::sync_channel(1);
    let retained = Arc::new(RetainedWorker {
        worker: Mutex::new(Some(worker)),
        observed: Mutex::new(Some(observed_tx)),
        completed: AtomicBool::new(false),
        reason,
    });
    retained_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(Arc::clone(&retained));

    if retained.reap_if_finished() {
        prune_completed_workers();
        return observed_rx;
    }

    if matches!(spawn, ReaperSpawn::InjectFailure) {
        tracing::error!(reason, "injected raw input pump reaper spawn failure");
        return observed_rx;
    }

    let reaper = Arc::clone(&retained);
    if let Err(error) = thread::Builder::new()
        .name("hypercolor-raw-input-reaper".to_owned())
        .spawn(move || {
            reaper.reap();
            prune_completed_workers();
        })
    {
        tracing::error!(%error, reason, "failed to start raw input pump join reaper");
    }
    observed_rx
}

fn reap_retained_workers() {
    let workers = retained_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for worker in workers {
        worker.reap_if_finished();
    }
    prune_completed_workers();
}

fn prune_completed_workers() {
    retained_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|worker| !worker.completed.load(Ordering::Acquire));
}

fn retained_worker_count() -> usize {
    retained_workers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len()
}

/// The worker body: create the pump, report readiness, then loop.
#[expect(
    clippy::too_many_arguments,
    reason = "each handle is a distinct channel back to the controlling thread; \
              bundling them into a struct would only move the same arity"
)]
fn run_worker(
    config: RawInputConfig,
    generation: u64,
    mut sink: impl FnMut(RawInputBatch<'_>),
    stop: &AtomicBool,
    window: &Mutex<isize>,
    device_count: &AtomicUsize,
    state: &Mutex<WorkerState>,
    ready_tx: &mpsc::SyncSender<RawInputResult<()>>,
) {
    let mut pump = match Pump::create(config, generation, stop) {
        Ok(pump) => pump,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };

    if let Ok(mut slot) = window.lock() {
        *slot = pump.window().0.addr().cast_signed();
    }
    device_count.store(pump.device_count(), Ordering::Release);

    // Devices attached before registration produce no arrival notification, so
    // their arrivals are delivered here — before readiness, so core never sees
    // input from a device it was never told about.
    pump.queue_initial_arrivals();
    pump.flush_pending(&mut sink);

    let _ = ready_tx.send(Ok(()));

    loop {
        // A panic while folding must not take the process with it, and must
        // not leave core believing a dead source is live.
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let keep_going = pump.run_once(stop, &mut sink);
            pump.flush_pending(&mut sink);
            keep_going
        }));

        match outcome {
            Ok(true) => {}
            Ok(false) => break,
            Err(_) => {
                if let Ok(mut guard) = state.lock() {
                    *guard = WorkerState::Failed("raw input fold panicked".to_owned());
                }
                tracing::error!("raw input fold panicked; stopping the pump");
                break;
            }
        }
        device_count.store(pump.device_count(), Ordering::Release);
    }

    // Clear the slot before the pump's `Drop` destroys the window, so no
    // later nudge can post to a handle Windows may reissue. Held under the
    // lock, which any in-flight nudge also holds, so the destroy strictly
    // follows every post that had already begun.
    if let Ok(mut slot) = window.lock() {
        *slot = 0;
    }
    drop(pump);
}
