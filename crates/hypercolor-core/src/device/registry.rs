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

    /// Reverse index for cleanup: `DeviceId` -> fingerprint.
    id_to_fingerprint: HashMap<DeviceId, DeviceFingerprint>,

    /// Scanner-provided metadata keyed by canonical device ID.
    metadata_by_id: HashMap<DeviceId, HashMap<String, String>>,
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
        let fallback_fingerprint = DeviceFingerprint(info.id.as_uuid().to_string());
        self.add_with_fingerprint(info, fallback_fingerprint).await
    }

    /// Register a device using a stable scanner-provided fingerprint.
    ///
    /// This should be used by discovery paths so a rediscovered device keeps
    /// the same logical identity even if a scanner emits a fresh `DeviceId`.
    pub async fn add_with_fingerprint(
        &self,
        info: DeviceInfo,
        fingerprint: DeviceFingerprint,
    ) -> DeviceId {
        self.add_with_fingerprint_and_metadata(info, fingerprint, HashMap::new())
            .await
    }

    /// Register a device using a stable scanner-provided fingerprint plus
    /// transport metadata such as IP address or hostname.
    pub async fn add_with_fingerprint_and_metadata(
        &self,
        info: DeviceInfo,
        fingerprint: DeviceFingerprint,
        metadata: HashMap<String, String>,
    ) -> DeviceId {
        let mut inner = self.inner.write().await;

        // Check for existing device by fingerprint
        if let Some(&existing_id) = inner.fingerprints.get(&fingerprint) {
            if let Some(entry) = inner.devices.get_mut(&existing_id) {
                let mut updated_info = info;
                // Keep the canonical registry ID stable across rediscovery.
                updated_info.id = existing_id;
                debug!(
                    device_id = %existing_id,
                    name = %updated_info.name,
                    "Updating existing device in registry"
                );
                entry.info = updated_info;
                inner
                    .id_to_fingerprint
                    .insert(existing_id, fingerprint.clone());
                if !metadata.is_empty() {
                    inner.metadata_by_id.insert(existing_id, metadata);
                }
                return existing_id;
            }

            // Stale fingerprint index entry (ID no longer exists).
            inner.fingerprints.remove(&fingerprint);
        }

        // New device
        let mut tracked_info = info;
        let mut id = tracked_info.id;

        // Defend against accidental ID reuse from scanners/backends.
        if inner.devices.contains_key(&id) {
            warn!(
                device_id = %id,
                "Device ID collision detected during registry add; allocating new ID"
            );
            while inner.devices.contains_key(&id) {
                id = DeviceId::new();
            }
        }
        tracked_info.id = id;

        let name = tracked_info.name.clone();
        let tracked = TrackedDevice {
            info: tracked_info,
            state: DeviceState::Known,
        };

        inner.fingerprints.insert(fingerprint.clone(), id);
        inner.id_to_fingerprint.insert(id, fingerprint);
        if !metadata.is_empty() {
            inner.metadata_by_id.insert(id, metadata);
        }
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
            if let Some(fingerprint) = inner.id_to_fingerprint.remove(id) {
                inner.fingerprints.remove(&fingerprint);
            } else {
                let fallback = DeviceFingerprint(id.as_uuid().to_string());
                inner.fingerprints.remove(&fallback);
            }
            inner.metadata_by_id.remove(id);
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

    /// Replace the stored metadata for a tracked device while preserving its
    /// canonical ID and lifecycle state.
    pub async fn update_info(&self, id: &DeviceId, info: DeviceInfo) -> Option<TrackedDevice> {
        let mut inner = self.inner.write().await;
        let entry = inner.devices.get_mut(id)?;

        let mut updated_info = info;
        updated_info.id = *id;
        entry.info = updated_info;

        debug!(device_id = %id, "Updated device metadata in registry");
        Some(entry.clone())
    }

    /// Update user-facing mutable settings for a tracked device.
    ///
    /// Supported updates:
    /// - `name`: display name override
    /// - `enabled`: maps to lifecycle state (`false` => `Disabled`,
    ///   `true` transitions `Disabled` back to `Known`)
    ///
    /// Returns the updated device snapshot, or `None` if the device ID is
    /// unknown.
    pub async fn update_user_settings(
        &self,
        id: &DeviceId,
        name: Option<String>,
        enabled: Option<bool>,
    ) -> Option<TrackedDevice> {
        let mut inner = self.inner.write().await;
        let entry = inner.devices.get_mut(id)?;

        if let Some(name) = name {
            entry.info.name = name;
        }

        if let Some(enabled) = enabled {
            if enabled {
                if entry.state == DeviceState::Disabled {
                    entry.state = DeviceState::Known;
                }
            } else {
                entry.state = DeviceState::Disabled;
            }
        }

        Some(entry.clone())
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

    /// Look up a device ID by stable fingerprint.
    pub async fn find_by_fingerprint(&self, fingerprint: &DeviceFingerprint) -> Option<DeviceId> {
        let inner = self.inner.read().await;
        inner.fingerprints.get(fingerprint).copied()
    }

    /// Look up a stable fingerprint by device ID.
    pub async fn fingerprint_for_id(&self, id: &DeviceId) -> Option<DeviceFingerprint> {
        let inner = self.inner.read().await;
        inner.id_to_fingerprint.get(id).cloned()
    }

    /// Look up scanner-provided transport metadata by device ID.
    pub async fn metadata_for_id(&self, id: &DeviceId) -> Option<HashMap<String, String>> {
        let inner = self.inner.read().await;
        inner.metadata_by_id.get(id).cloned()
    }

    /// Snapshot of the fingerprint index (`fingerprint -> device_id`).
    ///
    /// Useful for diffing full-scan results (new/reappeared/vanished) without
    /// exposing mutable internal state.
    pub async fn fingerprint_snapshot(&self) -> HashMap<DeviceFingerprint, DeviceId> {
        let inner = self.inner.read().await;
        inner.fingerprints.clone()
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
