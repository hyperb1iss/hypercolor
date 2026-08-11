#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use crossbeam_queue::ArrayQueue;

use crate::{MacosInputDiagnostics, MacosInputEvent, MacosInputGapReason};

pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 2048;

#[derive(Default)]
pub(crate) struct Diagnostics {
    dropped_events: AtomicU64,
    tap_disable_count: AtomicU64,
    unsupported_system_events: AtomicU64,
    invalid_scroll_phases: AtomicU64,
    last_point_delta_x: AtomicI64,
    last_point_delta_y: AtomicI64,
    repeated_tap_disable: AtomicU8,
}

impl Diagnostics {
    pub(crate) fn snapshot(&self) -> MacosInputDiagnostics {
        MacosInputDiagnostics {
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            tap_disable_count: self.tap_disable_count.load(Ordering::Relaxed),
            unsupported_system_events: self.unsupported_system_events.load(Ordering::Relaxed),
            invalid_scroll_phases: self.invalid_scroll_phases.load(Ordering::Relaxed),
            last_point_delta_x: self.last_point_delta_x.load(Ordering::Relaxed),
            last_point_delta_y: self.last_point_delta_y.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_drop(&self) {
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_tap_disable(&self, repeated: bool, reason: MacosInputGapReason) {
        self.tap_disable_count.fetch_add(1, Ordering::Relaxed);
        if repeated {
            let encoded = match reason {
                MacosInputGapReason::TapDisabledTimeout => 1,
                MacosInputGapReason::TapDisabledUserInput => 2,
                _ => 0,
            };
            self.repeated_tap_disable.store(encoded, Ordering::Release);
        }
    }

    pub(crate) fn record_unsupported_system_event(&self) {
        self.unsupported_system_events
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_invalid_scroll_phase(&self) {
        self.invalid_scroll_phases.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_point_delta(&self, x: i64, y: i64) {
        self.last_point_delta_x.store(x, Ordering::Relaxed);
        self.last_point_delta_y.store(y, Ordering::Relaxed);
    }

    pub(crate) fn take_repeated_tap_disable(&self) -> Option<MacosInputGapReason> {
        match self.repeated_tap_disable.swap(0, Ordering::AcqRel) {
            1 => Some(MacosInputGapReason::TapDisabledTimeout),
            2 => Some(MacosInputGapReason::TapDisabledUserInput),
            _ => None,
        }
    }
}

pub(crate) struct EventQueue {
    events: ArrayQueue<MacosInputEvent>,
    overflowed: AtomicBool,
    closed: AtomicBool,
    wake_tx: mpsc::SyncSender<()>,
    wake_rx: Mutex<mpsc::Receiver<()>>,
    terminal_gaps: Mutex<VecDeque<MacosInputGapReason>>,
    diagnostics: Diagnostics,
}

impl EventQueue {
    pub(crate) fn new(capacity: usize) -> Self {
        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        Self {
            events: ArrayQueue::new(capacity),
            overflowed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            wake_tx,
            wake_rx: Mutex::new(wake_rx),
            terminal_gaps: Mutex::new(VecDeque::new()),
            diagnostics: Diagnostics::default(),
        }
    }

    pub(crate) fn enqueue(&self, event: MacosInputEvent) {
        if self.overflowed.load(Ordering::Acquire) {
            self.diagnostics.record_drop();
            return;
        }
        if self.events.push(event).is_err() {
            self.diagnostics.record_drop();
            self.overflowed.store(true, Ordering::Release);
        }
        self.notify();
    }

    pub(crate) fn request_gap(&self, reason: MacosInputGapReason) {
        self.terminal_gaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(reason);
        self.notify();
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        if self.is_closed() {
            return;
        }
        let _ = self
            .wake_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv_timeout(timeout);
    }

    pub(crate) fn drain_into(&self, output: &mut Vec<MacosInputEvent>) {
        while let Some(event) = self.events.pop() {
            output.push(event);
        }
        if self.overflowed.swap(false, Ordering::AcqRel) {
            output.push(MacosInputEvent::StateGap {
                reason: MacosInputGapReason::QueueOverflow,
            });
        }
        output.extend(
            self.terminal_gaps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
                .map(|reason| MacosInputEvent::StateGap { reason }),
        );
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
            && !self.overflowed.load(Ordering::Acquire)
            && self
                .terminal_gaps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }

    pub(crate) fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    fn notify(&self) {
        let _ = self.wake_tx.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MacosPointerButton;

    fn button(pressed: bool) -> MacosInputEvent {
        MacosInputEvent::Button {
            button: MacosPointerButton::Left,
            pressed,
        }
    }

    #[test]
    fn overflow_ends_with_one_ordered_state_gap() {
        let queue = EventQueue::new(2);
        queue.enqueue(button(true));
        queue.enqueue(button(false));
        queue.enqueue(button(true));
        queue.enqueue(button(false));
        let mut drained = Vec::new();

        queue.drain_into(&mut drained);

        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0], button(true));
        assert_eq!(drained[1], button(false));
        assert_eq!(
            drained[2],
            MacosInputEvent::StateGap {
                reason: MacosInputGapReason::QueueOverflow
            }
        );
        assert_eq!(queue.diagnostics().snapshot().dropped_events, 2);
    }

    #[test]
    fn terminal_gaps_follow_preceding_edges() {
        let queue = EventQueue::new(2);
        queue.enqueue(button(true));
        queue.request_gap(MacosInputGapReason::SourceStopped);
        queue.close();
        let mut drained = Vec::new();

        queue.drain_into(&mut drained);

        assert_eq!(drained[0], button(true));
        assert_eq!(
            drained[1],
            MacosInputEvent::StateGap {
                reason: MacosInputGapReason::SourceStopped
            }
        );
        assert!(queue.is_closed());
        assert!(queue.is_empty());
    }

    #[test]
    fn repeated_tap_disable_retains_the_native_reason() {
        let diagnostics = Diagnostics::default();
        diagnostics.record_tap_disable(true, MacosInputGapReason::TapDisabledUserInput);

        assert_eq!(
            diagnostics.take_repeated_tap_disable(),
            Some(MacosInputGapReason::TapDisabledUserInput)
        );
        assert_eq!(diagnostics.take_repeated_tap_disable(), None);
    }
}
