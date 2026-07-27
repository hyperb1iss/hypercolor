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
use std::sync::{Arc, Mutex, mpsc};
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
use crate::worker_retention::{retain_raw_input_worker, spawn_raw_input_worker};

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

        let worker = spawn_raw_input_worker(
            thread::Builder::new().name("hypercolor-raw-input".to_owned()),
            {
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
            },
        )
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
                retain_raw_input_worker(worker, "readiness timeout");
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
                retain_raw_input_worker(worker, "session drop after stop timeout");
            }
            return;
        }
        self.stop();
        if let Some(worker) = self.worker.take() {
            retain_raw_input_worker(worker, "session drop after stop timeout");
        }
    }
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
    let initial_publication_active = run_initial_step(stop, || pump.queue_initial_arrivals())
        && run_initial_step(stop, || pump.flush_pending(&mut sink));

    if initial_publication_active {
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

fn run_initial_step(stop: &AtomicBool, step: impl FnOnce()) -> bool {
    if stop.load(Ordering::Acquire) {
        return false;
    }
    step();
    true
}

#[cfg(test)]
mod tests;
