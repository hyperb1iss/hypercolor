//! Event-loop wakeups from Servo to the worker thread.
//!
//! Servo calls [`EventLoopWaker::wake`] from its own threads whenever it
//! queues a message for the embedder: script results, load status, paint
//! frames. The worker blocks on [`ServoWakeSignal::wait_until`] between
//! `spin_event_loop` calls, so it resumes the moment Servo has work instead
//! of polling on a fixed sleep.

use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use servo::EventLoopWaker;

/// Latched wakeup flag shared between Servo's threads and the worker.
#[derive(Debug, Default)]
pub(super) struct ServoWakeSignal {
    woken: Mutex<bool>,
    condvar: Condvar,
}

impl ServoWakeSignal {
    /// Latch a wakeup and release any waiter.
    pub(super) fn wake(&self) {
        let mut woken = self.woken.lock().unwrap_or_else(PoisonError::into_inner);
        *woken = true;
        self.condvar.notify_one();
    }

    /// Block until a wakeup is latched or `deadline` passes, consuming the
    /// latch when one arrived. A wakeup raised before this call (for example
    /// while the caller was spinning Servo's event loop) returns immediately,
    /// so no message can slip between a spin and the wait.
    ///
    /// Callers must check their completion condition after each spin and
    /// before waiting: once the latch is consumed, the next return may be a
    /// long way off.
    pub(super) fn wait_until(&self, deadline: Instant) {
        let mut woken = self.woken.lock().unwrap_or_else(PoisonError::into_inner);
        while !*woken {
            let Some(remaining) = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
            else {
                return;
            };
            woken = self
                .condvar
                .wait_timeout(woken, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *woken = false;
    }
}

/// A reply channel for callbacks Servo invokes outside its embedder
/// channels. It wakes the worker when it delivers, and when it is dropped
/// undelivered (after disconnecting), so the worker never waits out a
/// deadline for a reply that can no longer arrive.
#[derive(Debug)]
pub(super) struct WakingSender<T> {
    sender: Option<SyncSender<T>>,
    signal: Arc<ServoWakeSignal>,
}

impl<T> WakingSender<T> {
    pub(super) fn new(sender: SyncSender<T>, signal: Arc<ServoWakeSignal>) -> Self {
        Self {
            sender: Some(sender),
            signal,
        }
    }

    pub(super) fn send(&self, value: T) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(value);
        }
        self.signal.wake();
    }
}

impl<T> Drop for WakingSender<T> {
    fn drop(&mut self) {
        // Disconnect before waking so the woken worker observes it.
        drop(self.sender.take());
        self.signal.wake();
    }
}

/// The [`EventLoopWaker`] handed to `ServoBuilder`.
#[derive(Clone, Debug)]
pub(super) struct ServoEventLoopWaker {
    signal: Arc<ServoWakeSignal>,
}

impl ServoEventLoopWaker {
    pub(super) fn new(signal: Arc<ServoWakeSignal>) -> Self {
        Self { signal }
    }
}

impl EventLoopWaker for ServoEventLoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.signal.wake();
    }
}
