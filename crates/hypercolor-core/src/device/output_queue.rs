//! Latest-frame output queues for device writes.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use hypercolor_types::device::DeviceId;
use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use super::traits::{DeviceBackend, DeviceFrameSink, DeviceWriteOutcome, OutputCadence};

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type DeviceFrameSinkHandle = Arc<dyn DeviceFrameSink>;
const OUTPUT_WRITE_FAILURE_REPEAT_LOG_INTERVAL: u64 = 60;

pub(super) type OutputLaneHandle = Arc<OutputLane>;

pub(super) enum OutputLane {
    Backend {
        backend: BackendHandle,
        device_id: DeviceId,
    },
    FrameSink {
        frame_sink: DeviceFrameSinkHandle,
    },
}

impl OutputLane {
    pub(super) fn backend(backend: BackendHandle, device_id: DeviceId) -> OutputLaneHandle {
        Arc::new(Self::Backend { backend, device_id })
    }

    pub(super) fn frame_sink(frame_sink: DeviceFrameSinkHandle) -> OutputLaneHandle {
        Arc::new(Self::FrameSink { frame_sink })
    }

    pub(super) const fn uses_frame_sink(&self) -> bool {
        matches!(self, Self::FrameSink { .. })
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> anyhow::Result<DeviceWriteOutcome> {
        match self {
            Self::Backend { backend, device_id } => {
                let mut backend = backend.lock().await;
                backend.write_colors_shared_outcome(device_id, colors).await
            }
            Self::FrameSink { frame_sink } => frame_sink.write_colors_shared_outcome(colors).await,
        }
    }
}

/// Snapshot of backend dispatch internals for reverse-engineering and tuning.
#[derive(Debug, Clone, Serialize)]
pub struct BackendManagerDebugSnapshot {
    /// Number of active output queues.
    pub queue_count: usize,

    /// Number of mapped layout devices.
    pub mapped_device_count: usize,

    /// Per-queue diagnostics.
    pub queues: Vec<OutputQueueDebugSnapshot>,
}

/// Snapshot of layout-to-backend routing state.
#[derive(Debug, Clone, Serialize)]
pub struct BackendRoutingDebugSnapshot {
    /// Registered backend IDs.
    pub backend_ids: Vec<String>,

    /// Number of layout-device mappings.
    pub mapping_count: usize,

    /// Number of active output queues.
    pub queue_count: usize,

    /// Detailed routing entries for each mapped layout device.
    pub mappings: Vec<LayoutRoutingDebugEntry>,

    /// Active queues with no corresponding layout mapping.
    pub orphaned_queues: Vec<OrphanedQueueDebugEntry>,
}

/// One layout-device routing entry.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutRoutingDebugEntry {
    /// Layout-level device reference.
    pub layout_device_id: String,

    /// Target backend ID.
    pub backend_id: String,

    /// Target backend device ID.
    pub device_id: String,

    /// Whether the target backend is currently registered.
    pub backend_registered: bool,

    /// Whether a queue is active for this mapping.
    pub queue_active: bool,
}

/// Queue entry that currently has no layout mapping.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedQueueDebugEntry {
    /// Backend ID for the orphaned queue.
    pub backend_id: String,

    /// Device ID for the orphaned queue.
    pub device_id: String,
}

/// Debug stats for a single output queue.
#[derive(Debug, Clone, Serialize)]
pub struct OutputQueueDebugSnapshot {
    /// Backend ID this queue targets.
    pub backend_id: String,

    /// Device ID this queue targets.
    pub device_id: String,

    /// Layout device IDs currently routed to this queue.
    pub mapped_layout_ids: Vec<String>,

    /// Configured target frame rate for this queue.
    pub target_fps: u32,

    /// Configured minimum output interval in milliseconds.
    pub target_interval_ms: Option<u64>,

    /// Whether this queue writes through a per-device hot-path frame sink.
    pub uses_frame_sink: bool,

    /// Whether the queue worker task has finished unexpectedly.
    pub worker_finished: bool,

    /// Total worker tasks replaced after finishing unexpectedly.
    pub worker_recoveries: u64,

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

    /// Total frames intentionally suppressed by the output lane.
    pub frames_suppressed: u64,

    /// Payload bytes successfully written by the worker.
    pub bytes_sent: u64,

    /// Frames dropped due to latest-frame replacement while I/O was busy.
    pub frames_dropped: u64,

    /// Average latency from enqueue to write completion.
    pub avg_latency_ms: u64,

    /// Average time spent waiting in the latest-frame slot before a write starts.
    pub avg_queue_wait_ms: u64,

    /// Average backend write duration from write start to write completion.
    pub avg_write_ms: u64,

    /// Last async write error observed by this queue worker.
    pub last_error: Option<String>,

    /// Total async write failures observed by this queue worker.
    pub errors_total: u64,

    /// Total async write failure warning logs emitted by this queue worker.
    pub write_failure_warnings_total: u64,

    /// Milliseconds since last worker write attempt.
    pub last_sent_ago_ms: Option<u64>,

    /// Most recent frame sequence seen by this queue.
    pub last_sequence: u64,
}

/// Typed per-device async output telemetry snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOutputStatistics {
    /// Backend ID this queue targets.
    pub backend_id: String,

    /// Device ID this queue targets.
    pub device_id: DeviceId,

    /// Layout device IDs currently routed to this queue.
    pub mapped_layout_ids: Vec<String>,

    /// Configured target frame rate for this queue.
    pub target_fps: u32,

    /// Configured minimum output interval in milliseconds.
    pub target_interval_ms: Option<u64>,

    /// Whether this queue writes through a per-device hot-path frame sink.
    pub uses_frame_sink: bool,

    /// Whether the queue worker task has finished unexpectedly.
    pub worker_finished: bool,

    /// Total worker tasks replaced after finishing unexpectedly.
    pub worker_recoveries: u64,

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

    /// Total frames intentionally suppressed by the output lane.
    pub frames_suppressed: u64,

    /// Payload bytes successfully written by the worker.
    pub bytes_sent: u64,

    /// Frames dropped due to latest-frame replacement while I/O was busy.
    pub frames_dropped: u64,

    /// Average latency from enqueue to write completion.
    pub avg_latency_ms: u64,

    /// Average time spent waiting in the latest-frame slot before a write starts.
    pub avg_queue_wait_ms: u64,

    /// Average backend write duration from write start to write completion.
    pub avg_write_ms: u64,

    /// Last async write error observed by this queue worker.
    pub last_error: Option<String>,

    /// Total async write failures observed by this queue worker.
    pub errors_total: u64,

    /// Total async write failure warning logs emitted by this queue worker.
    pub write_failure_warnings_total: u64,

    /// Milliseconds since last worker write attempt.
    pub last_sent_ago_ms: Option<u64>,

    /// Most recent frame sequence seen by this queue.
    pub last_sequence: u64,
}

impl DeviceOutputStatistics {
    pub(super) fn into_debug_snapshot(self) -> OutputQueueDebugSnapshot {
        OutputQueueDebugSnapshot {
            backend_id: self.backend_id,
            device_id: self.device_id.to_string(),
            mapped_layout_ids: self.mapped_layout_ids,
            target_fps: self.target_fps,
            target_interval_ms: self.target_interval_ms,
            uses_frame_sink: self.uses_frame_sink,
            worker_finished: self.worker_finished,
            worker_recoveries: self.worker_recoveries,
            frames_received: self.frames_received,
            frames_sent: self.frames_sent,
            frames_suppressed: self.frames_suppressed,
            bytes_sent: self.bytes_sent,
            frames_dropped: self.frames_dropped,
            avg_latency_ms: self.avg_latency_ms,
            avg_queue_wait_ms: self.avg_queue_wait_ms,
            avg_write_ms: self.avg_write_ms,
            last_error: self.last_error,
            errors_total: self.errors_total,
            write_failure_warnings_total: self.write_failure_warnings_total,
            last_sent_ago_ms: self.last_sent_ago_ms,
            last_sequence: self.last_sequence,
        }
    }
}

/// One async device write failure observed by an output queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncWriteFailure {
    /// Backend ID that owns the queue.
    pub backend_id: String,
    /// Physical device ID targeted by the queue.
    pub device_id: DeviceId,
    /// Most recent async write error string.
    pub error: String,
}

#[derive(Debug)]
struct OutputQueueMetrics {
    started_at: Instant,
    frames_received: AtomicU64,
    frames_sent: AtomicU64,
    frames_suppressed: AtomicU64,
    worker_recoveries: AtomicU64,
    bytes_sent: AtomicU64,
    frames_dropped: AtomicU64,
    total_latency_us: AtomicU64,
    total_queue_wait_us: AtomicU64,
    total_write_time_us: AtomicU64,
    errors_total: AtomicU64,
    write_failure_warnings_total: AtomicU64,
    last_sent_offset_us: AtomicU64,
    last_sequence: AtomicU64,
    last_success_sequence: AtomicU64,
    last_error_sequence: AtomicU64,
    last_error: StdMutex<Option<String>>,
}

impl OutputQueueMetrics {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            frames_received: AtomicU64::new(0),
            frames_sent: AtomicU64::new(0),
            frames_suppressed: AtomicU64::new(0),
            worker_recoveries: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            total_queue_wait_us: AtomicU64::new(0),
            total_write_time_us: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            write_failure_warnings_total: AtomicU64::new(0),
            last_sent_offset_us: AtomicU64::new(0),
            last_sequence: AtomicU64::new(0),
            last_success_sequence: AtomicU64::new(0),
            last_error_sequence: AtomicU64::new(0),
            last_error: StdMutex::new(None),
        }
    }

    fn record_received(&self, sequence: u64) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
        self.last_sequence.store(sequence, Ordering::Relaxed);
    }

    fn record_dropped(&self, dropped: u64) {
        self.frames_dropped.fetch_add(dropped, Ordering::Relaxed);
    }

    fn record_write_success(
        &self,
        sequence: u64,
        queue_wait: Duration,
        write_time: Duration,
        total_latency: Duration,
        sent_at: Instant,
        bytes_sent: usize,
    ) {
        self.frames_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(
            u64::try_from(bytes_sent).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.total_queue_wait_us
            .fetch_add(duration_micros(queue_wait), Ordering::Relaxed);
        self.total_write_time_us
            .fetch_add(duration_micros(write_time), Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(duration_micros(total_latency), Ordering::Relaxed);
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.last_success_sequence
            .store(sequence, Ordering::Relaxed);
    }

    fn record_write_suppressed(&self, sequence: u64, sent_at: Instant) {
        self.frames_suppressed.fetch_add(1, Ordering::Relaxed);
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.last_success_sequence
            .store(sequence, Ordering::Relaxed);
    }

    fn record_write_error(&self, sequence: u64, sent_at: Instant, error: String) {
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.errors_total.fetch_add(1, Ordering::Relaxed);
        self.last_error_sequence.store(sequence, Ordering::Relaxed);
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = Some(error);
        }
    }

    fn record_write_failure_warning(&self) {
        self.write_failure_warnings_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_worker_recovery(&self) {
        self.worker_recoveries.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
        cadence: OutputCadence,
        uses_frame_sink: bool,
        worker_finished: bool,
    ) -> DeviceOutputStatistics {
        let frames_received = self.frames_received.load(Ordering::Relaxed);
        let frames_sent = self.frames_sent.load(Ordering::Relaxed);
        let frames_suppressed = self.frames_suppressed.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let frames_dropped = self.frames_dropped.load(Ordering::Relaxed);
        let avg_latency_ms =
            average_micros_ms(self.total_latency_us.load(Ordering::Relaxed), frames_sent);
        let avg_queue_wait_ms = average_micros_ms(
            self.total_queue_wait_us.load(Ordering::Relaxed),
            frames_sent,
        );
        let avg_write_ms = average_micros_ms(
            self.total_write_time_us.load(Ordering::Relaxed),
            frames_sent,
        );
        let last_sent_offset_us = self.last_sent_offset_us.load(Ordering::Relaxed);
        let last_sent_ago_ms = (last_sent_offset_us > 0).then(|| {
            let last_sent_at = self
                .started_at
                .checked_add(Duration::from_micros(last_sent_offset_us))
                .unwrap_or(self.started_at);
            let ms = Instant::now()
                .saturating_duration_since(last_sent_at)
                .as_millis();
            u64::try_from(ms).unwrap_or(u64::MAX)
        });
        let last_error = (self.last_error_sequence.load(Ordering::Relaxed)
            > self.last_success_sequence.load(Ordering::Relaxed))
        .then(|| self.last_error.lock().ok().and_then(|guard| guard.clone()))
        .flatten();

        DeviceOutputStatistics {
            backend_id: backend_id.to_owned(),
            device_id,
            mapped_layout_ids,
            target_fps: cadence.target_fps(),
            target_interval_ms: cadence.interval_ms(),
            uses_frame_sink,
            worker_finished,
            worker_recoveries: self.worker_recoveries.load(Ordering::Relaxed),
            frames_received,
            frames_sent,
            frames_suppressed,
            bytes_sent,
            frames_dropped,
            avg_latency_ms,
            avg_queue_wait_ms,
            avg_write_ms,
            last_error,
            errors_total: self.errors_total.load(Ordering::Relaxed),
            write_failure_warnings_total: self.write_failure_warnings_total.load(Ordering::Relaxed),
            last_sent_ago_ms,
            last_sequence: self.last_sequence.load(Ordering::Relaxed),
        }
    }

    fn last_error(&self) -> Option<String> {
        (self.last_error_sequence.load(Ordering::Relaxed)
            > self.last_success_sequence.load(Ordering::Relaxed))
        .then(|| self.last_error.lock().ok().and_then(|guard| guard.clone()))
        .flatten()
    }
}

// ── OutputQueue ─────────────────────────────────────────────────────────────

/// Frame payload queued for asynchronous backend writes.
#[derive(Debug, Clone)]
struct FramePayload {
    /// LED colors for the target device.
    colors: Arc<Vec<[u8; 3]>>,
    /// Monotonic sequence for dropped-frame diagnostics.
    sequence: u64,
    /// Timestamp when this payload was queued by the render loop.
    produced_at: Instant,
}

#[derive(Debug, Default)]
pub(super) struct DeviceStagingBuffer {
    pub(super) output: Vec<[u8; 3]>,
    pub(super) remap_scratch: Vec<[u8; 3]>,
    pub(super) written_ranges: Vec<Range<usize>>,
    pub(super) has_segmented_write: bool,
    pub(super) required_len: usize,
    pub(super) frame_generation: u64,
}

impl DeviceStagingBuffer {
    pub(super) fn mark_written_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }

        if let Some(last) = self.written_ranges.last_mut() {
            if start >= last.start && start <= last.end {
                last.end = last.end.max(end);
                return;
            }

            if start >= last.end {
                self.written_ranges.push(start..end);
                return;
            }
        }

        let mut new_start = start;
        let mut new_end = end;
        let mut index = 0;

        while index < self.written_ranges.len() {
            let existing = &self.written_ranges[index];
            if existing.end < new_start {
                index += 1;
                continue;
            }

            if existing.start > new_end {
                break;
            }

            let existing = self.written_ranges.remove(index);
            new_start = new_start.min(existing.start);
            new_end = new_end.max(existing.end);
        }

        self.written_ranges.insert(index, new_start..new_end);
    }
}

/// Latest-frame queue for a single `(backend_id, device_id)` target.
///
/// Internally uses a `watch` channel so stale queued payloads are replaced
/// atomically and the sender never blocks the render loop.
pub(super) struct OutputQueue {
    tx: watch::Sender<Option<Arc<FramePayload>>>,
    io_task: JoinHandle<()>,
    cadence: OutputCadence,
    uses_frame_sink: bool,
    metrics: Arc<OutputQueueMetrics>,
    next_sequence: u64,
}

impl OutputQueue {
    /// Spawn an output worker for one physical target.
    pub(super) fn spawn(
        backend_id: String,
        device_id: DeviceId,
        lane: OutputLaneHandle,
        cadence: OutputCadence,
    ) -> Self {
        let metrics = Arc::new(OutputQueueMetrics::new(Instant::now()));
        Self::spawn_with_state(backend_id, device_id, lane, cadence, None, metrics, 0)
    }

    fn spawn_with_state(
        backend_id: String,
        device_id: DeviceId,
        lane: OutputLaneHandle,
        cadence: OutputCadence,
        initial_payload: Option<Arc<FramePayload>>,
        metrics: Arc<OutputQueueMetrics>,
        next_sequence: u64,
    ) -> Self {
        let (tx, mut rx) = watch::channel(initial_payload);
        let metrics_for_task = Arc::clone(&metrics);
        let uses_frame_sink = lane.uses_frame_sink();
        let initial_last_sent_sequence = metrics.last_success_sequence.load(Ordering::Relaxed);

        let io_task = tokio::spawn(async move {
            let send_interval = cadence.min_interval();
            let mut next_send_at = Instant::now();
            let mut last_sent_sequence = initial_last_sent_sequence;
            let mut pending = rx.borrow_and_update().clone();
            let mut last_logged_write_error = None::<String>;
            let mut repeated_write_failures_since_log = 0_u64;

            'worker: loop {
                if pending.is_none() {
                    // Sender dropped => manager shutdown or queue removed.
                    if rx.changed().await.is_err() {
                        break;
                    }
                    pending.clone_from(&rx.borrow_and_update());
                    continue;
                }

                if send_interval.is_some() {
                    while Instant::now() < next_send_at {
                        tokio::select! {
                            changed = rx.changed() => {
                                if changed.is_err() {
                                    break 'worker;
                                }
                                pending.clone_from(&rx.borrow_and_update());
                                if pending.is_none() {
                                    continue 'worker;
                                }
                            }
                            () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {
                                break;
                            }
                        }
                    }
                }

                let Some(frame) = pending.take() else {
                    continue;
                };

                let write_started = Instant::now();
                let queue_wait = write_started.saturating_duration_since(frame.produced_at);

                if frame.sequence > last_sent_sequence + 1 {
                    let dropped = frame.sequence - last_sent_sequence - 1;
                    metrics_for_task.record_dropped(dropped);

                    trace!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        dropped,
                        "dropping stale device frames"
                    );
                }

                let result = lane
                    .write_colors_shared_outcome(Arc::clone(&frame.colors))
                    .await;
                let send_completed = Instant::now();
                let write_time = send_completed.saturating_duration_since(write_started);
                let payload_bytes = frame.colors.len().saturating_mul(3);

                match result {
                    Ok(outcome) => {
                        if outcome.is_sent() {
                            metrics_for_task.record_write_success(
                                frame.sequence,
                                queue_wait,
                                write_time,
                                send_completed.saturating_duration_since(frame.produced_at),
                                send_completed,
                                payload_bytes,
                            );
                        } else {
                            metrics_for_task
                                .record_write_suppressed(frame.sequence, send_completed);
                        }
                        last_logged_write_error = None;
                        repeated_write_failures_since_log = 0;
                    }
                    Err(error) => {
                        let error = error.to_string();
                        metrics_for_task.record_write_error(
                            frame.sequence,
                            send_completed,
                            error.clone(),
                        );

                        if last_logged_write_error.as_deref() == Some(error.as_str()) {
                            repeated_write_failures_since_log =
                                repeated_write_failures_since_log.saturating_add(1);
                        } else {
                            last_logged_write_error = Some(error.clone());
                            repeated_write_failures_since_log = 0;
                        }

                        if repeated_write_failures_since_log == 0
                            || repeated_write_failures_since_log
                                >= OUTPUT_WRITE_FAILURE_REPEAT_LOG_INTERVAL
                        {
                            metrics_for_task.record_write_failure_warning();
                            warn!(
                                backend_id = %backend_id,
                                device_id = %device_id,
                                error = %error,
                                suppressed_repeated_failures = repeated_write_failures_since_log,
                                "device output worker write failed"
                            );
                            repeated_write_failures_since_log = 0;
                        } else {
                            trace!(
                                backend_id = %backend_id,
                                device_id = %device_id,
                                error = %error,
                                "suppressed repeated device output worker write failure"
                            );
                        }
                    }
                }

                last_sent_sequence = frame.sequence;

                if let Some(interval) = send_interval {
                    next_send_at = advance_deadline(next_send_at, interval, Instant::now());
                }
            }
        });

        Self {
            tx,
            io_task,
            cadence,
            uses_frame_sink,
            metrics,
            next_sequence,
        }
    }

    pub(super) fn recover(
        self,
        backend_id: String,
        device_id: DeviceId,
        lane: OutputLaneHandle,
        cadence: OutputCadence,
    ) -> Self {
        let initial_payload = self.latest_unconfirmed_payload();
        let metrics = Arc::clone(&self.metrics);
        let next_sequence = self.next_sequence;
        metrics.record_worker_recovery();
        Self::spawn_with_state(
            backend_id,
            device_id,
            lane,
            cadence,
            initial_payload,
            metrics,
            next_sequence,
        )
    }

    fn latest_unconfirmed_payload(&self) -> Option<Arc<FramePayload>> {
        let payload = self.tx.borrow().clone()?;
        let last_success_sequence = self.metrics.last_success_sequence.load(Ordering::Relaxed);
        (payload.sequence > last_success_sequence).then_some(payload)
    }

    pub(super) fn worker_finished(&self) -> bool {
        self.io_task.is_finished()
    }

    pub(super) fn uses_frame_sink(&self) -> bool {
        self.uses_frame_sink
    }

    /// Push the latest payload for this device.
    pub(super) fn push(&mut self, colors: Vec<[u8; 3]>) -> Option<Vec<[u8; 3]>> {
        if self.should_suppress_duplicate(&colors) {
            return Some(colors);
        }

        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        let produced_at = Instant::now();
        self.metrics.record_received(sequence);

        let mut next_colors = Some(Arc::new(colors));
        let mut recycled = None;
        self.tx.send_modify(|current| {
            if let Some(payload) = current.as_mut().and_then(Arc::get_mut) {
                let previous = std::mem::replace(
                    &mut payload.colors,
                    next_colors
                        .take()
                        .expect("pending colors should exist before reuse"),
                );
                recycled = Arc::try_unwrap(previous).ok();
                payload.sequence = sequence;
                payload.produced_at = produced_at;
            } else {
                *current = Some(Arc::new(FramePayload {
                    colors: next_colors
                        .take()
                        .expect("pending colors should exist before allocation"),
                    sequence,
                    produced_at,
                }));
            }
        });

        recycled
    }

    fn should_suppress_duplicate(&self, colors: &[[u8; 3]]) -> bool {
        let current = self.tx.borrow();
        let Some(payload) = current.as_ref() else {
            return false;
        };
        if payload.colors.as_slice() != colors {
            return false;
        }

        let last_success_sequence = self.metrics.last_success_sequence.load(Ordering::Relaxed);
        let last_error_sequence = self.metrics.last_error_sequence.load(Ordering::Relaxed);

        if payload.sequence == last_error_sequence && last_error_sequence > last_success_sequence {
            return false;
        }

        payload.sequence > last_success_sequence
            || payload.sequence == last_success_sequence
                && last_success_sequence >= last_error_sequence
    }

    pub(super) fn retry_latest_after_error(&mut self) -> Option<usize> {
        let current = self.tx.borrow();
        let Some(payload) = current.as_ref() else {
            return None;
        };

        let last_success_sequence = self.metrics.last_success_sequence.load(Ordering::Relaxed);
        let last_error_sequence = self.metrics.last_error_sequence.load(Ordering::Relaxed);
        if payload.sequence != last_error_sequence || last_error_sequence <= last_success_sequence {
            return None;
        }

        let led_count = payload.colors.len();
        drop(current);
        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        let produced_at = Instant::now();
        self.metrics.record_received(sequence);
        self.tx.send_modify(|current| {
            let Some(payload) = current else {
                return;
            };

            if let Some(payload) = Arc::get_mut(payload) {
                payload.sequence = sequence;
                payload.produced_at = produced_at;
                return;
            }

            let colors = payload.colors.clone();
            *current = Some(Arc::new(FramePayload {
                colors,
                sequence,
                produced_at,
            }));
        });
        Some(led_count)
    }

    pub(super) fn statistics(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
    ) -> DeviceOutputStatistics {
        self.metrics.snapshot(
            backend_id,
            device_id,
            mapped_layout_ids,
            self.cadence,
            self.uses_frame_sink,
            self.io_task.is_finished(),
        )
    }

    pub(super) fn last_error(&self) -> Option<String> {
        self.metrics.last_error()
    }
}

impl Drop for OutputQueue {
    fn drop(&mut self) {
        self.io_task.abort();
    }
}

fn average_micros_ms(total_micros: u64, sample_count: u64) -> u64 {
    if sample_count == 0 {
        return 0;
    }

    total_micros
        .checked_div(sample_count)
        .unwrap_or_default()
        .checked_div(1_000)
        .unwrap_or_default()
}

fn duration_micros(duration: Duration) -> u64 {
    let micros = duration.as_micros();
    u64::try_from(micros).unwrap_or(u64::MAX)
}

fn advance_deadline(previous_deadline: Instant, interval: Duration, now: Instant) -> Instant {
    previous_deadline
        .checked_add(interval)
        .unwrap_or(now)
        .max(now)
}
