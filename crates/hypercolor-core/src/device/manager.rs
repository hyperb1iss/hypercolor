//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_types::canvas::{linear_to_output_u8, srgb_to_linear};
use hypercolor_types::device::{DeviceId, DeviceInfo, ZoneInfo};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout, ZoneAttachment};

use super::traits::DeviceBackend;

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type BackendDeviceKey = (String, DeviceId);
const UNMAPPED_LAYOUT_WARN_INTERVAL: Duration = Duration::from_secs(5);
const LED_PERCEPTUAL_COMPENSATION_STRENGTH: f32 = 0.22;
const LED_NEUTRAL_COMPENSATION_WEIGHT: f32 = 0.25;
const LED_HEADROOM_WEIGHT_FLOOR: f32 = 0.1;

/// Lightweight handle for backend I/O that can outlive the manager lock.
///
/// Clone this from [`BackendManager::backend_io`] while holding the manager
/// briefly, then perform the awaited backend call after releasing the outer
/// manager mutex.
#[derive(Clone)]
pub struct BackendIo {
    backend_id: String,
    backend: BackendHandle,
}

impl BackendIo {
    /// Connect a device, retrying once after cleanup and backend discovery refresh.
    ///
    /// Returns the backend's preferred output FPS for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails both before and
    /// after discovery refresh.
    pub async fn connect_with_refresh(&self, device_id: DeviceId) -> Result<u32> {
        let mut backend = self.backend.lock().await;

        if let Err(initial_error) = backend.connect(&device_id).await {
            let initial_message = initial_error.to_string();
            debug!(
                backend_id = %self.backend_id,
                %device_id,
                error = %initial_message,
                "initial connect failed; refreshing backend discovery state and retrying"
            );

            match backend.disconnect(&device_id).await {
                Ok(()) => debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    "best-effort cleanup after failed connect completed"
                ),
                Err(cleanup_error) => debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %cleanup_error,
                    "best-effort cleanup after failed connect could not release an existing session"
                ),
            }

            backend.discover().await.with_context(|| {
                format!(
                    "backend '{}' discovery refresh failed after initial connect failure for device {device_id}: {initial_message}",
                    self.backend_id
                )
            })?;

            if let Err(retry_error) = backend.connect(&device_id).await {
                let retry_message = retry_error.to_string();
                debug!(
                    backend_id = %self.backend_id,
                    %device_id,
                    error = %retry_message,
                    "connect still failing after discovery refresh"
                );
                return Err(retry_error).with_context(|| {
                    format!(
                        "failed to connect device {device_id} using backend '{}' after discovery refresh (initial error: {initial_message})",
                        self.backend_id
                    )
                });
            }

            debug!(
                backend_id = %self.backend_id,
                %device_id,
                "connect succeeded after discovery refresh"
            );
        }

        Ok(backend.target_fps(&device_id).unwrap_or(60))
    }

    /// Fetch refreshed metadata for a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata retrieval fails.
    pub async fn connected_device_info(&self, device_id: DeviceId) -> Result<Option<DeviceInfo>> {
        let backend = self.backend.lock().await;
        backend
            .connected_device_info(&device_id)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch connected device metadata for {device_id} using backend '{}'",
                    self.backend_id
                )
            })
    }

    /// Disconnect a device from the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend disconnect call fails.
    pub async fn disconnect(&self, device_id: DeviceId) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend.disconnect(&device_id).await.with_context(|| {
            format!(
                "failed to disconnect device {device_id} using backend '{}'",
                self.backend_id
            )
        })
    }

    /// Write immediate LED colors directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend write fails.
    pub async fn write_colors(&self, device_id: DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_colors(&device_id, colors)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} colors to device {device_id} using backend '{}'",
                    colors.len(),
                    self.backend_id
                )
            })
    }

    /// Write immediate display bytes directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame(&self, device_id: DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    jpeg_data.len(),
                    self.backend_id
                )
            })
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame_owned(
        &self,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let byte_len = jpeg_data.len();
        let mut backend = self.backend.lock().await;
        backend
            .write_display_frame_owned(&device_id, jpeg_data)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} display bytes to device {device_id} using backend '{}'",
                    byte_len, self.backend_id
                )
            })
    }
}

/// Contiguous LED range on a physical device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRange {
    /// Inclusive start LED index.
    pub start: usize,
    /// Number of LEDs in this range.
    pub length: usize,
}

impl SegmentRange {
    /// Create a new range.
    #[must_use]
    pub const fn new(start: usize, length: usize) -> Self {
        Self { start, length }
    }

    /// Exclusive end LED index.
    #[must_use]
    pub const fn end(self) -> usize {
        self.start.saturating_add(self.length)
    }
}

// ── Debug Snapshot ──────────────────────────────────────────────────────────

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

    /// Total frames accepted from the render loop.
    pub frames_received: u64,

    /// Total frames successfully written by the worker.
    pub frames_sent: u64,

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

    /// Milliseconds since last worker write attempt.
    pub last_sent_ago_ms: Option<u64>,

    /// Most recent frame sequence seen by this queue.
    pub last_sequence: u64,
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
    frames_dropped: AtomicU64,
    total_latency_us: AtomicU64,
    total_queue_wait_us: AtomicU64,
    total_write_time_us: AtomicU64,
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
            frames_dropped: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            total_queue_wait_us: AtomicU64::new(0),
            total_write_time_us: AtomicU64::new(0),
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
    ) {
        self.frames_sent.fetch_add(1, Ordering::Relaxed);
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

    fn record_write_error(&self, sequence: u64, sent_at: Instant, error: String) {
        self.last_sent_offset_us.store(
            duration_micros(sent_at.saturating_duration_since(self.started_at)),
            Ordering::Relaxed,
        );
        self.last_error_sequence.store(sequence, Ordering::Relaxed);
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = Some(error);
        }
    }

    fn snapshot(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
        target_fps: u32,
    ) -> OutputQueueDebugSnapshot {
        let frames_received = self.frames_received.load(Ordering::Relaxed);
        let frames_sent = self.frames_sent.load(Ordering::Relaxed);
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

        OutputQueueDebugSnapshot {
            backend_id: backend_id.to_owned(),
            device_id: device_id.to_string(),
            mapped_layout_ids,
            target_fps,
            frames_received,
            frames_sent,
            frames_dropped,
            avg_latency_ms,
            avg_queue_wait_ms,
            avg_write_ms,
            last_error,
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
    colors: Vec<[u8; 3]>,
    /// Monotonic sequence for dropped-frame diagnostics.
    sequence: u64,
    /// Timestamp when this payload was queued by the render loop.
    produced_at: Instant,
}

#[derive(Debug, Default)]
struct DeviceStagingBuffer {
    output: Vec<[u8; 3]>,
    remap_scratch: Vec<[u8; 3]>,
    written_ranges: Vec<Range<usize>>,
    has_segmented_write: bool,
    required_len: usize,
    frame_generation: u64,
}

impl DeviceStagingBuffer {
    fn mark_written_range(&mut self, start: usize, end: usize) {
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
struct OutputQueue {
    tx: watch::Sender<Option<Arc<FramePayload>>>,
    _io_task: JoinHandle<()>,
    target_fps: u32,
    metrics: Arc<OutputQueueMetrics>,
    next_sequence: u64,
}

impl OutputQueue {
    /// Spawn an output worker for one physical target.
    fn spawn(
        backend_id: String,
        device_id: DeviceId,
        backend: BackendHandle,
        target_fps: u32,
    ) -> Self {
        let (tx, mut rx) = watch::channel(None::<Arc<FramePayload>>);
        let metrics = Arc::new(OutputQueueMetrics::new(Instant::now()));
        let metrics_for_task = Arc::clone(&metrics);

        let io_task = tokio::spawn(async move {
            let send_interval = target_interval_for_fps(target_fps);
            let mut next_send_at = Instant::now();
            let mut last_sent_sequence = 0_u64;
            let mut pending = None::<Arc<FramePayload>>;

            loop {
                if pending.is_none() {
                    // Sender dropped => manager shutdown or queue removed.
                    if rx.changed().await.is_err() {
                        break;
                    }
                    pending.clone_from(&rx.borrow_and_update());
                    continue;
                }

                if send_interval.is_some() {
                    tokio::select! {
                        changed = rx.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            pending.clone_from(&rx.borrow_and_update());
                            continue;
                        }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_send_at)) => {}
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

                let result = {
                    let mut backend = backend.lock().await;
                    backend.write_colors(&device_id, &frame.colors).await
                };
                let send_completed = Instant::now();
                let write_time = send_completed.saturating_duration_since(write_started);

                match &result {
                    Ok(()) => metrics_for_task.record_write_success(
                        frame.sequence,
                        queue_wait,
                        write_time,
                        send_completed.saturating_duration_since(frame.produced_at),
                        send_completed,
                    ),
                    Err(error) => metrics_for_task.record_write_error(
                        frame.sequence,
                        send_completed,
                        error.to_string(),
                    ),
                }

                if let Err(error) = result {
                    warn!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        error = %error,
                        "device output worker write failed"
                    );
                }

                last_sent_sequence = frame.sequence;

                if let Some(interval) = send_interval {
                    next_send_at = advance_deadline(next_send_at, interval, Instant::now());
                }
            }
        });

        Self {
            tx,
            _io_task: io_task,
            target_fps,
            metrics,
            next_sequence: 0,
        }
    }

    /// Push the latest payload for this device.
    fn push(&mut self, colors: Vec<[u8; 3]>) -> Option<Vec<[u8; 3]>> {
        if self.should_suppress_duplicate(&colors) {
            return Some(colors);
        }

        self.next_sequence = self.next_sequence.saturating_add(1);
        let sequence = self.next_sequence;
        let produced_at = Instant::now();
        self.metrics.record_received(sequence);

        let mut next_colors = Some(colors);
        let mut recycled = None;
        self.tx.send_modify(|current| {
            if let Some(payload) = current.as_mut().and_then(Arc::get_mut) {
                recycled = Some(std::mem::replace(
                    &mut payload.colors,
                    next_colors
                        .take()
                        .expect("pending colors should exist before reuse"),
                ));
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

    fn retry_latest_after_error(&mut self) -> Option<usize> {
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
        let colors = payload.colors.clone();
        drop(current);
        let _ = self.push(colors);
        Some(led_count)
    }

    fn snapshot(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
    ) -> OutputQueueDebugSnapshot {
        self.metrics
            .snapshot(backend_id, device_id, mapped_layout_ids, self.target_fps)
    }
}

// ── BackendManager ──────────────────────────────────────────────────────────

/// Routes per-zone color data to the correct device backends.
///
/// On each frame, [`write_frame`](Self::write_frame) groups zone colors
/// by target device (using the spatial layout mapping) and dispatches
/// one payload per device to a non-blocking output queue.
#[derive(Default)]
pub struct BackendManager {
    /// Registered backends, keyed by `BackendInfo.id` (e.g., `"wled"`, `"usb"`).
    backends: HashMap<String, BackendHandle>,

    /// Maps spatial layout `DeviceZone.device_id` strings to `(backend_id, DeviceId)`.
    ///
    /// Populated during device discovery/connection. Entries are added via
    /// [`map_device`](Self::map_device) when a zone's device reference is
    /// resolved to an actual connected device.
    device_map: HashMap<String, DeviceMapping>,

    /// Per-target latest-frame output queues.
    output_queues: HashMap<BackendDeviceKey, OutputQueue>,

    /// Reusable per-device color staging for steady-state frame routing.
    device_staging: HashMap<BackendDeviceKey, DeviceStagingBuffer>,

    /// Device staging keys touched during recent frames.
    active_staging_keys: Vec<BackendDeviceKey>,
    active_staging_len: usize,

    /// Monotonic frame generation for staging reset bookkeeping.
    staging_generation: u64,

    /// Preferred output FPS for connected devices, captured at connect time.
    device_fps_cache: HashMap<BackendDeviceKey, u32>,

    /// User-configured per-device output brightness scalar.
    device_brightness: HashMap<DeviceId, f32>,

    /// Incremented whenever software output brightness state changes.
    device_brightness_generation: u64,

    /// Reference-counted direct-control locks that suppress queued frame writes.
    direct_control_locks: HashMap<BackendDeviceKey, usize>,

    /// Layout device IDs already warned as unmapped in the current layout state.
    warned_unmapped_layout_devices: HashSet<String>,

    /// Number of unmapped-layout warnings emitted since process start.
    unmapped_layout_warning_count: u64,

    /// Last warning time for zone-to-segment color length mismatches.
    last_segment_mismatch_warn_at: HashMap<String, Instant>,

    /// Connected devices already reported as unused by the active layout.
    warned_inactive_layout_devices: HashSet<BackendDeviceKey>,

    /// Incremented whenever routing-relevant device mappings change.
    routing_mapping_generation: u64,

    /// Number of times the cached routing plan has been rebuilt.
    routing_plan_rebuild_count: u64,

    /// Cached routing metadata for the current layout + mapping generation.
    routing_plan: Option<Arc<RoutingPlan>>,
}

/// Internal mapping from a layout device identifier to a backend + device.
#[derive(Debug, Clone)]
struct DeviceMapping {
    backend_id: String,
    device_id: DeviceId,
    segment: Option<SegmentRange>,
    zone_segments: HashMap<String, SegmentRange>,
    physical_led_count: Option<usize>,
}

#[derive(Debug)]
struct RoutingPlan {
    layout_signature: u64,
    mapping_generation: u64,
    active_layout_device_ids: HashSet<String>,
    active_target_keys: Vec<BackendDeviceKey>,
    zone_routes: HashMap<String, PlannedZoneRoute>,
    ordered_zone_routes: Vec<OrderedZoneRoute>,
    inactive_devices: Vec<BackendDeviceKey>,
    mapped_layout_ids_by_device: HashMap<BackendDeviceKey, Vec<String>>,
}

#[derive(Debug, Clone)]
enum PlannedZoneRoute {
    Mapped(CompiledZoneRoute),
    Unmapped { layout_device_id: String },
}

#[derive(Debug, Clone)]
struct CompiledZoneRoute {
    layout_device_id: String,
    target_key: BackendDeviceKey,
    led_mapping: Option<Box<[u32]>>,
    segment: Option<SegmentRange>,
    attachment: Option<ZoneAttachment>,
    physical_led_count: Option<usize>,
}

#[derive(Debug, Clone)]
struct OrderedZoneRoute {
    zone_id: String,
    route: PlannedZoneRoute,
}

// ── FrameWriteStats ─────────────────────────────────────────────────────────

/// Statistics from a single frame's device push.
#[derive(Debug, Clone, Default)]
pub struct FrameWriteStats {
    /// Number of devices that received color data.
    pub devices_written: usize,

    /// Total LEDs written across all devices.
    pub total_leds: usize,

    /// Errors encountered during writes (non-fatal — every device still gets its data).
    pub errors: Vec<String>,
}

impl BackendManager {
    /// Create an empty backend manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn invalidate_routing_plan(&mut self) {
        self.routing_mapping_generation = self.routing_mapping_generation.saturating_add(1);
        self.routing_plan = None;
    }

    /// Register a device backend. Uses `backend.info().id` as the key.
    ///
    /// Replaces any existing backend with the same ID.
    pub fn register_backend(&mut self, backend: Box<dyn DeviceBackend>) {
        let info = backend.info();
        let backend_id = info.id.clone();

        debug!(
            backend_id = %backend_id,
            name = %info.name,
            "registered device backend"
        );

        // If a backend gets replaced, drop all output queues bound to that ID.
        // They are lazily recreated on the next frame.
        self.output_queues
            .retain(|(queued_backend_id, _), _| queued_backend_id != &backend_id);
        self.device_staging
            .retain(|(staged_backend_id, _), _| staged_backend_id != &backend_id);
        self.device_fps_cache
            .retain(|(cached_backend_id, _), _| cached_backend_id != &backend_id);
        self.direct_control_locks
            .retain(|(locked_backend_id, _), _| locked_backend_id != &backend_id);
        self.warned_inactive_layout_devices
            .retain(|(warn_backend_id, _)| warn_backend_id != &backend_id);

        self.backends
            .insert(backend_id, Arc::new(Mutex::new(backend)));
    }

    /// Clone a backend I/O handle without holding the manager across awaits.
    #[must_use]
    pub fn backend_io(&self, backend_id: &str) -> Option<BackendIo> {
        self.backends
            .get(backend_id)
            .cloned()
            .map(|backend| BackendIo {
                backend_id: backend_id.to_owned(),
                backend,
            })
    }

    /// Map a spatial layout `device_id` to a `(backend_id, DeviceId)` pair.
    ///
    /// Call this after device discovery to link a zone's device reference
    /// to an actual connected device.
    pub fn map_device(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
    ) {
        self.map_device_with_segment(layout_device_id, backend_id, device_id, None);
    }

    /// Map a spatial layout `device_id` with an explicit physical LED range.
    ///
    /// When `segment` is `Some`, zone colors targeting this mapping are
    /// written into that slice of the physical output buffer.
    pub fn map_device_with_segment(
        &mut self,
        layout_device_id: impl Into<String>,
        backend_id: impl Into<String>,
        device_id: DeviceId,
        segment: Option<SegmentRange>,
    ) {
        let layout_id = layout_device_id.into();
        let backend = backend_id.into();
        debug!(
            layout_device_id = %layout_id,
            backend_id = %backend,
            %device_id,
            segment_start = segment.map(|s| s.start),
            segment_length = segment.map(|s| s.length),
            "mapped device"
        );
        self.warned_unmapped_layout_devices.remove(&layout_id);
        self.device_map.insert(
            layout_id,
            DeviceMapping {
                backend_id: backend,
                device_id,
                segment,
                zone_segments: HashMap::new(),
                physical_led_count: None,
            },
        );
        self.invalidate_routing_plan();
    }

    /// Attach hardware zone segment metadata to an existing layout-device mapping.
    ///
    /// This lets spatial zones that share one `device_id` but differ by
    /// `zone_name` target the correct physical LED ranges on multi-zone devices.
    #[must_use]
    pub fn set_device_zone_segments(
        &mut self,
        layout_device_id: &str,
        device_info: &DeviceInfo,
    ) -> bool {
        {
            let Some(mapping) = self.device_map.get_mut(layout_device_id) else {
                return false;
            };

            mapping.zone_segments = zone_segments_from_device_info(device_info);
            mapping.physical_led_count = device_output_len(device_info);
        }
        self.invalidate_routing_plan();
        true
    }

    /// Connect a physical device and map it to a layout device identifier.
    ///
    /// This keeps connect + map as a single operation so discovery/lifecycle
    /// code can avoid split-brain states.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or the backend connect call
    /// fails.
    pub async fn connect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        let target_fps = io.connect_with_refresh(device_id).await?;
        self.device_fps_cache
            .insert((backend_id.to_owned(), device_id), target_fps);

        self.map_device(
            layout_device_id.to_owned(),
            backend_id.to_owned(),
            device_id,
        );
        Ok(())
    }

    /// Query refreshed metadata for a connected physical device.
    ///
    /// Backends can use this to expose connect-time topology discovery back
    /// to the daemon after a successful handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or metadata retrieval fails.
    pub async fn connected_device_info(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Result<Option<DeviceInfo>> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.connected_device_info(device_id).await
    }

    /// Disconnect a physical device and unmap its layout device identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or disconnect fails.
    pub async fn disconnect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        _layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.disconnect(device_id).await?;

        let _ = self.remove_device_mappings_for_physical(backend_id, device_id);
        Ok(())
    }

    /// Write one immediate color payload to a specific physical device.
    ///
    /// This bypasses spatial routing and output queues, and is intended for
    /// short, direct control operations like identify/flash actions.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_colors(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        colors: &[[u8; 3]],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_colors(device_id, colors).await
    }

    /// Adjust hardware brightness for a specific physical device.
    ///
    /// This bypasses spatial routing and targets the backend directly.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn set_device_brightness(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        brightness: u8,
    ) -> Result<()> {
        let Some(backend) = self.backends.get(backend_id).cloned() else {
            bail!("backend '{backend_id}' is not registered");
        };

        let mut backend = backend.lock().await;
        backend
            .set_brightness(&device_id, brightness)
            .await
            .with_context(|| {
                format!(
                    "failed to set brightness {brightness} on device {device_id} using backend '{backend_id}'"
                )
            })
    }

    /// Configure software output brightness for a physical device.
    pub fn set_device_output_brightness(&mut self, device_id: DeviceId, brightness: f32) {
        let normalized = brightness.clamp(0.0, 1.0);
        let changed = if normalized >= 0.999 {
            self.device_brightness.remove(&device_id).is_some()
        } else {
            self.device_brightness
                .insert(device_id, normalized)
                .is_none_or(|previous| previous.to_bits() != normalized.to_bits())
        };
        if changed {
            self.device_brightness_generation = self.device_brightness_generation.saturating_add(1);
        }
    }

    /// Read the configured software output brightness for a physical device.
    #[must_use]
    pub fn device_output_brightness(&self, device_id: DeviceId) -> f32 {
        self.device_brightness
            .get(&device_id)
            .copied()
            .unwrap_or(1.0)
    }

    /// Monotonic generation for software output-brightness state changes.
    #[must_use]
    pub fn output_brightness_generation(&self) -> u64 {
        self.device_brightness_generation
    }

    /// Write one immediate JPEG display payload to a specific physical device.
    ///
    /// This bypasses spatial routing and targets display-capable backends
    /// directly for screen/LCD updates.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: &[u8],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame(device_id, jpeg_data).await
    }

    /// Write one owned JPEG display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame_owned(device_id, jpeg_data).await
    }

    /// Cache a backend-provided output FPS for a physical device.
    pub fn set_cached_target_fps(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        target_fps: u32,
    ) {
        self.device_fps_cache
            .insert((backend_id.to_owned(), device_id), target_fps);
    }

    /// Suppress queued frame writes for a specific physical device.
    ///
    /// Returns the active direct-control lock count after incrementing.
    pub fn begin_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        let key = (backend_id.to_owned(), device_id);
        let count = self.direct_control_locks.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// Release one direct-control lock for a specific physical device.
    ///
    /// Returns the remaining lock count after decrementing.
    pub fn end_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        let key = (backend_id.to_owned(), device_id);
        let Some(count) = self.direct_control_locks.get_mut(&key) else {
            return 0;
        };

        *count = count.saturating_sub(1);
        let remaining = *count;
        if remaining == 0 {
            self.direct_control_locks.remove(&key);
        }

        remaining
    }

    /// Whether queued frame writes are currently suppressed for a device.
    #[must_use]
    pub fn is_direct_control_active(&self, backend_id: &str, device_id: DeviceId) -> bool {
        self.is_direct_control_active_key(&(backend_id.to_owned(), device_id))
    }

    fn is_direct_control_active_key(&self, key: &BackendDeviceKey) -> bool {
        self.direct_control_locks
            .get(key)
            .is_some_and(|count| *count > 0)
    }

    /// Remove a device mapping.
    pub fn unmap_device(&mut self, layout_device_id: &str) -> bool {
        let Some(mapping) = self.device_map.remove(layout_device_id) else {
            return false;
        };

        // If no other mapping targets this physical device, tear down its queue.
        let still_used = self.device_map.values().any(|candidate| {
            candidate.backend_id == mapping.backend_id && candidate.device_id == mapping.device_id
        });

        if !still_used {
            let key = (mapping.backend_id, mapping.device_id);
            self.remove_device_target_state(&key);
        }

        self.invalidate_routing_plan();
        true
    }

    /// Remove all mappings for a physical target and drop its queue.
    pub fn remove_device_mappings_for_physical(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> usize {
        let before = self.device_map.len();
        self.device_map.retain(|_, mapping| {
            !(mapping.backend_id == backend_id && mapping.device_id == device_id)
        });
        let removed = before.saturating_sub(self.device_map.len());

        if !self
            .device_map
            .values()
            .any(|mapping| mapping.backend_id == backend_id && mapping.device_id == device_id)
        {
            let key = (backend_id.to_owned(), device_id);
            self.remove_device_target_state(&key);
        }

        if removed > 0 {
            self.invalidate_routing_plan();
        }
        removed
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn routing_plan_rebuild_count(&self) -> u64 {
        self.routing_plan_rebuild_count
    }

    #[doc(hidden)]
    #[must_use]
    pub fn ordered_routing_zone_count(&mut self, layout: &SpatialLayout) -> usize {
        self.routing_plan(layout).ordered_zone_routes.len()
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn unmapped_layout_warning_count(&self) -> u64 {
        self.unmapped_layout_warning_count
    }

    /// List registered backend IDs.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<&str> {
        self.backends.keys().map(String::as_str).collect()
    }

    /// Number of registered backends.
    #[must_use]
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Number of mapped devices.
    #[must_use]
    pub fn mapped_device_count(&self) -> usize {
        self.device_map.len()
    }

    /// Physical devices that are connected and mapped, but not referenced by the active layout.
    ///
    /// This is a diagnostic view over the current routing table. Devices in this list
    /// will not receive queued frame writes until a layout zone targets one of their aliases.
    #[doc(hidden)]
    #[must_use]
    pub fn connected_devices_without_layout_targets(
        &self,
        layout: &SpatialLayout,
    ) -> Vec<(String, DeviceId)> {
        let targeted = layout
            .zones
            .iter()
            .filter_map(|zone| {
                self.device_map
                    .get(zone.device_id.as_str())
                    .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            })
            .collect::<HashSet<_>>();

        let mut inactive = self
            .device_map
            .values()
            .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|key| !targeted.contains(key))
            .collect::<Vec<_>>();

        inactive.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });
        inactive
    }

    fn routing_plan(&mut self, layout: &SpatialLayout) -> Arc<RoutingPlan> {
        let layout_signature = layout_routing_signature(layout);
        let needs_rebuild = self.routing_plan.as_ref().is_none_or(|plan| {
            plan.layout_signature != layout_signature
                || plan.mapping_generation != self.routing_mapping_generation
        });

        if needs_rebuild {
            let plan = Arc::new(self.compile_routing_plan(layout, layout_signature));
            self.routing_plan = Some(Arc::clone(&plan));
            self.routing_plan_rebuild_count = self.routing_plan_rebuild_count.saturating_add(1);
            return plan;
        }

        Arc::clone(
            self.routing_plan
                .as_ref()
                .expect("routing plan should exist when cache is valid"),
        )
    }

    fn compile_routing_plan(&self, layout: &SpatialLayout, layout_signature: u64) -> RoutingPlan {
        let mut active_layout_device_ids = HashSet::with_capacity(layout.zones.len());
        let mut active_target_keys = HashSet::with_capacity(layout.zones.len());
        let mut zone_routes = HashMap::with_capacity(layout.zones.len());
        let mut ordered_zone_routes = Vec::with_capacity(layout.zones.len());

        for zone in &layout.zones {
            active_layout_device_ids.insert(zone.device_id.clone());

            let route = if let Some(mapping) = self.device_map.get(zone.device_id.as_str()) {
                let target_key = (mapping.backend_id.clone(), mapping.device_id);
                active_target_keys.insert(target_key.clone());
                PlannedZoneRoute::Mapped(CompiledZoneRoute {
                    layout_device_id: zone.device_id.clone(),
                    target_key,
                    led_mapping: normalized_led_mapping(zone.led_mapping.as_deref()),
                    segment: mapped_segment_for_zone_name(
                        &zone.id,
                        zone.zone_name.as_deref(),
                        mapping,
                    ),
                    attachment: zone.attachment.clone(),
                    physical_led_count: mapping.physical_led_count,
                })
            } else {
                PlannedZoneRoute::Unmapped {
                    layout_device_id: zone.device_id.clone(),
                }
            };

            if should_use_ordered_routing(zone) {
                ordered_zone_routes.push(OrderedZoneRoute {
                    zone_id: zone.id.clone(),
                    route: route.clone(),
                });
            }
            zone_routes.insert(zone.id.clone(), route);
        }

        let mut mapped_layout_ids_by_device: HashMap<BackendDeviceKey, Vec<String>> =
            HashMap::new();
        for (layout_device_id, mapping) in &self.device_map {
            mapped_layout_ids_by_device
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .push(layout_device_id.clone());
        }
        for ids in mapped_layout_ids_by_device.values_mut() {
            ids.sort_unstable();
        }

        let mut inactive_devices = mapped_layout_ids_by_device
            .keys()
            .filter(|key| !active_target_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        inactive_devices.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });

        RoutingPlan {
            layout_signature,
            mapping_generation: self.routing_mapping_generation,
            active_layout_device_ids,
            active_target_keys: {
                let mut active_target_keys = active_target_keys.into_iter().collect::<Vec<_>>();
                active_target_keys.sort_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
                });
                active_target_keys
            },
            zone_routes,
            ordered_zone_routes,
            inactive_devices,
            mapped_layout_ids_by_device,
        }
    }

    fn begin_staging_frame(&mut self) {
        self.staging_generation = self.staging_generation.saturating_add(1);
        self.active_staging_len = 0;
    }

    fn staging_buffer(&mut self, key: &BackendDeviceKey) -> &mut DeviceStagingBuffer {
        let generation = self.staging_generation;
        let mut became_active = false;

        if let Some(staging) = self.device_staging.get_mut(key) {
            if staging.frame_generation != generation {
                staging.output.clear();
                staging.required_len = 0;
                staging.written_ranges.clear();
                staging.has_segmented_write = false;
                staging.frame_generation = generation;
                became_active = true;
            }
        } else {
            let staging = self.device_staging.entry(key.clone()).or_default();
            staging.output.clear();
            staging.required_len = 0;
            staging.written_ranges.clear();
            staging.has_segmented_write = false;
            staging.frame_generation = generation;
            became_active = true;
        }

        if became_active {
            if self.active_staging_len < self.active_staging_keys.len() {
                self.active_staging_keys[self.active_staging_len].clone_from(key);
            } else {
                self.active_staging_keys.push(key.clone());
            }
            self.active_staging_len += 1;
        }

        self.device_staging
            .get_mut(key)
            .expect("staging buffer must exist after entry initialization")
    }

    fn remove_device_target_state(&mut self, key: &BackendDeviceKey) {
        self.output_queues.remove(key);
        self.device_staging.remove(key);
        self.device_fps_cache.remove(key);
        self.direct_control_locks.remove(key);
        self.warned_inactive_layout_devices.remove(key);
    }

    /// Push frame color data to all mapped devices.
    ///
    /// For each zone in `zone_colors`, looks up the target device via the
    /// spatial layout's zone-to-device mapping, groups colors by device,
    /// and enqueues one payload per device. Errors are
    /// collected but do not halt processing — every mapped device gets
    /// its data.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, and queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
    ) -> FrameWriteStats {
        self.write_frame_with_brightness(zone_colors, layout, 1.0, None)
            .await
    }

    /// Push frame color data to all mapped devices with optional per-device
    /// output brightness scalars.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::too_many_lines,
        reason = "frame routing keeps mapping, remap, segmented writes, queue dispatch together so the hot-path ordering stays readable"
    )]
    pub async fn write_frame_with_brightness(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
        global_brightness: f32,
        device_brightness: Option<&HashMap<DeviceId, f32>>,
    ) -> FrameWriteStats {
        self.begin_staging_frame();
        let plan = self.routing_plan(layout);
        self.warned_unmapped_layout_devices
            .retain(|layout_device_id| plan.active_layout_device_ids.contains(layout_device_id));

        let mut stats = FrameWriteStats::default();

        let newly_inactive = plan
            .inactive_devices
            .iter()
            .filter(|key| !self.warned_inactive_layout_devices.contains(*key))
            .cloned()
            .collect::<Vec<_>>();

        if !newly_inactive.is_empty() {
            let devices = newly_inactive
                .iter()
                .map(|(backend_id, device_id)| format!("{backend_id}:{device_id}"))
                .collect::<Vec<_>>();
            let mapped_layout_ids_by_device = newly_inactive
                .iter()
                .map(|(backend_id, device_id)| {
                    let aliases = plan
                        .mapped_layout_ids_by_device
                        .get(&(backend_id.clone(), *device_id))
                        .cloned()
                        .unwrap_or_default();
                    format!("{backend_id}:{device_id} => [{}]", aliases.join(", "))
                })
                .collect::<Vec<_>>();
            warn!(
                inactive_device_count = devices.len(),
                devices = ?devices,
                layout_zone_count = layout.zones.len(),
                mapped_layout_ids = ?mapped_layout_ids_by_device,
                "connected devices have no active layout zones; frames will not be sent"
            );
        }
        self.warned_inactive_layout_devices.clear();
        self.warned_inactive_layout_devices
            .extend(plan.inactive_devices.iter().cloned());

        if zone_colors.len() == plan.ordered_zone_routes.len()
            && zone_colors
                .iter()
                .zip(&plan.ordered_zone_routes)
                .all(|(zone_colors, ordered)| zone_colors.zone_id == ordered.zone_id)
        {
            for (zone_colors, ordered) in zone_colors.iter().zip(&plan.ordered_zone_routes) {
                self.route_zone_colors(
                    zone_colors.zone_id.as_str(),
                    &zone_colors.colors,
                    &ordered.route,
                );
            }
        } else {
            for zc in zone_colors {
                let Some(route) = plan.zone_routes.get(zc.zone_id.as_str()) else {
                    warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                    continue;
                };
                self.route_zone_colors(&zc.zone_id, &zc.colors, route);
            }
        }

        let active_staging_len = self.active_staging_len;
        let mut active_staging_keys = Vec::new();
        std::mem::swap(&mut active_staging_keys, &mut self.active_staging_keys);
        self.active_staging_len = 0;

        for key in active_staging_keys.iter().take(active_staging_len) {
            let (backend_id, device_id) = key;

            if self.is_direct_control_active_key(key) {
                trace!(
                    backend_id = %backend_id,
                    device_id = %device_id,
                    "skipping queued device frame while direct control is active"
                );
                continue;
            }

            if !self.backends.contains_key(backend_id.as_str()) {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            }

            let device_output_brightness = self.device_output_brightness(*device_id);
            let per_frame_brightness = device_brightness
                .and_then(|settings| settings.get(device_id).copied())
                .unwrap_or(1.0);
            let brightness = (global_brightness * per_frame_brightness * device_output_brightness)
                .clamp(0.0, 1.0);

            let values = {
                let staging = self
                    .device_staging
                    .get_mut(key)
                    .expect("active staging key should resolve to a staging buffer");
                if staging.output.len() < staging.required_len {
                    staging.output.resize(staging.required_len, [0, 0, 0]);
                }

                prepare_output_for_led_ranges(
                    &mut staging.output,
                    &staging.written_ranges,
                    brightness,
                );

                let mut values = Vec::new();
                std::mem::swap(&mut values, &mut staging.output);
                values
            };

            let Some(queue) = self.ensure_output_queue_for_key(key) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                if let Some(staging) = self.device_staging.get_mut(key) {
                    staging.output = values;
                }
                continue;
            };

            stats.devices_written += 1;
            stats.total_leds += values.len();
            let recycled = queue.push(values);
            if let (Some(staging), Some(recycled)) = (self.device_staging.get_mut(key), recycled) {
                staging.output = recycled;
            }
        }

        self.active_staging_keys = active_staging_keys;

        stats
    }

    /// Whether existing queued outputs can be reused for the active layout.
    ///
    /// Returns `true` when every active physical target already has an output
    /// queue, so a retained frame can skip re-routing and reuse the latest
    /// queued payloads.
    #[must_use]
    pub fn can_reuse_routed_frame_outputs(&mut self, layout: &SpatialLayout) -> bool {
        let plan = self.routing_plan(layout);
        plan.active_target_keys.iter().all(|key| {
            self.is_direct_control_active_key(key) || self.output_queues.contains_key(key)
        })
    }

    /// Reuse the latest routed outputs for the active layout.
    ///
    /// This only nudges queues that need a retry after an asynchronous write
    /// failure; successfully queued or in-flight identical payloads are left
    /// untouched.
    pub fn reuse_routed_frame_outputs(&mut self, layout: &SpatialLayout) -> FrameWriteStats {
        let plan = self.routing_plan(layout);
        let mut stats = FrameWriteStats::default();

        for key in &plan.active_target_keys {
            if self.is_direct_control_active_key(key) {
                continue;
            }

            let Some(queue) = self.output_queues.get_mut(key) else {
                continue;
            };
            if let Some(led_count) = queue.retry_latest_after_error() {
                stats.devices_written = stats.devices_written.saturating_add(1);
                stats.total_leds = stats.total_leds.saturating_add(led_count);
            }
        }

        stats
    }

    fn route_zone_colors(&mut self, zone_id: &str, colors: &[[u8; 3]], route: &PlannedZoneRoute) {
        let PlannedZoneRoute::Mapped(route) = route else {
            let PlannedZoneRoute::Unmapped { layout_device_id } = route else {
                unreachable!("only mapped or unmapped zone routes are compiled");
            };
            if self
                .warned_unmapped_layout_devices
                .insert(layout_device_id.clone())
            {
                self.unmapped_layout_warning_count =
                    self.unmapped_layout_warning_count.saturating_add(1);
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %layout_device_id,
                    "zone skipped because the target layout device is not mapped to a connected backend device"
                );
            }
            return;
        };

        self.warned_unmapped_layout_devices
            .remove(route.layout_device_id.as_str());

        let segment = attachment_segment_for_zone(
            zone_id,
            route.segment,
            route.attachment.as_ref(),
            colors.len(),
        );
        let mismatch = {
            let staging = self.staging_buffer(&route.target_key);
            staging.required_len = staging
                .required_len
                .max(route.physical_led_count.unwrap_or_default());
            let remapped_colors = remap_zone_colors(
                zone_id,
                colors,
                route.led_mapping.as_deref(),
                &mut staging.remap_scratch,
            );
            let remapped_len = remapped_colors.len();

            if let Some(segment) = segment {
                staging.has_segmented_write = true;
                let segment_end = segment.end();
                if staging.output.len() < segment_end {
                    staging.output.resize(segment_end, [0, 0, 0]);
                }

                let copy_len = segment.length.min(remapped_len);
                if copy_len > 0 {
                    let start = segment.start;
                    let end = start.saturating_add(copy_len);
                    staging.output[start..end].copy_from_slice(&remapped_colors[..copy_len]);
                    staging.mark_written_range(start, end);
                }

                (copy_len != segment.length).then_some((
                    segment.start,
                    segment.length,
                    remapped_len,
                ))
            } else {
                if staging.has_segmented_write {
                    warn!(
                        zone_id = %zone_id,
                        "mixed segmented and non-segmented mappings for the same physical device"
                    );
                }
                let start = staging.output.len();
                staging.output.extend_from_slice(remapped_colors);
                staging.mark_written_range(start, staging.output.len());
                None
            }
        };

        if let Some((segment_start, expected, received)) = mismatch {
            let warn_key = format!("{}:{zone_id}", route.layout_device_id);
            let should_warn = self
                .last_segment_mismatch_warn_at
                .get(&warn_key)
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= UNMAPPED_LAYOUT_WARN_INTERVAL);

            if should_warn {
                warn!(
                    zone_id = %zone_id,
                    layout_device_id = %route.layout_device_id,
                    segment_start,
                    expected,
                    received,
                    "zone color count does not match mapped segment length"
                );
                self.last_segment_mismatch_warn_at
                    .insert(warn_key, Instant::now());
            }
        }
    }

    fn ensure_output_queue_for_key(&mut self, key: &BackendDeviceKey) -> Option<&mut OutputQueue> {
        if !self.output_queues.contains_key(key) {
            let backend = self.backends.get(key.0.as_str())?.clone();
            let target_fps = self.device_fps_cache.get(key).copied().unwrap_or(60);
            let queue = OutputQueue::spawn(key.0.clone(), key.1, backend, target_fps);
            self.output_queues.insert(key.clone(), queue);
        }

        self.output_queues.get_mut(key)
    }

    /// Return the cached target FPS for a connected physical device, if present.
    #[must_use]
    pub fn cached_target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.device_fps_cache
            .get(&(backend_id.to_owned(), device_id))
            .copied()
    }

    /// Snapshot async write failures currently retained by output queues.
    #[must_use]
    pub fn async_write_failures(&self) -> Vec<AsyncWriteFailure> {
        let mut failures = self
            .output_queues
            .iter()
            .filter_map(|((backend_id, device_id), queue)| {
                let error = queue.metrics.last_error()?;

                Some(AsyncWriteFailure {
                    backend_id: backend_id.clone(),
                    device_id: *device_id,
                    error,
                })
            })
            .collect::<Vec<_>>();

        failures.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
        });
        failures
    }

    /// Build a debug snapshot of queue and routing internals.
    #[must_use]
    pub fn debug_snapshot(&self) -> BackendManagerDebugSnapshot {
        let mut layout_ids_by_key: HashMap<BackendDeviceKey, Vec<String>> = HashMap::new();
        for (layout_device_id, mapping) in &self.device_map {
            layout_ids_by_key
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .push(layout_device_id.clone());
        }

        for ids in layout_ids_by_key.values_mut() {
            ids.sort_unstable();
        }

        let mut queues = Vec::with_capacity(self.output_queues.len());
        for ((backend_id, device_id), queue) in &self.output_queues {
            let mapped_layout_ids = layout_ids_by_key
                .get(&(backend_id.clone(), *device_id))
                .cloned()
                .unwrap_or_default();
            queues.push(queue.snapshot(backend_id, *device_id, mapped_layout_ids));
        }

        queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.cmp(&right.device_id))
        });

        BackendManagerDebugSnapshot {
            queue_count: queues.len(),
            mapped_device_count: self.device_map.len(),
            queues,
        }
    }

    /// Build a routing-focused debug snapshot (layout IDs -> backend targets).
    #[must_use]
    pub fn routing_snapshot(&self) -> BackendRoutingDebugSnapshot {
        let mut backend_ids = self.backends.keys().cloned().collect::<Vec<_>>();
        backend_ids.sort_unstable();

        let mapped_keys = self
            .device_map
            .values()
            .map(|mapping| (mapping.backend_id.clone(), mapping.device_id))
            .collect::<std::collections::HashSet<_>>();

        let mut mappings = self
            .device_map
            .iter()
            .map(|(layout_device_id, mapping)| {
                let key = (mapping.backend_id.clone(), mapping.device_id);
                LayoutRoutingDebugEntry {
                    layout_device_id: layout_device_id.clone(),
                    backend_id: mapping.backend_id.clone(),
                    device_id: mapping.device_id.to_string(),
                    backend_registered: self.backends.contains_key(&mapping.backend_id),
                    queue_active: self.output_queues.contains_key(&key),
                }
            })
            .collect::<Vec<_>>();
        mappings.sort_by(|left, right| left.layout_device_id.cmp(&right.layout_device_id));

        let mut orphaned_queues = self
            .output_queues
            .keys()
            .filter(|key| !mapped_keys.contains(*key))
            .map(|(backend_id, device_id)| OrphanedQueueDebugEntry {
                backend_id: backend_id.clone(),
                device_id: device_id.to_string(),
            })
            .collect::<Vec<_>>();
        orphaned_queues.sort_by(|left, right| {
            left.backend_id
                .cmp(&right.backend_id)
                .then(left.device_id.cmp(&right.device_id))
        });

        BackendRoutingDebugSnapshot {
            backend_ids,
            mapping_count: self.device_map.len(),
            queue_count: self.output_queues.len(),
            mappings,
            orphaned_queues,
        }
    }
}

fn prepare_output_for_led_ranges(
    colors: &mut [[u8; 3]],
    written_ranges: &[Range<usize>],
    brightness: f32,
) {
    let brightness = brightness.clamp(0.0, 1.0);
    let full_brightness = brightness >= 0.999;
    if brightness <= 0.0 {
        colors.fill([0, 0, 0]);
        return;
    }

    if written_ranges.is_empty() {
        return;
    }

    if written_ranges.len() == 1 {
        let range = &written_ranges[0];
        if range.start == 0 && range.end == colors.len() {
            prepare_output_for_leds(colors, brightness, full_brightness);
            return;
        }
    }

    for range in written_ranges {
        let start = range.start.min(colors.len());
        let end = range.end.min(colors.len());
        if start >= end {
            continue;
        }
        prepare_output_for_leds(&mut colors[start..end], brightness, full_brightness);
    }
}

fn prepare_output_for_leds(colors: &mut [[u8; 3]], brightness: f32, full_brightness: bool) {
    if full_brightness {
        prepare_output_for_leds_full_brightness(colors);
        return;
    }

    prepare_output_for_leds_scaled(colors, brightness);
}

fn prepare_output_for_leds_full_brightness(colors: &mut [[u8; 3]]) {
    for color in colors {
        let [red_u8, green_u8, blue_u8] = *color;
        if red_u8 == 0 && green_u8 == 0 && blue_u8 == 0 {
            continue;
        }

        let mut red = decode_srgb_channel(red_u8);
        let mut green = decode_srgb_channel(green_u8);
        let mut blue = decode_srgb_channel(blue_u8);
        apply_led_perceptual_compensation_channels(&mut red, &mut green, &mut blue);
        *color = [
            linear_to_output_u8(red),
            linear_to_output_u8(green),
            linear_to_output_u8(blue),
        ];
    }
}

fn prepare_output_for_leds_scaled(colors: &mut [[u8; 3]], brightness: f32) {
    for color in colors {
        let [red_u8, green_u8, blue_u8] = *color;
        if red_u8 == 0 && green_u8 == 0 && blue_u8 == 0 {
            continue;
        }

        let mut red = decode_srgb_channel(red_u8);
        let mut green = decode_srgb_channel(green_u8);
        let mut blue = decode_srgb_channel(blue_u8);
        apply_led_perceptual_compensation_channels(&mut red, &mut green, &mut blue);
        red *= brightness;
        green *= brightness;
        blue *= brightness;
        *color = [
            linear_to_output_u8(red),
            linear_to_output_u8(green),
            linear_to_output_u8(blue),
        ];
    }
}

fn should_use_ordered_routing(zone: &DeviceZone) -> bool {
    zone.zone_name.as_deref() != Some("Display")
}

fn apply_led_perceptual_compensation_channels(red: &mut f32, green: &mut f32, blue: &mut f32) {
    let max_channel = (*red).max(*green).max(*blue);
    if max_channel <= f32::EPSILON {
        return;
    }

    let min_channel = (*red).min(*green).min(*blue);
    let luma = red.mul_add(0.2126, green.mul_add(0.7152, *blue * 0.0722));
    let headroom = 1.0 - max_channel;
    if headroom <= f32::EPSILON {
        return;
    }

    // Point-light LEDs under-represent low-luma chromatic colors, especially
    // blue/cyan/magenta. Lift those gently while keeping neutrals closer to
    // the source and never exceeding the available channel headroom.
    let whiteness = min_channel / max_channel;
    let colorfulness = LED_NEUTRAL_COMPENSATION_WEIGHT
        + (1.0 - LED_NEUTRAL_COMPENSATION_WEIGHT) * (1.0 - whiteness);
    let shadow_bias = 1.0 - luma;
    let headroom_weight = LED_HEADROOM_WEIGHT_FLOOR + (1.0 - LED_HEADROOM_WEIGHT_FLOOR) * headroom;
    let gain = 1.0
        + LED_PERCEPTUAL_COMPENSATION_STRENGTH
            * shadow_bias
            * shadow_bias
            * headroom_weight
            * colorfulness;
    let gain = gain.min(1.0 / max_channel);

    if gain <= 1.0 {
        return;
    }

    *red = (*red * gain).min(1.0);
    *green = (*green * gain).min(1.0);
    *blue = (*blue * gain).min(1.0);
}

fn decode_srgb_channel(channel: u8) -> f32 {
    static SRGB_TO_LED_LINEAR_LUT: OnceLock<[f32; 256]> = OnceLock::new();

    SRGB_TO_LED_LINEAR_LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let srgb = f32::from(u8::try_from(index).expect("LUT index must fit in u8")) / 255.0;
            srgb_to_linear(srgb)
        })
    })[usize::from(channel)]
}

fn target_interval_for_fps(target_fps: u32) -> Option<Duration> {
    if target_fps == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(1.0 / f64::from(target_fps)))
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

fn remap_zone_colors<'a>(
    zone_id: &str,
    colors: &'a [[u8; 3]],
    led_mapping: Option<&[u32]>,
    scratch: &'a mut Vec<[u8; 3]>,
) -> &'a [[u8; 3]] {
    let Some(led_mapping) = led_mapping else {
        return colors;
    };

    if led_mapping.len() != colors.len() {
        warn!(
            zone_id = %zone_id,
            mapping_len = led_mapping.len(),
            color_len = colors.len(),
            "ignoring zone LED mapping because it does not match the sampled LED count"
        );
        return colors;
    }

    scratch.clear();
    scratch.resize(colors.len(), [0, 0, 0]);
    for (spatial_index, &physical_index) in led_mapping.iter().enumerate() {
        let Ok(physical_index) = usize::try_from(physical_index) else {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                "ignoring zone LED mapping because one physical index does not fit in usize"
            );
            return colors;
        };
        if physical_index >= scratch.len() {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                color_len = colors.len(),
                "ignoring zone LED mapping because one physical index is out of bounds"
            );
            return colors;
        }
        scratch[physical_index] = colors[spatial_index];
    }

    scratch.as_slice()
}

fn normalized_led_mapping(led_mapping: Option<&[u32]>) -> Option<Box<[u32]>> {
    let led_mapping = led_mapping?;

    if led_mapping
        .iter()
        .enumerate()
        .all(|(index, &physical_index)| u32::try_from(index).ok() == Some(physical_index))
    {
        return None;
    }

    Some(led_mapping.to_vec().into_boxed_slice())
}

fn zone_segments_from_device_info(device_info: &DeviceInfo) -> HashMap<String, SegmentRange> {
    let mut next_start = 0_usize;
    let mut segments = HashMap::with_capacity(device_info.zones.len());

    for zone in &device_info.zones {
        let Some(segment) = next_zone_segment(zone, next_start) else {
            continue;
        };
        next_start = segment.end();
        segments.insert(zone.name.clone(), segment);
    }

    segments
}

fn device_output_len(device_info: &DeviceInfo) -> Option<usize> {
    let total_leds = device_info
        .total_led_count()
        .max(device_info.capabilities.led_count);
    let Ok(total_leds) = usize::try_from(total_leds) else {
        warn!(
            device = %device_info.name,
            device_led_count = total_leds,
            "ignoring device output length because led_count does not fit in usize"
        );
        return None;
    };

    Some(total_leds)
}

fn next_zone_segment(zone: &ZoneInfo, start: usize) -> Option<SegmentRange> {
    let Ok(length) = usize::try_from(zone.led_count) else {
        warn!(
            zone_name = %zone.name,
            zone_led_count = zone.led_count,
            "ignoring device zone segment because led_count does not fit in usize"
        );
        return None;
    };

    Some(SegmentRange::new(start, length))
}

fn mapped_segment_for_zone_name(
    zone_id: &str,
    zone_name: Option<&str>,
    mapping: &DeviceMapping,
) -> Option<SegmentRange> {
    let Some(zone_name) = zone_name else {
        return mapping.segment;
    };
    let Some(zone_segment) = mapping.zone_segments.get(zone_name).copied() else {
        return mapping.segment;
    };

    let Some(base_segment) = mapping.segment else {
        return Some(zone_segment);
    };

    if zone_segment.start >= base_segment.start && zone_segment.end() <= base_segment.end() {
        return Some(zone_segment);
    }

    if base_segment.start >= zone_segment.start && base_segment.end() <= zone_segment.end() {
        return Some(base_segment);
    }

    let overlap_start = base_segment.start.max(zone_segment.start);
    let overlap_end = base_segment.end().min(zone_segment.end());
    if overlap_start < overlap_end {
        warn!(
            zone_id = %zone_id,
            zone_name = %zone_name,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            zone_segment_start = zone_segment.start,
            zone_segment_length = zone_segment.length,
            "using the overlap between the logical device segment and the hardware zone segment"
        );
        return Some(SegmentRange::new(
            overlap_start,
            overlap_end.saturating_sub(overlap_start),
        ));
    }

    warn!(
        zone_id = %zone_id,
        zone_name = %zone_name,
        base_segment_start = base_segment.start,
        base_segment_length = base_segment.length,
        zone_segment_start = zone_segment.start,
        zone_segment_length = zone_segment.length,
        "ignoring hardware zone segment because it does not overlap the mapped logical segment"
    );
    Some(base_segment)
}

fn layout_routing_signature(layout: &SpatialLayout) -> u64 {
    let mut hasher = DefaultHasher::new();
    layout.id.hash(&mut hasher);
    layout.zones.len().hash(&mut hasher);

    for zone in &layout.zones {
        zone.id.hash(&mut hasher);
        zone.device_id.hash(&mut hasher);
        zone.zone_name.hash(&mut hasher);
        zone.led_mapping.hash(&mut hasher);
        hash_attachment(zone.attachment.as_ref(), &mut hasher);
    }

    hasher.finish()
}

fn hash_attachment(attachment: Option<&ZoneAttachment>, hasher: &mut DefaultHasher) {
    let Some(attachment) = attachment else {
        0_u8.hash(hasher);
        return;
    };

    1_u8.hash(hasher);
    attachment.template_id.hash(hasher);
    attachment.slot_id.hash(hasher);
    attachment.instance.hash(hasher);
    attachment.led_start.hash(hasher);
    attachment.led_count.hash(hasher);
    attachment.led_mapping.hash(hasher);
}

fn attachment_segment_for_zone(
    zone_id: &str,
    base_segment: Option<SegmentRange>,
    attachment: Option<&ZoneAttachment>,
    sampled_led_count: usize,
) -> Option<SegmentRange> {
    let Some(attachment) = attachment else {
        return base_segment;
    };
    let (Some(led_start), Some(led_count)) = (attachment.led_start, attachment.led_count) else {
        return base_segment;
    };

    let Ok(led_start) = usize::try_from(led_start) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            "ignoring attachment segment override because led_start does not fit in usize"
        );
        return base_segment;
    };
    let Ok(led_count) = usize::try_from(led_count) else {
        warn!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            "ignoring attachment segment override because led_count does not fit in usize"
        );
        return base_segment;
    };
    let resolved_led_count = if sampled_led_count > 0 && sampled_led_count != led_count {
        debug!(
            zone_id = %zone_id,
            attachment_led_count = led_count,
            sampled_led_count,
            "attachment segment length differs from sampled zone length; using sampled LED count"
        );
        sampled_led_count
    } else {
        led_count
    };
    let attachment_end = led_start.saturating_add(resolved_led_count);

    if let Some(base_segment) = base_segment {
        if led_start >= base_segment.start && attachment_end <= base_segment.end() {
            return Some(SegmentRange::new(led_start, resolved_led_count));
        }

        if attachment_end <= base_segment.length {
            return Some(SegmentRange::new(
                base_segment.start.saturating_add(led_start),
                resolved_led_count,
            ));
        }

        if resolved_led_count == base_segment.length {
            return Some(base_segment);
        }

        warn!(
            zone_id = %zone_id,
            attachment_led_start = led_start,
            attachment_led_count = led_count,
            resolved_led_count,
            base_segment_start = base_segment.start,
            base_segment_length = base_segment.length,
            "ignoring attachment segment override because it exceeds the mapped segment"
        );
        return Some(base_segment);
    }

    Some(SegmentRange::new(led_start, resolved_led_count))
}
