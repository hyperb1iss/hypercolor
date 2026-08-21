//! Latest-frame output queues for device writes.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use hypercolor_types::device::{DeviceError, DeviceId};
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{trace, warn};

use super::traits::{
    DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceDeliveryStatus, DeviceFrameSink, OutputCadence,
};

type BackendHandle = Arc<dyn DeviceBackend>;
type DeviceFrameSinkHandle = Arc<dyn DeviceFrameSink>;
const OUTPUT_WRITE_FAILURE_REPEAT_LOG_INTERVAL: u64 = 60;
const OUTPUT_REASSERTION_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const WORKER_PHASE_IDLE: u8 = 0;
const WORKER_PHASE_CADENCE: u8 = 1;
const WORKER_PHASE_TRANSPORT: u8 = 2;
static NEXT_QUEUE_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_queue_generation() -> u64 {
    NEXT_QUEUE_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            (generation != 0).then(|| generation.checked_add(1).unwrap_or(0))
        })
        .expect("device output queue generation space exhausted")
}

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

    async fn deliver_colors_shared(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        match self {
            Self::Backend { backend, device_id } => {
                backend
                    .deliver_colors_shared_observed(device_id, id, colors, observer)
                    .await
            }
            Self::FrameSink { frame_sink } => {
                frame_sink
                    .deliver_colors_shared_observed(id, colors, observer)
                    .await
            }
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

    /// Maximum transport silence before cached-payload replay, in milliseconds.
    pub max_frame_silence_ms: Option<u64>,

    /// Whether this queue writes through a per-device hot-path frame sink.
    pub uses_frame_sink: bool,

    /// Whether the queue worker task has finished unexpectedly.
    pub worker_finished: bool,

    /// Total worker tasks replaced after finishing unexpectedly.
    pub worker_recoveries: u64,

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames accepted from the render loop.
    pub accepted: u64,

    /// Total cached payloads queued to restore transport liveness.
    pub cached_payload_reassertions: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

    /// Total delivery attempts that reached transport I/O.
    pub transport_started: u64,

    /// Total delivery attempts completed by the transport.
    pub transport_completed: u64,

    /// Total delivery attempts failed by the lane or transport.
    pub transport_failed: u64,

    /// Total frames intentionally suppressed by the output lane.
    pub frames_suppressed: u64,

    /// Payload bytes successfully written by the worker.
    pub bytes_sent: u64,

    /// Payload bytes completed by the transport.
    pub completed_payload_bytes: u64,

    /// Frames dropped due to latest-frame replacement while I/O was busy.
    pub frames_dropped: u64,

    /// Total pending frames superseded by newer accepted frames.
    pub coalesced: u64,

    /// Frames superseded while intentionally pacing to the target cadence.
    pub coalesced_target_cadence: u64,

    /// Frames superseded while transport or its worker was behind.
    pub coalesced_backend_overrun: u64,

    /// Average latency from enqueue to write completion.
    pub avg_latency_ms: u64,

    /// Average time spent waiting in the latest-frame slot before a write starts.
    pub avg_queue_wait_ms: u64,

    /// Average backend write duration from write start to write completion.
    pub avg_write_ms: u64,

    /// Average actual transport duration from actor start to terminal ack.
    pub avg_transport_latency_ms: u64,

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

    /// Generation qualifying every delivery sequence in this snapshot.
    pub queue_generation: u64,

    /// Most recent sequence acknowledged as transport-started.
    pub last_transport_started_sequence: u64,

    /// Most recent sequence acknowledged as transport-completed.
    pub last_transport_completed_sequence: u64,

    /// Most recent sequence acknowledged as transport-failed.
    pub last_transport_failed_sequence: u64,
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

    /// Maximum transport silence before cached-payload replay, in milliseconds.
    pub max_frame_silence_ms: Option<u64>,

    /// Whether this queue writes through a per-device hot-path frame sink.
    pub uses_frame_sink: bool,

    /// Whether the queue worker task has finished unexpectedly.
    pub worker_finished: bool,

    /// Total worker tasks replaced after finishing unexpectedly.
    pub worker_recoveries: u64,

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames accepted from the render loop.
    pub accepted: u64,

    /// Total cached payloads queued to restore transport liveness.
    pub cached_payload_reassertions: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

    /// Total delivery attempts that reached transport I/O.
    pub transport_started: u64,

    /// Total delivery attempts completed by the transport.
    pub transport_completed: u64,

    /// Total delivery attempts failed by the lane or transport.
    pub transport_failed: u64,

    /// Total frames intentionally suppressed by the output lane.
    pub frames_suppressed: u64,

    /// Payload bytes successfully written by the worker.
    pub bytes_sent: u64,

    /// Payload bytes completed by the transport.
    pub completed_payload_bytes: u64,

    /// Frames dropped due to latest-frame replacement while I/O was busy.
    pub frames_dropped: u64,

    /// Total pending frames superseded by newer accepted frames.
    pub coalesced: u64,

    /// Frames superseded while intentionally pacing to the target cadence.
    pub coalesced_target_cadence: u64,

    /// Frames superseded while transport or its worker was behind.
    pub coalesced_backend_overrun: u64,

    /// Average latency from enqueue to write completion.
    pub avg_latency_ms: u64,

    /// Average time spent waiting in the latest-frame slot before a write starts.
    pub avg_queue_wait_ms: u64,

    /// Average backend write duration from write start to write completion.
    pub avg_write_ms: u64,

    /// Average actual transport duration from actor start to terminal ack.
    pub avg_transport_latency_ms: u64,

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

    /// Generation qualifying every delivery sequence in this snapshot.
    pub queue_generation: u64,

    /// Most recent sequence acknowledged as transport-started.
    pub last_transport_started_sequence: u64,

    /// Most recent sequence acknowledged as transport-completed.
    pub last_transport_completed_sequence: u64,

    /// Most recent sequence acknowledged as transport-failed.
    pub last_transport_failed_sequence: u64,
}

impl DeviceOutputStatistics {
    pub(super) fn into_debug_snapshot(self) -> OutputQueueDebugSnapshot {
        OutputQueueDebugSnapshot {
            backend_id: self.backend_id,
            device_id: self.device_id.to_string(),
            mapped_layout_ids: self.mapped_layout_ids,
            target_fps: self.target_fps,
            target_interval_ms: self.target_interval_ms,
            max_frame_silence_ms: self.max_frame_silence_ms,
            uses_frame_sink: self.uses_frame_sink,
            worker_finished: self.worker_finished,
            worker_recoveries: self.worker_recoveries,
            frames_received: self.frames_received,
            accepted: self.accepted,
            cached_payload_reassertions: self.cached_payload_reassertions,
            frames_sent: self.frames_sent,
            transport_started: self.transport_started,
            transport_completed: self.transport_completed,
            transport_failed: self.transport_failed,
            frames_suppressed: self.frames_suppressed,
            bytes_sent: self.bytes_sent,
            completed_payload_bytes: self.completed_payload_bytes,
            frames_dropped: self.frames_dropped,
            coalesced: self.coalesced,
            coalesced_target_cadence: self.coalesced_target_cadence,
            coalesced_backend_overrun: self.coalesced_backend_overrun,
            avg_latency_ms: self.avg_latency_ms,
            avg_queue_wait_ms: self.avg_queue_wait_ms,
            avg_write_ms: self.avg_write_ms,
            avg_transport_latency_ms: self.avg_transport_latency_ms,
            last_error: self.last_error,
            errors_total: self.errors_total,
            write_failure_warnings_total: self.write_failure_warnings_total,
            last_sent_ago_ms: self.last_sent_ago_ms,
            last_sequence: self.last_sequence,
            queue_generation: self.queue_generation,
            last_transport_started_sequence: self.last_transport_started_sequence,
            last_transport_completed_sequence: self.last_transport_completed_sequence,
            last_transport_failed_sequence: self.last_transport_failed_sequence,
        }
    }
}

/// One queue-qualified async device write failure.
#[derive(Clone)]
pub struct AsyncWriteFailure {
    /// Backend ID that owns the queue.
    pub backend_id: String,
    /// Physical device ID targeted by the queue.
    pub device_id: DeviceId,
    /// Queue generation and delivery sequence that failed.
    pub delivery_id: DeviceDeliveryId,
    /// Most recent typed async write error.
    pub error: DeviceError,
    fence: std::sync::Weak<AsyncWriteFailureFence>,
}

impl AsyncWriteFailure {
    /// Whether this exact failure is still the queue's current terminal state.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.fence
            .upgrade()
            .is_some_and(|fence| fence.is_current(self.delivery_id))
    }

    /// Atomically acknowledge this failure if it is still current.
    ///
    /// The acknowledgement is the recovery linearization point. A newer
    /// success or queue generation makes the ticket stale.
    #[must_use]
    pub fn try_acknowledge(&self) -> bool {
        self.fence
            .upgrade()
            .is_some_and(|fence| fence.try_acknowledge(self.delivery_id))
    }
}

impl std::fmt::Debug for AsyncWriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AsyncWriteFailure")
            .field("backend_id", &self.backend_id)
            .field("device_id", &self.device_id)
            .field("delivery_id", &self.delivery_id)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AsyncWriteFailure {
    fn eq(&self, other: &Self) -> bool {
        self.backend_id == other.backend_id
            && self.device_id == other.device_id
            && self.delivery_id == other.delivery_id
            && self.error == other.error
    }
}

impl Eq for AsyncWriteFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetainedWriteFailure {
    id: DeviceDeliveryId,
    error: DeviceError,
    acknowledged: bool,
}

#[derive(Debug)]
struct AsyncWriteFailureFence {
    active: AtomicBool,
    metrics: Arc<OutputQueueMetrics>,
}

impl AsyncWriteFailureFence {
    fn is_current(&self, id: DeviceDeliveryId) -> bool {
        self.active.load(Ordering::Acquire) && self.metrics.is_failure_current(id)
    }

    fn try_acknowledge(&self, id: DeviceDeliveryId) -> bool {
        self.active.load(Ordering::Acquire)
            && self.metrics.try_acknowledge_failure(id)
            && self.active.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct OutputQueueMetrics {
    started_at: Instant,
    active_generation: AtomicU64,
    accepted: AtomicU64,
    cached_payload_reassertions: AtomicU64,
    frames_received: AtomicU64,
    frames_sent: AtomicU64,
    frames_suppressed: AtomicU64,
    worker_recoveries: AtomicU64,
    bytes_sent: AtomicU64,
    frames_dropped: AtomicU64,
    coalesced_target_cadence: AtomicU64,
    coalesced_backend_overrun: AtomicU64,
    transport_started: AtomicU64,
    transport_failed: AtomicU64,
    total_latency_us: AtomicU64,
    total_queue_wait_us: AtomicU64,
    total_write_time_us: AtomicU64,
    errors_total: AtomicU64,
    write_failure_warnings_total: AtomicU64,
    last_sent_offset_us: AtomicU64,
    last_sequence: AtomicU64,
    last_transport_started_sequence: AtomicU64,
    last_handled_sequence: AtomicU64,
    last_success_sequence: AtomicU64,
    last_error_sequence: AtomicU64,
    retained_failure: StdMutex<Option<RetainedWriteFailure>>,
}

impl OutputQueueMetrics {
    fn new(started_at: Instant, generation: u64) -> Self {
        Self {
            started_at,
            active_generation: AtomicU64::new(generation),
            accepted: AtomicU64::new(0),
            cached_payload_reassertions: AtomicU64::new(0),
            frames_received: AtomicU64::new(0),
            frames_sent: AtomicU64::new(0),
            frames_suppressed: AtomicU64::new(0),
            worker_recoveries: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            coalesced_target_cadence: AtomicU64::new(0),
            coalesced_backend_overrun: AtomicU64::new(0),
            transport_started: AtomicU64::new(0),
            transport_failed: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            total_queue_wait_us: AtomicU64::new(0),
            total_write_time_us: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            write_failure_warnings_total: AtomicU64::new(0),
            last_sent_offset_us: AtomicU64::new(0),
            last_sequence: AtomicU64::new(0),
            last_transport_started_sequence: AtomicU64::new(0),
            last_handled_sequence: AtomicU64::new(0),
            last_success_sequence: AtomicU64::new(0),
            last_error_sequence: AtomicU64::new(0),
            retained_failure: StdMutex::new(None),
        }
    }

    fn activate_generation(&self, generation: u64) {
        let mut retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.active_generation.store(generation, Ordering::Release);
        self.last_sequence.store(0, Ordering::Relaxed);
        self.last_transport_started_sequence
            .store(0, Ordering::Relaxed);
        self.last_handled_sequence.store(0, Ordering::Relaxed);
        self.last_success_sequence.store(0, Ordering::Relaxed);
        self.last_error_sequence.store(0, Ordering::Relaxed);
        *retained_failure = None;
    }

    fn is_current(&self, id: DeviceDeliveryId) -> bool {
        self.active_generation.load(Ordering::Acquire) == id.queue_generation
    }

    fn record_accepted(&self, id: DeviceDeliveryId) {
        if !self.is_current(id) {
            return;
        }
        self.accepted.fetch_add(1, Ordering::Relaxed);
        self.last_sequence.store(id.sequence, Ordering::Relaxed);
    }

    fn record_received(&self, id: DeviceDeliveryId) {
        if self.is_current(id) {
            self.frames_received.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_cached_payload_reassertion(&self, id: DeviceDeliveryId) {
        if !self.is_current(id) {
            return;
        }
        self.cached_payload_reassertions
            .fetch_add(1, Ordering::Relaxed);
        self.last_sequence.store(id.sequence, Ordering::Relaxed);
    }

    fn record_coalesced(&self, id: DeviceDeliveryId, phase: u8) {
        if !self.is_current(id) {
            return;
        }
        let counter = if phase == WORKER_PHASE_CADENCE {
            &self.coalesced_target_cadence
        } else {
            &self.coalesced_backend_overrun
        };
        counter.fetch_add(1, Ordering::Relaxed);
        self.frames_dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn record_write_success(
        &self,
        id: DeviceDeliveryId,
        queue_wait: Duration,
        transport_latency: Duration,
        total_latency: Duration,
        sent_at: Instant,
        completed_payload_bytes: u64,
    ) {
        let mut retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current(id) {
            return;
        }
        self.frames_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent
            .fetch_add(completed_payload_bytes, Ordering::Relaxed);
        self.total_queue_wait_us
            .fetch_add(duration_micros(queue_wait), Ordering::Relaxed);
        self.total_write_time_us
            .fetch_add(duration_micros(transport_latency), Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(duration_micros(total_latency), Ordering::Relaxed);
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.last_success_sequence
            .store(id.sequence, Ordering::Relaxed);
        self.last_handled_sequence
            .store(id.sequence, Ordering::Release);
        if retained_failure.as_ref().is_some_and(|failure| {
            failure.id.queue_generation == id.queue_generation && failure.id.sequence <= id.sequence
        }) {
            *retained_failure = None;
        }
    }

    fn record_write_suppressed(&self, id: DeviceDeliveryId, sent_at: Instant) {
        let mut retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current(id) {
            return;
        }
        self.frames_suppressed.fetch_add(1, Ordering::Relaxed);
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.last_handled_sequence
            .store(id.sequence, Ordering::Release);
        if retained_failure.as_ref().is_some_and(|failure| {
            failure.id.queue_generation == id.queue_generation && failure.id.sequence <= id.sequence
        }) {
            *retained_failure = None;
        }
    }

    fn record_accepted_duplicate(&self, id: DeviceDeliveryId) {
        if self.is_current(id) {
            self.frames_suppressed.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_write_error(&self, id: DeviceDeliveryId, sent_at: Instant, error: DeviceError) {
        let mut retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current(id) {
            return;
        }
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.errors_total.fetch_add(1, Ordering::Relaxed);
        self.transport_failed.fetch_add(1, Ordering::Relaxed);
        self.last_error_sequence
            .store(id.sequence, Ordering::Release);
        if id.sequence > self.last_handled_sequence.load(Ordering::Acquire) {
            *retained_failure = Some(RetainedWriteFailure {
                id,
                error,
                acknowledged: false,
            });
        }
    }

    fn record_delivery_ack(
        &self,
        ack: &DeviceDeliveryAck,
        queue_wait: Duration,
        total_latency: Duration,
        completed_at: Instant,
    ) {
        if !self.is_current(ack.id) {
            return;
        }
        if ack.transport_started {
            self.record_transport_started(ack.id);
        }

        match ack.status {
            DeviceDeliveryStatus::Completed => self.record_write_success(
                ack.id,
                queue_wait,
                ack.transport_latency,
                total_latency,
                completed_at,
                ack.completed_payload_bytes,
            ),
            DeviceDeliveryStatus::SuppressedDuplicate | DeviceDeliveryStatus::SuppressedCadence => {
                self.record_write_suppressed(ack.id, completed_at);
            }
            DeviceDeliveryStatus::Failed => self.record_write_error(
                ack.id,
                completed_at,
                ack.error.clone().unwrap_or_else(|| {
                    DeviceError::protocol("output queue", "delivery failed without an error")
                }),
            ),
        }
    }

    fn record_transport_started(&self, id: DeviceDeliveryId) {
        if !self.is_current(id) {
            return;
        }
        let previous = self
            .last_transport_started_sequence
            .fetch_max(id.sequence, Ordering::AcqRel);
        if previous < id.sequence {
            self.transport_started.fetch_add(1, Ordering::Relaxed);
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
        let accepted = self.accepted.load(Ordering::Relaxed);
        let frames_sent = self.frames_sent.load(Ordering::Relaxed);
        let frames_suppressed = self.frames_suppressed.load(Ordering::Relaxed);
        let bytes_sent = self.bytes_sent.load(Ordering::Relaxed);
        let frames_dropped = self.frames_dropped.load(Ordering::Relaxed);
        let transport_started = self.transport_started.load(Ordering::Relaxed);
        let transport_failed = self.transport_failed.load(Ordering::Relaxed);
        let coalesced_target_cadence = self.coalesced_target_cadence.load(Ordering::Relaxed);
        let coalesced_backend_overrun = self.coalesced_backend_overrun.load(Ordering::Relaxed);
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
        let last_error = self
            .retained_failure()
            .map(|failure| failure.error.to_string());

        DeviceOutputStatistics {
            backend_id: backend_id.to_owned(),
            device_id,
            mapped_layout_ids,
            target_fps: cadence.target_fps(),
            target_interval_ms: cadence.interval_ms(),
            max_frame_silence_ms: cadence.max_frame_silence_ms(),
            uses_frame_sink,
            worker_finished,
            worker_recoveries: self.worker_recoveries.load(Ordering::Relaxed),
            frames_received,
            accepted,
            cached_payload_reassertions: self.cached_payload_reassertions.load(Ordering::Relaxed),
            frames_sent,
            transport_started,
            transport_completed: frames_sent,
            transport_failed,
            frames_suppressed,
            bytes_sent,
            completed_payload_bytes: bytes_sent,
            frames_dropped,
            coalesced: frames_dropped,
            coalesced_target_cadence,
            coalesced_backend_overrun,
            avg_latency_ms,
            avg_queue_wait_ms,
            avg_write_ms,
            avg_transport_latency_ms: avg_write_ms,
            last_error,
            errors_total: self.errors_total.load(Ordering::Relaxed),
            write_failure_warnings_total: self.write_failure_warnings_total.load(Ordering::Relaxed),
            last_sent_ago_ms,
            last_sequence: self.last_sequence.load(Ordering::Relaxed),
            queue_generation: self.active_generation.load(Ordering::Relaxed),
            last_transport_started_sequence: self
                .last_transport_started_sequence
                .load(Ordering::Relaxed),
            last_transport_completed_sequence: self.last_success_sequence.load(Ordering::Relaxed),
            last_transport_failed_sequence: self.last_error_sequence.load(Ordering::Relaxed),
        }
    }

    fn retained_failure(&self) -> Option<RetainedWriteFailure> {
        let retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retained_failure
            .as_ref()
            .filter(|failure| {
                self.is_current(failure.id)
                    && self.last_error_sequence.load(Ordering::Acquire) == failure.id.sequence
            })
            .cloned()
    }

    fn pending_failure(&self) -> Option<RetainedWriteFailure> {
        self.retained_failure()
            .filter(|failure| !failure.acknowledged && self.failure_matches(failure.id))
    }

    fn is_failure_current(&self, id: DeviceDeliveryId) -> bool {
        let retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retained_failure.as_ref().is_some_and(|failure| {
            failure.id == id && !failure.acknowledged && self.failure_matches(id)
        })
    }

    fn try_acknowledge_failure(&self, id: DeviceDeliveryId) -> bool {
        let mut retained_failure = self
            .retained_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(failure) = retained_failure.as_mut() else {
            return false;
        };
        if failure.id != id || failure.acknowledged || !self.failure_matches(id) {
            return false;
        }

        self.last_handled_sequence
            .fetch_max(id.sequence, Ordering::AcqRel);
        failure.acknowledged = true;
        true
    }

    fn failure_matches(&self, id: DeviceDeliveryId) -> bool {
        self.is_current(id)
            && self.last_error_sequence.load(Ordering::Acquire) == id.sequence
            && self.last_handled_sequence.load(Ordering::Acquire) < id.sequence
    }
}

impl DeviceDeliveryObserver for OutputQueueMetrics {
    fn transport_started(&self, id: DeviceDeliveryId) {
        self.record_transport_started(id);
    }
}

// ── OutputQueue ─────────────────────────────────────────────────────────────

/// Frame payload queued for asynchronous backend writes.
#[derive(Debug, Clone)]
struct FramePayload {
    /// LED colors for the target device.
    colors: Arc<Vec<[u8; 3]>>,
    /// Monotonic sequence for dropped-frame diagnostics.
    id: DeviceDeliveryId,
    /// Timestamp when this payload was queued by the render loop.
    produced_at: Instant,
    /// Whether the worker queued this payload from its liveness cache.
    cached_reassertion: bool,
}

#[derive(Debug)]
struct DeliverySequence {
    generation: u64,
    next_sequence: u64,
}

impl DeliverySequence {
    const fn new(generation: u64, next_sequence: u64) -> Self {
        Self {
            generation,
            next_sequence,
        }
    }

    fn next(&mut self) -> (DeviceDeliveryId, bool) {
        let generation_changed = if let Some(sequence) = self.next_sequence.checked_add(1) {
            self.next_sequence = sequence;
            false
        } else {
            self.generation = next_queue_generation();
            self.next_sequence = 1;
            true
        };

        (
            DeviceDeliveryId {
                queue_generation: self.generation,
                sequence: self.next_sequence,
            },
            generation_changed,
        )
    }
}

fn requeue_cached_payload(
    tx: &watch::Sender<Option<Arc<FramePayload>>>,
    delivery_sequence: &StdMutex<DeliverySequence>,
    metrics: &OutputQueueMetrics,
) -> bool {
    let mut delivery_sequence = delivery_sequence
        .lock()
        .expect("device delivery sequence lock should not be poisoned");
    let current = tx.borrow();
    let Some(payload) = current.as_ref() else {
        return false;
    };
    let last_terminal_sequence = metrics
        .last_handled_sequence
        .load(Ordering::Relaxed)
        .max(metrics.last_error_sequence.load(Ordering::Relaxed));
    if payload.id.sequence > last_terminal_sequence {
        return false;
    }
    drop(current);

    let (id, generation_changed) = delivery_sequence.next();
    if generation_changed {
        metrics.activate_generation(id.queue_generation);
    }
    metrics.record_cached_payload_reassertion(id);

    let produced_at = Instant::now();
    tx.send_modify(|current| {
        let Some(payload) = current else {
            return;
        };

        if let Some(payload) = Arc::get_mut(payload) {
            payload.id = id;
            payload.produced_at = produced_at;
            payload.cached_reassertion = true;
            return;
        }

        let colors = Arc::clone(&payload.colors);
        *payload = Arc::new(FramePayload {
            colors,
            id,
            produced_at,
            cached_reassertion: true,
        });
    });
    true
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
    delivery_sequence: Arc<StdMutex<DeliverySequence>>,
    worker_phase: Arc<AtomicU8>,
    active_sequence: Arc<AtomicU64>,
    failure_fence: Arc<AsyncWriteFailureFence>,
}

impl OutputQueue {
    /// Spawn an output worker for one physical target.
    pub(super) fn spawn(
        backend_id: String,
        device_id: DeviceId,
        lane: OutputLaneHandle,
        cadence: OutputCadence,
    ) -> Self {
        let generation = next_queue_generation();
        let metrics = Arc::new(OutputQueueMetrics::new(Instant::now(), generation));
        Self::spawn_with_state(
            backend_id, device_id, lane, cadence, None, metrics, generation, 0,
        )
    }

    fn spawn_with_state(
        backend_id: String,
        device_id: DeviceId,
        lane: OutputLaneHandle,
        cadence: OutputCadence,
        initial_payload: Option<Arc<FramePayload>>,
        metrics: Arc<OutputQueueMetrics>,
        generation: u64,
        next_sequence: u64,
    ) -> Self {
        metrics.activate_generation(generation);
        let initial_payload = initial_payload.map(|payload| {
            Arc::new(FramePayload {
                colors: Arc::clone(&payload.colors),
                id: DeviceDeliveryId {
                    queue_generation: generation,
                    sequence: payload.id.sequence,
                },
                produced_at: payload.produced_at,
                cached_reassertion: payload.cached_reassertion,
            })
        });
        let (tx, mut rx) = watch::channel(initial_payload);
        let delivery_sequence = Arc::new(StdMutex::new(DeliverySequence::new(
            generation,
            next_sequence,
        )));
        let delivery_sequence_for_task = Arc::clone(&delivery_sequence);
        let tx_for_task = tx.clone();
        let metrics_for_task = Arc::clone(&metrics);
        let worker_phase = Arc::new(AtomicU8::new(WORKER_PHASE_IDLE));
        let phase_for_task = Arc::clone(&worker_phase);
        let active_sequence = Arc::new(AtomicU64::new(0));
        let active_sequence_for_task = Arc::clone(&active_sequence);
        let uses_frame_sink = lane.uses_frame_sink();
        let failure_fence = Arc::new(AsyncWriteFailureFence {
            active: AtomicBool::new(true),
            metrics: Arc::clone(&metrics),
        });
        let io_task = tokio::spawn(async move {
            let send_interval = cadence.min_interval();
            let max_frame_silence = cadence.max_frame_silence();
            let mut next_send_at = Instant::now();
            let mut reassert_at = None::<Instant>;
            let mut pending = rx.borrow_and_update().clone();
            let mut last_logged_write_error = None::<String>;
            let mut repeated_write_failures_since_log = 0_u64;

            'worker: loop {
                if pending.is_none() {
                    if let Some(deadline) = reassert_at {
                        tokio::select! {
                            changed = rx.changed() => {
                                if changed.is_err() {
                                    break;
                                }
                                pending.clone_from(&rx.borrow_and_update());
                            }
                            () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                                if requeue_cached_payload(
                                    &tx_for_task,
                                    delivery_sequence_for_task.as_ref(),
                                    metrics_for_task.as_ref(),
                                ) {
                                    pending.clone_from(&rx.borrow_and_update());
                                }
                                reassert_at = None;
                            }
                        }
                    } else {
                        // Sender dropped => manager shutdown or queue removed.
                        if rx.changed().await.is_err() {
                            break;
                        }
                        pending.clone_from(&rx.borrow_and_update());
                    }
                    continue;
                }

                if send_interval.is_some() {
                    phase_for_task.store(WORKER_PHASE_CADENCE, Ordering::Release);
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
                phase_for_task.store(WORKER_PHASE_TRANSPORT, Ordering::Release);
                active_sequence_for_task.store(frame.id.sequence, Ordering::Release);
                let ack = lane
                    .deliver_colors_shared(
                        frame.id,
                        Arc::clone(&frame.colors),
                        metrics_for_task.clone(),
                    )
                    .await;
                let send_completed = Instant::now();
                active_sequence_for_task.store(0, Ordering::Release);
                phase_for_task.store(WORKER_PHASE_IDLE, Ordering::Release);
                metrics_for_task.record_delivery_ack(
                    &ack,
                    queue_wait,
                    send_completed.saturating_duration_since(frame.produced_at),
                    send_completed,
                );

                match ack.status {
                    DeviceDeliveryStatus::Completed
                    | DeviceDeliveryStatus::SuppressedDuplicate
                    | DeviceDeliveryStatus::SuppressedCadence => {
                        last_logged_write_error = None;
                        repeated_write_failures_since_log = 0;
                    }
                    DeviceDeliveryStatus::Failed => {
                        let error = ack.error.clone().unwrap_or_else(|| {
                            DeviceError::protocol(device_id, "delivery failed without an error")
                        });
                        let error_text = error.to_string();

                        if last_logged_write_error.as_deref() == Some(error_text.as_str()) {
                            repeated_write_failures_since_log =
                                repeated_write_failures_since_log.saturating_add(1);
                        } else {
                            last_logged_write_error = Some(error_text);
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

                if let Some(max_frame_silence) = max_frame_silence {
                    let next_interval = match ack.status {
                        DeviceDeliveryStatus::Completed | DeviceDeliveryStatus::Failed => {
                            max_frame_silence
                        }
                        DeviceDeliveryStatus::SuppressedDuplicate
                        | DeviceDeliveryStatus::SuppressedCadence => send_interval
                            .filter(|interval| !interval.is_zero())
                            .unwrap_or(OUTPUT_REASSERTION_RETRY_INTERVAL)
                            .min(max_frame_silence),
                    };
                    reassert_at = Some(advance_deadline(
                        send_completed,
                        next_interval,
                        send_completed,
                    ));
                }

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
            delivery_sequence,
            worker_phase,
            active_sequence,
            failure_fence,
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
        let next_sequence = self
            .delivery_sequence
            .lock()
            .expect("device delivery sequence lock should not be poisoned")
            .next_sequence;
        let generation = next_queue_generation();
        metrics.record_worker_recovery();
        Self::spawn_with_state(
            backend_id,
            device_id,
            lane,
            cadence,
            initial_payload,
            metrics,
            generation,
            next_sequence,
        )
    }

    fn latest_unconfirmed_payload(&self) -> Option<Arc<FramePayload>> {
        let payload = self.tx.borrow().clone()?;
        let last_handled_sequence = self.metrics.last_handled_sequence.load(Ordering::Relaxed);
        (payload.id.sequence > last_handled_sequence).then_some(payload)
    }

    pub(super) fn worker_finished(&self) -> bool {
        self.io_task.is_finished()
    }

    pub(super) fn uses_frame_sink(&self) -> bool {
        self.uses_frame_sink
    }

    /// Push the latest payload for this device.
    pub(super) fn push(&mut self, colors: Vec<[u8; 3]>) -> Option<Vec<[u8; 3]>> {
        let mut delivery_sequence = self
            .delivery_sequence
            .lock()
            .expect("device delivery sequence lock should not be poisoned");
        let (id, generation_changed) = delivery_sequence.next();
        if generation_changed {
            self.metrics.activate_generation(id.queue_generation);
            let _ = self.tx.send_replace(None);
        }
        self.metrics.record_accepted(id);

        if self.should_suppress_duplicate(&colors) {
            self.metrics.record_accepted_duplicate(id);
            return Some(colors);
        }
        self.metrics.record_received(id);

        let produced_at = Instant::now();

        let mut next_colors = Some(Arc::new(colors));
        let mut recycled = None;
        self.tx.send_modify(|current| {
            if let Some(previous) = current.as_ref() {
                let active_sequence = self.active_sequence.load(Ordering::Acquire);
                let last_terminal_sequence = self
                    .metrics
                    .last_handled_sequence
                    .load(Ordering::Relaxed)
                    .max(self.metrics.last_error_sequence.load(Ordering::Relaxed));
                if !previous.cached_reassertion
                    && previous.id.sequence != active_sequence
                    && previous.id.sequence > last_terminal_sequence
                {
                    let phase = self.worker_phase.load(Ordering::Acquire);
                    self.metrics.record_coalesced(previous.id, phase);
                    trace!(
                        queue_generation = previous.id.queue_generation,
                        sequence = previous.id.sequence,
                        phase,
                        "coalescing pending device frame"
                    );
                }
            }
            if let Some(payload) = current.as_mut().and_then(Arc::get_mut) {
                let previous = std::mem::replace(
                    &mut payload.colors,
                    next_colors
                        .take()
                        .expect("pending colors should exist before reuse"),
                );
                recycled = Arc::try_unwrap(previous).ok();
                payload.id = id;
                payload.produced_at = produced_at;
                payload.cached_reassertion = false;
            } else {
                *current = Some(Arc::new(FramePayload {
                    colors: next_colors
                        .take()
                        .expect("pending colors should exist before allocation"),
                    id,
                    produced_at,
                    cached_reassertion: false,
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

        let last_handled_sequence = self.metrics.last_handled_sequence.load(Ordering::Relaxed);
        let last_error_sequence = self.metrics.last_error_sequence.load(Ordering::Relaxed);

        if payload.id.sequence == last_error_sequence && last_error_sequence > last_handled_sequence
        {
            return false;
        }

        payload.id.sequence > last_handled_sequence
            || payload.id.sequence == last_handled_sequence
                && last_handled_sequence >= last_error_sequence
    }

    pub(super) fn retry_latest_after_error(&mut self) -> Option<usize> {
        let mut delivery_sequence = self
            .delivery_sequence
            .lock()
            .expect("device delivery sequence lock should not be poisoned");
        let current = self.tx.borrow();
        let Some(payload) = current.as_ref() else {
            return None;
        };

        let last_handled_sequence = self.metrics.last_handled_sequence.load(Ordering::Relaxed);
        let last_error_sequence = self.metrics.last_error_sequence.load(Ordering::Relaxed);
        if payload.id.sequence != last_error_sequence
            || last_error_sequence <= last_handled_sequence
        {
            return None;
        }

        let led_count = payload.colors.len();
        let cached_reassertion = payload.cached_reassertion;
        drop(current);
        let (id, generation_changed) = delivery_sequence.next();
        if generation_changed {
            self.metrics.activate_generation(id.queue_generation);
            let _ = self.tx.send_replace(None);
        }
        let produced_at = Instant::now();
        if cached_reassertion {
            self.metrics.record_cached_payload_reassertion(id);
        } else {
            self.metrics.record_accepted(id);
            self.metrics.record_received(id);
        }
        self.tx.send_modify(|current| {
            let Some(payload) = current else {
                return;
            };

            if let Some(payload) = Arc::get_mut(payload) {
                payload.id = id;
                payload.produced_at = produced_at;
                return;
            }

            let colors = payload.colors.clone();
            let cached_reassertion = payload.cached_reassertion;
            *current = Some(Arc::new(FramePayload {
                colors,
                id,
                produced_at,
                cached_reassertion,
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

    pub(super) fn async_write_failure(
        &self,
        backend_id: String,
        device_id: DeviceId,
    ) -> Option<AsyncWriteFailure> {
        let failure = self.metrics.pending_failure()?;
        Some(AsyncWriteFailure {
            backend_id,
            device_id,
            delivery_id: failure.id,
            error: failure.error,
            fence: Arc::downgrade(&self.failure_fence),
        })
    }
}

impl Drop for OutputQueue {
    fn drop(&mut self) {
        self.failure_fence.active.store(false, Ordering::Release);
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
