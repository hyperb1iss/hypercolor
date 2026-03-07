//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::HashMap;
use std::borrow::Cow;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;

use super::traits::DeviceBackend;

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type BackendDeviceKey = (String, DeviceId);

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

    /// Last async write error observed by this queue worker.
    pub last_error: Option<String>,

    /// Milliseconds since last worker write attempt.
    pub last_sent_ago_ms: Option<u64>,

    /// Most recent frame sequence seen by this queue.
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Default)]
struct OutputQueueMetrics {
    frames_received: u64,
    frames_sent: u64,
    frames_dropped: u64,
    total_latency: Duration,
    last_error: Option<String>,
    last_sent_at: Option<Instant>,
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

/// Latest-frame queue for a single `(backend_id, device_id)` target.
///
/// Internally uses a `watch` channel so stale queued payloads are replaced
/// atomically and the sender never blocks the render loop.
struct OutputQueue {
    tx: watch::Sender<Option<Arc<FramePayload>>>,
    _io_task: JoinHandle<()>,
    target_fps: u32,
    metrics: Arc<StdMutex<OutputQueueMetrics>>,
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
        let metrics = Arc::new(StdMutex::new(OutputQueueMetrics::default()));
        let metrics_for_task = Arc::clone(&metrics);

        let io_task = tokio::spawn(async move {
            let frame_interval = if target_fps == 0 {
                None
            } else {
                Some(Duration::from_secs_f64(1.0 / f64::from(target_fps)))
            };

            let mut last_sent_sequence = 0_u64;

            loop {
                // Sender dropped => manager shutdown or queue removed.
                if rx.changed().await.is_err() {
                    break;
                }

                let Some(frame) = rx.borrow_and_update().clone() else {
                    continue;
                };

                if frame.sequence > last_sent_sequence + 1 {
                    let dropped = frame.sequence - last_sent_sequence - 1;
                    if let Ok(mut snapshot) = metrics_for_task.lock() {
                        snapshot.frames_dropped = snapshot.frames_dropped.saturating_add(dropped);
                    }

                    trace!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        dropped,
                        "dropping stale device frames"
                    );
                }

                let send_started = Instant::now();
                let result = {
                    let mut backend = backend.lock().await;
                    backend.write_colors(&device_id, &frame.colors).await
                };
                let send_completed = Instant::now();

                if let Ok(mut snapshot) = metrics_for_task.lock() {
                    snapshot.last_sent_at = Some(send_completed);

                    match &result {
                        Ok(()) => {
                            snapshot.frames_sent = snapshot.frames_sent.saturating_add(1);
                            snapshot.total_latency +=
                                send_completed.saturating_duration_since(frame.produced_at);
                            snapshot.last_error = None;
                        }
                        Err(error) => {
                            snapshot.last_error = Some(error.to_string());
                        }
                    }
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

                if let Some(interval) = frame_interval {
                    let elapsed = send_started.elapsed();
                    if let Some(remaining) = interval.checked_sub(elapsed) {
                        tokio::time::sleep(remaining).await;
                    }
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
    fn push(&mut self, colors: Vec<[u8; 3]>) {
        if let Ok(mut snapshot) = self.metrics.lock() {
            snapshot.frames_received = snapshot.frames_received.saturating_add(1);
        }

        self.next_sequence = self.next_sequence.saturating_add(1);

        self.tx.send_replace(Some(Arc::new(FramePayload {
            colors,
            sequence: self.next_sequence,
            produced_at: Instant::now(),
        })));
    }

    fn snapshot(
        &self,
        backend_id: &str,
        device_id: DeviceId,
        mapped_layout_ids: Vec<String>,
    ) -> OutputQueueDebugSnapshot {
        let metrics = self
            .metrics
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let avg_latency_ms = if metrics.frames_sent == 0 {
            0
        } else {
            let divisor = u32::try_from(metrics.frames_sent).unwrap_or(u32::MAX);
            let average = metrics
                .total_latency
                .checked_div(divisor)
                .unwrap_or_default()
                .as_millis();
            u64::try_from(average).unwrap_or(u64::MAX)
        };
        let now = Instant::now();
        let last_sent_ago_ms = metrics.last_sent_at.map(|last| {
            let ms = now.saturating_duration_since(last).as_millis();
            u64::try_from(ms).unwrap_or(u64::MAX)
        });

        OutputQueueDebugSnapshot {
            backend_id: backend_id.to_owned(),
            device_id: device_id.to_string(),
            mapped_layout_ids,
            target_fps: self.target_fps,
            frames_received: metrics.frames_received,
            frames_sent: metrics.frames_sent,
            frames_dropped: metrics.frames_dropped,
            avg_latency_ms,
            last_error: metrics.last_error,
            last_sent_ago_ms,
            last_sequence: self.next_sequence,
        }
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
    /// Registered backends, keyed by `BackendInfo.id` (e.g., `"wled"`, `"openrgb"`).
    backends: HashMap<String, BackendHandle>,

    /// Maps spatial layout `DeviceZone.device_id` strings to `(backend_id, DeviceId)`.
    ///
    /// Populated during device discovery/connection. Entries are added via
    /// [`map_device`](Self::map_device) when a zone's device reference is
    /// resolved to an actual connected device.
    device_map: HashMap<String, DeviceMapping>,

    /// Per-target latest-frame output queues.
    output_queues: HashMap<BackendDeviceKey, OutputQueue>,

    /// Reference-counted direct-control locks that suppress queued frame writes.
    direct_control_locks: HashMap<BackendDeviceKey, usize>,
}

/// Internal mapping from a layout device identifier to a backend + device.
#[derive(Debug, Clone)]
struct DeviceMapping {
    backend_id: String,
    device_id: DeviceId,
    segment: Option<SegmentRange>,
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
        self.direct_control_locks
            .retain(|(locked_backend_id, _), _| locked_backend_id != &backend_id);

        self.backends
            .insert(backend_id, Arc::new(Mutex::new(backend)));
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
        self.device_map.insert(
            layout_id,
            DeviceMapping {
                backend_id: backend,
                device_id,
                segment,
            },
        );
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
        let Some(backend) = self.backends.get(backend_id).cloned() else {
            bail!("backend '{backend_id}' is not registered");
        };

        {
            let mut backend = backend.lock().await;
            if let Err(initial_error) = backend.connect(&device_id).await {
                let initial_message = initial_error.to_string();
                debug!(
                    backend_id = %backend_id,
                    %device_id,
                    error = %initial_message,
                    "initial connect failed; refreshing backend discovery state and retrying"
                );

                backend.discover().await.with_context(|| {
                    format!(
                        "backend '{backend_id}' discovery refresh failed after initial connect failure for device {device_id}: {initial_message}"
                    )
                })?;

                if let Err(retry_error) = backend.connect(&device_id).await {
                    let retry_message = retry_error.to_string();
                    debug!(
                        backend_id = %backend_id,
                        %device_id,
                        error = %retry_message,
                        "connect still failing after discovery refresh"
                    );
                    return Err(retry_error).with_context(|| {
                        format!(
                            "failed to connect device {device_id} using backend '{backend_id}' after discovery refresh (initial error: {initial_message})"
                        )
                    });
                }

                debug!(
                    backend_id = %backend_id,
                    %device_id,
                    "connect succeeded after discovery refresh"
                );
            }
        }

        self.map_device(
            layout_device_id.to_owned(),
            backend_id.to_owned(),
            device_id,
        );
        Ok(())
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
        let Some(backend) = self.backends.get(backend_id).cloned() else {
            bail!("backend '{backend_id}' is not registered");
        };

        {
            let mut backend = backend.lock().await;
            backend.disconnect(&device_id).await.with_context(|| {
                format!("failed to disconnect device {device_id} using backend '{backend_id}'")
            })?;
        }

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
        let Some(backend) = self.backends.get(backend_id).cloned() else {
            bail!("backend '{backend_id}' is not registered");
        };

        let mut backend = backend.lock().await;
        backend
            .write_colors(&device_id, colors)
            .await
            .with_context(|| {
                format!(
                    "failed to write {} colors to device {device_id} using backend '{backend_id}'",
                    colors.len()
                )
            })
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
        self.direct_control_locks
            .get(&(backend_id.to_owned(), device_id))
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
            self.output_queues.remove(&key);
            self.direct_control_locks.remove(&key);
        }

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
            self.output_queues
                .remove(&(backend_id.to_owned(), device_id));
            self.direct_control_locks
                .remove(&(backend_id.to_owned(), device_id));
        }

        removed
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

    /// Push frame color data to all mapped devices.
    ///
    /// For each zone in `zone_colors`, looks up the target device via the
    /// spatial layout's zone-to-device mapping, groups colors by device,
    /// and enqueues one payload per device. Errors are
    /// collected but do not halt processing — every mapped device gets
    /// its data.
    #[allow(clippy::unused_async)]
    pub async fn write_frame(
        &mut self,
        zone_colors: &[ZoneColors],
        layout: &SpatialLayout,
    ) -> FrameWriteStats {
        let mut stats = FrameWriteStats::default();

        // Build zone_id → layout device_id lookup from the spatial layout.
        let zone_to_device: HashMap<&str, &str> = layout
            .zones
            .iter()
            .map(|z| (z.id.as_str(), z.device_id.as_str()))
            .collect();
        let zone_to_mapping: HashMap<&str, Option<&[u32]>> = layout
            .zones
            .iter()
            .map(|zone| (zone.id.as_str(), zone.led_mapping.as_deref()))
            .collect();

        #[allow(clippy::items_after_statements)]
        #[derive(Default)]
        struct AccumulatedColors {
            values: Vec<[u8; 3]>,
            has_segmented_write: bool,
        }

        // Group colors by (backend_id, device_id). Owned keys to avoid
        // borrow conflicts with `self.backends` during the write phase.
        let mut device_colors: HashMap<(String, DeviceId), AccumulatedColors> = HashMap::new();

        for zc in zone_colors {
            let Some(layout_device_id) = zone_to_device.get(zc.zone_id.as_str()) else {
                warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                continue;
            };
            let remapped_colors = remap_zone_colors(
                &zc.zone_id,
                &zc.colors,
                zone_to_mapping
                    .get(zc.zone_id.as_str())
                    .copied()
                    .flatten(),
            );

            let Some(mapping) = self.device_map.get(*layout_device_id) else {
                // Not mapped — device may not be connected. Silent skip.
                continue;
            };

            let key = (mapping.backend_id.clone(), mapping.device_id);
            let entry = device_colors.entry(key).or_default();

            if let Some(segment) = mapping.segment {
                entry.has_segmented_write = true;
                let required_len = segment.end();
                if entry.values.len() < required_len {
                    entry.values.resize(required_len, [0, 0, 0]);
                }

                let copy_len = segment.length.min(remapped_colors.len());
                if copy_len > 0 {
                    let start = segment.start;
                    let end = start.saturating_add(copy_len);
                    entry.values[start..end].copy_from_slice(&remapped_colors[..copy_len]);
                }

                if copy_len != segment.length {
                    warn!(
                        zone_id = %zc.zone_id,
                        expected = segment.length,
                        received = remapped_colors.len(),
                        "zone color count does not match mapped segment length"
                    );
                }
            } else {
                if entry.has_segmented_write {
                    warn!(
                        zone_id = %zc.zone_id,
                        "mixed segmented and non-segmented mappings for the same physical device"
                    );
                }
                entry.values.extend_from_slice(&remapped_colors);
            }
        }

        // Dispatch to output queues.
        for ((backend_id, device_id), colors) in device_colors {
            if self.is_direct_control_active(backend_id.as_str(), device_id) {
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

            let Some(queue) = self.ensure_output_queue(backend_id.as_str(), device_id) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            };

            stats.devices_written += 1;
            stats.total_leds += colors.values.len();
            queue.push(colors.values);
        }

        stats
    }

    fn ensure_output_queue(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<&mut OutputQueue> {
        let key = (backend_id.to_owned(), device_id);

        if !self.output_queues.contains_key(&key) {
            let backend = self.backends.get(backend_id)?.clone();

            // Use a 60fps default for now; backend-specific caps can be
            // introduced once the trait grows explicit max-fps reporting.
            let queue = OutputQueue::spawn(backend_id.to_owned(), device_id, backend, 60);
            self.output_queues.insert(key.clone(), queue);
        }

        self.output_queues.get_mut(&key)
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

fn remap_zone_colors<'a>(
    zone_id: &str,
    colors: &'a [[u8; 3]],
    led_mapping: Option<&[u32]>,
) -> Cow<'a, [[u8; 3]]> {
    let Some(led_mapping) = led_mapping else {
        return Cow::Borrowed(colors);
    };

    if led_mapping.len() != colors.len() {
        warn!(
            zone_id = %zone_id,
            mapping_len = led_mapping.len(),
            color_len = colors.len(),
            "ignoring zone LED mapping because it does not match the sampled LED count"
        );
        return Cow::Borrowed(colors);
    }

    let mut reordered = vec![[0, 0, 0]; colors.len()];
    for (spatial_index, &physical_index) in led_mapping.iter().enumerate() {
        let Ok(physical_index) = usize::try_from(physical_index) else {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                "ignoring zone LED mapping because one physical index does not fit in usize"
            );
            return Cow::Borrowed(colors);
        };
        if physical_index >= reordered.len() {
            warn!(
                zone_id = %zone_id,
                mapping_index = physical_index,
                color_len = colors.len(),
                "ignoring zone LED mapping because one physical index is out of bounds"
            );
            return Cow::Borrowed(colors);
        }
        reordered[physical_index] = colors[spatial_index];
    }

    Cow::Owned(reordered)
}
