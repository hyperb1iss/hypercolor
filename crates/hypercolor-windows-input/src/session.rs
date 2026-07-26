//! Session lifecycle: the readiness handshake, the bounded join, and stop.
//!
//! `start()` blocks until the worker has created its window, taken the
//! registration, and enumerated attached devices, and it returns the worker's
//! actual initialization error rather than succeeding ahead of that work. A
//! session that reports ready is one that can genuinely see input.
//!
//! The bounded join is where the interesting hazard lives. If the pump is
//! wedged — blocked in a driver call, or folding behind a render thread that
//! itself wedged — a bounded join has to give up and detach it, and a detached
//! thread could later wake and mutate core state belonging to a *restarted*
//! session. Capture toggles with effect demand, so restart is routine, not
//! exotic. Two guards close that: core rejects batches whose epoch it no
//! longer owns, and the registration claim in [`crate::claim`] stops the stale
//! worker from deregistering its replacement.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
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
    /// The pump's window, as a raw address so it can cross threads. Only ever
    /// used for `PostMessageW`, which is documented as callable from any
    /// thread; nothing here touches the window itself.
    window: Arc<AtomicIsize>,
    device_count: Arc<AtomicUsize>,
    state: Arc<Mutex<WorkerState>>,
    finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
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
        let window = Arc::new(AtomicIsize::new(0));
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
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
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
        self.state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Stop capturing. Idempotent, and safe to call from any thread.
    ///
    /// Sets the flag *and* posts the nudge so the common path tears down
    /// immediately rather than waiting out the wake budget. If the pump does
    /// not finish inside the join timeout it is detached rather than waited on
    /// forever; the epoch and the registration claim make that safe.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.nudge();

        let Some(worker) = self.worker.take() else {
            return;
        };
        match self.finished.recv_timeout(JOIN_TIMEOUT) {
            Ok(()) => {
                let _ = worker.join();
            }
            Err(_) => {
                tracing::warn!(
                    "raw input pump did not stop within the join timeout; detaching it. \
                     Its batches are epoch-rejected and it cannot deregister a replacement."
                );
                if let Ok(mut guard) = self.state.lock() {
                    *guard =
                        WorkerState::Failed("pump did not stop within the join timeout".to_owned());
                }
            }
        }
    }

    /// Wake the pump out of its wait so it observes the stop flag now.
    fn nudge(&self) {
        let handle = self.window.load(Ordering::Acquire);
        if handle == 0 {
            return;
        }
        let window = HWND(std::ptr::without_provenance_mut(handle.cast_unsigned()));
        // SAFETY: `PostMessageW` is documented as callable from any thread; it
        // queues the message rather than touching window state. A window
        // already destroyed makes this fail cleanly, which the `let _` accepts.
        let _ = unsafe { PostMessageW(Some(window), WM_HYPERCOLOR_STOP, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for RawInputSession {
    fn drop(&mut self) {
        self.stop();
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
    window: &AtomicIsize,
    device_count: &AtomicUsize,
    state: &Mutex<WorkerState>,
    ready_tx: &mpsc::SyncSender<RawInputResult<()>>,
) {
    let mut pump = match Pump::create(config, generation) {
        Ok(pump) => pump,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };

    window.store(pump.window().0.addr().cast_signed(), Ordering::Release);
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
}
