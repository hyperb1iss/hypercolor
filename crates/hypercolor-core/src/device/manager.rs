//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and queues a single payload per
//! device for asynchronous transmission.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;

use super::traits::DeviceBackend;

type BackendHandle = Arc<Mutex<Box<dyn DeviceBackend>>>;
type BackendDeviceKey = (String, DeviceId);

// ── OutputQueue ─────────────────────────────────────────────────────────────

/// Frame payload queued for asynchronous backend writes.
#[derive(Debug, Clone)]
struct FramePayload {
    /// LED colors for the target device.
    colors: Vec<[u8; 3]>,
    /// Monotonic sequence for dropped-frame diagnostics.
    sequence: u64,
}

/// Latest-frame queue for a single `(backend_id, device_id)` target.
///
/// Internally uses a `watch` channel so stale queued payloads are replaced
/// atomically and the sender never blocks the render loop.
struct OutputQueue {
    tx: watch::Sender<Option<Arc<FramePayload>>>,
    _io_task: JoinHandle<()>,
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
                    trace!(
                        backend_id = %backend_id,
                        device_id = %device_id,
                        dropped = frame.sequence - last_sent_sequence - 1,
                        "dropping stale device frames"
                    );
                }

                let send_started = Instant::now();
                let result = {
                    let mut backend = backend.lock().await;
                    backend.write_colors(&device_id, &frame.colors).await
                };

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
                    if elapsed < interval {
                        tokio::time::sleep(interval - elapsed).await;
                    }
                }
            }
        });

        Self {
            tx,
            _io_task: io_task,
            next_sequence: 0,
        }
    }

    /// Push the latest payload for this device.
    fn push(&mut self, colors: Vec<[u8; 3]>) {
        self.next_sequence = self.next_sequence.saturating_add(1);

        self.tx.send_replace(Some(Arc::new(FramePayload {
            colors,
            sequence: self.next_sequence,
        })));
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
}

/// Internal mapping from a layout device identifier to a backend + device.
#[derive(Debug, Clone)]
struct DeviceMapping {
    backend_id: String,
    device_id: DeviceId,
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
        let layout_id = layout_device_id.into();
        let backend = backend_id.into();
        debug!(
            layout_device_id = %layout_id,
            backend_id = %backend,
            %device_id,
            "mapped device"
        );
        self.device_map.insert(
            layout_id,
            DeviceMapping {
                backend_id: backend,
                device_id,
            },
        );
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
            self.output_queues
                .remove(&(mapping.backend_id, mapping.device_id));
        }

        true
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

        // Group colors by (backend_id, device_id). Owned keys to avoid
        // borrow conflicts with `self.backends` during the write phase.
        let mut device_colors: HashMap<(String, DeviceId), Vec<[u8; 3]>> = HashMap::new();

        for zc in zone_colors {
            let Some(layout_device_id) = zone_to_device.get(zc.zone_id.as_str()) else {
                warn!(zone_id = %zc.zone_id, "zone not found in spatial layout");
                continue;
            };

            let Some(mapping) = self.device_map.get(*layout_device_id) else {
                // Not mapped — device may not be connected. Silent skip.
                continue;
            };

            device_colors
                .entry((mapping.backend_id.clone(), mapping.device_id))
                .or_default()
                .extend_from_slice(&zc.colors);
        }

        // Dispatch to output queues.
        for ((backend_id, device_id), colors) in device_colors {
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
            stats.total_leds += colors.len();
            queue.push(colors);
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
}
