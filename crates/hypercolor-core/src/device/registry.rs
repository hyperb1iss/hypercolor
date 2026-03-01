//! Thread-safe device registry for tracking known devices.
//!
//! The [`DeviceRegistry`] stores all devices the engine knows about — both
//! actively connected and previously seen. It is the single source of truth
//! for device identity, state, and metadata within a running daemon session.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DeviceState};

// ── TrackedDevice ────────────────────────────────────────────────────────

/// A device entry in the registry, combining identity with runtime state.
#[derive(Debug, Clone)]
pub struct TrackedDevice {
    /// Full device metadata.
    pub info: DeviceInfo,

    /// Current lifecycle state.
    pub state: DeviceState,
}

// ── DeviceRegistry ───────────────────────────────────────────────────────

/// Thread-safe registry for tracking all known devices.
///
/// Uses `Arc<RwLock<...>>` internally so it can be shared across the render
/// loop, discovery orchestrator, REST API handlers, and WebSocket broadcast
/// tasks without external synchronization.
///
/// Devices are indexed by [`DeviceId`] for fast lookup and by
/// [`DeviceFingerprint`] for deduplication during discovery.
#[derive(Debug, Clone)]
pub struct DeviceRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// Primary index: `DeviceId` -> tracked device.
    devices: HashMap<DeviceId, TrackedDevice>,

    /// Deduplication index: fingerprint -> `DeviceId`.
    fingerprints: HashMap<DeviceFingerprint, DeviceId>,
}

impl DeviceRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner::default())),
        }
    }

    /// Register a new device or update an existing one.
    ///
    /// If a device with the same fingerprint already exists, its metadata is
    /// updated in place and the existing `DeviceId` is returned. Otherwise a
    /// new entry is created.
    pub async fn add(&self, info: DeviceInfo) -> DeviceId {
        let fingerprint = info.id.as_uuid().to_string();
        let fp = DeviceFingerprint(fingerprint);
        let mut inner = self.inner.write().await;

        // Check for existing device by fingerprint
        if let Some(&existing_id) = inner.fingerprints.get(&fp) {
            if let Some(entry) = inner.devices.get_mut(&existing_id) {
                debug!(
                    device_id = %existing_id,
                    name = %info.name,
                    "Updating existing device in registry"
                );
                entry.info = info;
                return existing_id;
            }
        }

        // New device
        let id = info.id;
        let name = info.name.clone();
        let tracked = TrackedDevice {
            info,
            state: DeviceState::Known,
        };

        inner.fingerprints.insert(fp, id);
        inner.devices.insert(id, tracked);

        info!(device_id = %id, name = %name, "Device added to registry");
        id
    }

    /// Remove a device from the registry.
    ///
    /// Returns the tracked device if it existed, `None` otherwise.
    pub async fn remove(&self, id: &DeviceId) -> Option<TrackedDevice> {
        let mut inner = self.inner.write().await;

        let device = inner.devices.remove(id);
        if device.is_some() {
            // Clean up the fingerprint index
            let fingerprint = DeviceFingerprint(id.as_uuid().to_string());
            inner.fingerprints.remove(&fingerprint);
            info!(device_id = %id, "Device removed from registry");
        } else {
            warn!(device_id = %id, "Attempted to remove unknown device");
        }
        device
    }

    /// Look up a device by its ID.
    ///
    /// Returns a clone of the tracked device data. For frequent hot-path
    /// access, callers should cache the result locally.
    pub async fn get(&self, id: &DeviceId) -> Option<TrackedDevice> {
        let inner = self.inner.read().await;
        inner.devices.get(id).cloned()
    }

    /// List all tracked devices.
    ///
    /// Returns cloned snapshots — safe to hold across await points without
    /// blocking other registry operations.
    pub async fn list(&self) -> Vec<TrackedDevice> {
        let inner = self.inner.read().await;
        inner.devices.values().cloned().collect()
    }

    /// Update the state of a tracked device.
    ///
    /// Returns `true` if the device was found and updated, `false` if the
    /// device ID is unknown.
    pub async fn set_state(&self, id: &DeviceId, state: DeviceState) -> bool {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.devices.get_mut(id) {
            debug!(
                device_id = %id,
                from = %entry.state,
                to = %state,
                "Device state transition"
            );
            entry.state = state;
            true
        } else {
            warn!(device_id = %id, "State update for unknown device");
            false
        }
    }

    /// Number of devices currently tracked.
    pub async fn len(&self) -> usize {
        let inner = self.inner.read().await;
        inner.devices.len()
    }

    /// Whether the registry contains no devices.
    pub async fn is_empty(&self) -> bool {
        let inner = self.inner.read().await;
        inner.devices.is_empty()
    }

    /// Check whether a device with the given ID exists.
    pub async fn contains(&self, id: &DeviceId) -> bool {
        let inner = self.inner.read().await;
        inner.devices.contains_key(id)
    }

    /// List all devices in a specific state.
    pub async fn list_by_state(&self, state: &DeviceState) -> Vec<TrackedDevice> {
        let inner = self.inner.read().await;
        inner
            .devices
            .values()
            .filter(|d| &d.state == state)
            .cloned()
            .collect()
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
