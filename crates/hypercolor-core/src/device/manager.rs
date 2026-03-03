//! Device backend manager — routes per-zone color data to physical hardware.
//!
//! The [`BackendManager`] is the last mile of the frame pipeline. It holds
//! all registered device backends and a mapping from spatial layout device
//! identifiers to internal `(backend_id, DeviceId)` pairs. On each frame,
//! it groups zone colors by target device and dispatches a single
//! `write_colors` call per device.

use std::collections::HashMap;

use tracing::{debug, warn};

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;

use super::traits::DeviceBackend;

// ── BackendManager ──────────────────────────────────────────────────────────

/// Routes per-zone color data to the correct device backends.
///
/// On each frame, [`write_frame`](Self::write_frame) groups zone colors
/// by target device (using the spatial layout mapping) and dispatches
/// one `write_colors` call per device to the appropriate backend.
#[derive(Default)]
pub struct BackendManager {
    /// Registered backends, keyed by `BackendInfo.id` (e.g., `"wled"`, `"openrgb"`).
    backends: HashMap<String, Box<dyn DeviceBackend>>,

    /// Maps spatial layout `DeviceZone.device_id` strings to `(backend_id, DeviceId)`.
    ///
    /// Populated during device discovery/connection. Entries are added via
    /// [`map_device`](Self::map_device) when a zone's device reference is
    /// resolved to an actual connected device.
    device_map: HashMap<String, DeviceMapping>,
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
        debug!(backend_id = %info.id, name = %info.name, "registered device backend");
        self.backends.insert(info.id, backend);
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
        self.device_map.remove(layout_device_id).is_some()
    }

    /// Get a mutable reference to a backend by ID.
    pub fn backend_mut(&mut self, id: &str) -> Option<&mut Box<dyn DeviceBackend>> {
        self.backends.get_mut(id)
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
    /// and dispatches one `write_colors` call per device. Errors are
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

        // Dispatch to backends.
        for ((backend_id, device_id), colors) in &device_colors {
            let Some(backend) = self.backends.get_mut(backend_id.as_str()) else {
                stats
                    .errors
                    .push(format!("backend '{backend_id}' not registered"));
                continue;
            };

            match backend.write_colors(device_id, colors).await {
                Ok(()) => {
                    stats.devices_written += 1;
                    stats.total_leds += colors.len();
                }
                Err(e) => {
                    stats.errors.push(format!("{backend_id}:{device_id}: {e}"));
                }
            }
        }

        stats
    }
}
