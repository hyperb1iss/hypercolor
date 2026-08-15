#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;

use crate::{MacosInputDiagnostics, MacosInputEvent, MacosInputGapReason};

pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 2048;
const LATENCY_BUCKET_WIDTH_NS: u64 = 100_000;
const LATENCY_BUCKET_COUNT: usize = 4096;

struct QueuedInputEvent {
    event: MacosInputEvent,
    callback_entry: Instant,
}

struct AtomicLatencyHistogram {
    buckets: Box<[AtomicU64]>,
    generation: AtomicU64,
    sample_count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AtomicLatencySnapshot {
    sample_count: u64,
    total_ns: u64,
    max_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
}

impl Default for AtomicLatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: (0..=LATENCY_BUCKET_COUNT)
                .map(|_| AtomicU64::new(0))
                .collect(),
            generation: AtomicU64::new(0),
            sample_count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }
}

impl AtomicLatencyHistogram {
    fn record(&self, elapsed: Duration) {
        self.record_with_hook(elapsed, || {});
    }

    fn record_with_hook(&self, elapsed: Duration, before_complete: impl FnOnce()) {
        let generation = self.begin_write();
        let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let bucket = usize::try_from(elapsed_ns / LATENCY_BUCKET_WIDTH_NS)
            .unwrap_or(usize::MAX)
            .min(LATENCY_BUCKET_COUNT);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        let _ = self
            .total_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(elapsed_ns))
            });
        self.max_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
        before_complete();
        self.sample_count.fetch_add(1, Ordering::Relaxed);
        self.generation
            .store(generation.wrapping_add(1), Ordering::Release);
    }

    fn begin_write(&self) -> u64 {
        let mut generation = self.generation.load(Ordering::Relaxed);
        loop {
            if generation & 1 == 1 {
                std::hint::spin_loop();
                generation = self.generation.load(Ordering::Acquire);
                continue;
            }
            let started = generation.wrapping_add(1);
            match self.generation.compare_exchange_weak(
                generation,
                started,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return started,
                Err(observed) => generation = observed,
            }
        }
    }

    fn percentile_upper_bound_ns(&self, percentile: u64, sample_count: u64, maximum: u64) -> u64 {
        if sample_count == 0 {
            return 0;
        }
        let rank = sample_count.saturating_mul(percentile).saturating_add(99) / 100;
        let mut observed = 0_u64;
        for (index, count) in self.buckets.iter().enumerate() {
            observed = observed.saturating_add(count.load(Ordering::Relaxed));
            if observed >= rank {
                if index == LATENCY_BUCKET_COUNT {
                    return maximum;
                }
                return u64::try_from(index.saturating_add(1))
                    .unwrap_or(u64::MAX)
                    .saturating_mul(LATENCY_BUCKET_WIDTH_NS)
                    .min(maximum);
            }
        }
        maximum
    }

    fn snapshot(&self) -> AtomicLatencySnapshot {
        self.snapshot_with_hooks(|| {}, || {})
    }

    fn snapshot_with_hooks(
        &self,
        mut retrying: impl FnMut(),
        mut after_p95: impl FnMut(),
    ) -> AtomicLatencySnapshot {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            if generation & 1 == 1 {
                retrying();
                std::hint::spin_loop();
                continue;
            }
            let sample_count = self.sample_count.load(Ordering::Relaxed);
            let total_ns = self.total_ns.load(Ordering::Relaxed);
            let max_ns = self.max_ns.load(Ordering::Relaxed);
            let p95_ns = self.percentile_upper_bound_ns(95, sample_count, max_ns);
            after_p95();
            let p99_ns = self.percentile_upper_bound_ns(99, sample_count, max_ns);
            std::sync::atomic::fence(Ordering::Acquire);
            if self.generation.load(Ordering::Relaxed) == generation {
                return AtomicLatencySnapshot {
                    sample_count,
                    total_ns,
                    max_ns,
                    p95_ns,
                    p99_ns,
                };
            }
            retrying();
        }
    }
}

#[derive(Default)]
pub(crate) struct Diagnostics {
    events_received: AtomicU64,
    events_published: AtomicU64,
    dropped_events: AtomicU64,
    tap_disable_count: AtomicU64,
    tap_disabled_timeout: AtomicU64,
    tap_disabled_user_input: AtomicU64,
    tap_reenabled: AtomicU64,
    state_gaps: AtomicU64,
    unsupported_system_events: AtomicU64,
    invalid_scroll_phases: AtomicU64,
    last_point_delta_x: AtomicI64,
    last_point_delta_y: AtomicI64,
    callback_to_publication: AtomicLatencyHistogram,
    repeated_tap_disable: AtomicU8,
    secure_input_active: AtomicBool,
}

impl Diagnostics {
    fn snapshot(&self, queue_capacity: usize, queue_depth: usize) -> MacosInputDiagnostics {
        let callback_to_publication = self.callback_to_publication.snapshot();
        MacosInputDiagnostics {
            queue_capacity,
            queue_depth,
            events_received: self.events_received.load(Ordering::Relaxed),
            events_published: self.events_published.load(Ordering::Relaxed),
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            tap_disable_count: self.tap_disable_count.load(Ordering::Relaxed),
            tap_disabled_timeout: self.tap_disabled_timeout.load(Ordering::Relaxed),
            tap_disabled_user_input: self.tap_disabled_user_input.load(Ordering::Relaxed),
            tap_reenabled: self.tap_reenabled.load(Ordering::Relaxed),
            state_gaps: self.state_gaps.load(Ordering::Relaxed),
            unsupported_system_events: self.unsupported_system_events.load(Ordering::Relaxed),
            invalid_scroll_phases: self.invalid_scroll_phases.load(Ordering::Relaxed),
            last_point_delta_x: self.last_point_delta_x.load(Ordering::Relaxed),
            last_point_delta_y: self.last_point_delta_y.load(Ordering::Relaxed),
            callback_to_publication_sample_count: callback_to_publication.sample_count,
            callback_to_publication_total_ns: callback_to_publication.total_ns,
            callback_to_publication_max_ns: callback_to_publication.max_ns,
            callback_to_publication_p95_ns: callback_to_publication.p95_ns,
            callback_to_publication_p99_ns: callback_to_publication.p99_ns,
            secure_input_active: self.secure_input_active.load(Ordering::Relaxed),
        }
    }

    /// Publish the health tick's secure-input observation so status
    /// snapshots read this cached value instead of calling Carbon from
    /// other threads at frame rate.
    pub(crate) fn set_secure_input_active(&self, active: bool) {
        self.secure_input_active.store(active, Ordering::Relaxed);
    }

    fn record_received(&self) {
        self.events_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_published(&self, count: usize, callback_entries: &[Instant]) {
        self.events_published
            .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
        let published_at = Instant::now();
        for callback_entry in callback_entries {
            self.callback_to_publication
                .record(published_at.saturating_duration_since(*callback_entry));
        }
    }

    pub(crate) fn record_drop(&self) {
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_tap_disable(&self, repeated: bool, reason: MacosInputGapReason) {
        self.tap_disable_count.fetch_add(1, Ordering::Relaxed);
        match reason {
            MacosInputGapReason::TapDisabledTimeout => {
                self.tap_disabled_timeout.fetch_add(1, Ordering::Relaxed);
            }
            MacosInputGapReason::TapDisabledUserInput => {
                self.tap_disabled_user_input.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if repeated {
            let encoded = match reason {
                MacosInputGapReason::TapDisabledTimeout => 1,
                MacosInputGapReason::TapDisabledUserInput => 2,
                _ => 0,
            };
            self.repeated_tap_disable.store(encoded, Ordering::Release);
        }
    }

    pub(crate) fn record_tap_reenabled(&self) {
        self.tap_reenabled.fetch_add(1, Ordering::Relaxed);
    }

    fn record_gap(&self) {
        self.state_gaps.fetch_add(1, Ordering::Relaxed);
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
    events: ArrayQueue<QueuedInputEvent>,
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

    #[cfg(test)]
    pub(crate) fn enqueue(&self, event: MacosInputEvent) {
        self.enqueue_at(event, Instant::now());
    }

    pub(crate) fn enqueue_at(&self, event: MacosInputEvent, callback_entry: Instant) {
        self.diagnostics.record_received();
        if matches!(event, MacosInputEvent::StateGap { .. }) {
            self.diagnostics.record_gap();
        }
        if self.overflowed.load(Ordering::Acquire) {
            self.diagnostics.record_drop();
            return;
        }
        if self
            .events
            .push(QueuedInputEvent {
                event,
                callback_entry,
            })
            .is_err()
        {
            self.diagnostics.record_drop();
            self.overflowed.store(true, Ordering::Release);
        }
        self.notify();
    }

    pub(crate) fn request_gap(&self, reason: MacosInputGapReason) {
        self.diagnostics.record_gap();
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

    pub(crate) fn drain_into(
        &self,
        output: &mut Vec<MacosInputEvent>,
        callback_entries: &mut Vec<Instant>,
    ) {
        while let Some(queued) = self.events.pop() {
            output.push(queued.event);
            callback_entries.push(queued.callback_entry);
        }
        if self.overflowed.swap(false, Ordering::AcqRel) {
            self.diagnostics.record_gap();
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

    pub(crate) fn diagnostics_snapshot(&self) -> MacosInputDiagnostics {
        self.diagnostics
            .snapshot(self.events.capacity(), self.events.len())
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
        let mut callback_entries = Vec::new();

        queue.drain_into(&mut drained, &mut callback_entries);
        queue
            .diagnostics()
            .record_published(drained.len(), &callback_entries);

        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0], button(true));
        assert_eq!(drained[1], button(false));
        assert_eq!(
            drained[2],
            MacosInputEvent::StateGap {
                reason: MacosInputGapReason::QueueOverflow
            }
        );
        let diagnostics = queue.diagnostics_snapshot();
        assert_eq!(diagnostics.queue_capacity, 2);
        assert_eq!(diagnostics.queue_depth, 0);
        assert_eq!(diagnostics.events_received, 4);
        assert_eq!(diagnostics.events_published, 3);
        assert_eq!(diagnostics.dropped_events, 2);
        assert_eq!(diagnostics.state_gaps, 1);
    }

    #[test]
    fn terminal_gaps_follow_preceding_edges() {
        let queue = EventQueue::new(2);
        queue.enqueue(button(true));
        queue.request_gap(MacosInputGapReason::SourceStopped);
        queue.close();
        let mut drained = Vec::new();
        let mut callback_entries = Vec::new();

        queue.drain_into(&mut drained, &mut callback_entries);

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
        diagnostics.record_tap_disable(false, MacosInputGapReason::TapDisabledTimeout);
        diagnostics.record_tap_disable(true, MacosInputGapReason::TapDisabledUserInput);
        diagnostics.record_tap_reenabled();

        assert_eq!(
            diagnostics.take_repeated_tap_disable(),
            Some(MacosInputGapReason::TapDisabledUserInput)
        );
        assert_eq!(diagnostics.take_repeated_tap_disable(), None);
        let snapshot = diagnostics.snapshot(2_048, 17);
        assert_eq!(snapshot.queue_capacity, 2_048);
        assert_eq!(snapshot.queue_depth, 17);
        assert_eq!(snapshot.tap_disable_count, 2);
        assert_eq!(snapshot.tap_disabled_timeout, 1);
        assert_eq!(snapshot.tap_disabled_user_input, 1);
        assert_eq!(snapshot.tap_reenabled, 1);
    }

    #[test]
    fn callback_latency_records_only_after_canonical_publication() {
        let queue = EventQueue::new(2);
        queue.enqueue(button(true));
        let mut drained = Vec::new();
        let mut callback_entries = Vec::new();
        queue.drain_into(&mut drained, &mut callback_entries);

        let before = queue.diagnostics_snapshot();
        assert_eq!(before.events_published, 0);
        assert_eq!(before.callback_to_publication_sample_count, 0);

        queue
            .diagnostics()
            .record_published(drained.len(), &callback_entries);
        let after = queue.diagnostics_snapshot();
        assert_eq!(after.events_published, 1);
        assert_eq!(after.callback_to_publication_sample_count, 1);
        assert!(after.callback_to_publication_p95_ns > 0);
        assert!(after.callback_to_publication_p99_ns > 0);
    }

    #[test]
    fn callback_latency_percentiles_never_exceed_the_observed_maximum() {
        let histogram = AtomicLatencyHistogram::default();
        histogram.record(Duration::from_nanos(1));

        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.p95_ns, 1);
        assert_eq!(snapshot.p99_ns, 1);
        assert_eq!(snapshot.max_ns, 1);
    }

    #[test]
    fn callback_latency_snapshot_retries_when_population_changes() {
        let histogram = AtomicLatencyHistogram::default();
        histogram.record(Duration::from_nanos(40));
        let mut injected = false;

        let snapshot = histogram.snapshot_with_hooks(
            || {},
            || {
                if !injected {
                    histogram.record(Duration::from_nanos(70));
                    injected = true;
                }
            },
        );

        assert_eq!(
            snapshot,
            AtomicLatencySnapshot {
                sample_count: 2,
                total_ns: 110,
                max_ns: 70,
                p95_ns: 70,
                p99_ns: 70,
            }
        );
    }
}
