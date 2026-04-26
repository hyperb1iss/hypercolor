//! Thread-safe device registry for tracking known devices.
//!
//! The [`DeviceRegistry`] stores all devices the engine knows about — both
//! actively connected and previously seen. It is the single source of truth
//! for device identity, state, and metadata within a running daemon session.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::{DiscoveredDevice, DiscoveryConnectBehavior};
use crate::types::device::{
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceUserSettings,
};

// ── TrackedDevice ────────────────────────────────────────────────────────

/// A device entry in the registry, combining identity with runtime state.
#[derive(Debug, Clone)]
pub struct TrackedDevice {
    /// Full device metadata.
    pub info: DeviceInfo,

    /// Current lifecycle state.
    pub state: DeviceState,

    /// Whether lifecycle should auto-connect this device when it is discovered.
    pub connect_behavior: DiscoveryConnectBehavior,

    /// Persisted user-facing settings layered on top of discovered metadata.
    pub user_settings: DeviceUserSettings,
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
    generation: Arc<AtomicU64>,
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
            generation: Arc::new(AtomicU64::new(0)),
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
        self.add_entry(
            info,
            fingerprint,
            metadata,
            DiscoveryConnectBehavior::AutoConnect,
        )
        .await
    }

    /// Register a scanner-produced device with explicit connection behavior.
    pub async fn add_discovered(&self, mut discovered: DiscoveredDevice) -> DeviceId {
        discovered.info.origin = discovered.origin;
        self.add_entry(
            discovered.info,
            discovered.fingerprint,
            discovered.metadata,
            discovered.connect_behavior,
        )
        .await
    }

    async fn add_entry(
        &self,
        info: DeviceInfo,
        fingerprint: DeviceFingerprint,
        metadata: HashMap<String, String>,
        connect_behavior: DiscoveryConnectBehavior,
    ) -> DeviceId {
        let mut inner = self.inner.write().await;

        // Check for existing device by fingerprint
        if let Some(&existing_id) = inner.fingerprints.get(&fingerprint) {
            if let Some(entry) = inner.devices.get_mut(&existing_id) {
                let mut updated_info = info;
                // Keep the canonical registry ID stable across rediscovery.
                updated_info.id = existing_id;
                preserve_renderable_device_shape(&mut updated_info, &entry.info, &entry.state);
                apply_user_settings_to_info(&mut updated_info, &entry.user_settings);
                debug!(
                    device_id = %existing_id,
                    name = %updated_info.name,
                    "Updating existing device in registry"
                );
                entry.info = updated_info;
                entry.connect_behavior = connect_behavior;
                inner
                    .id_to_fingerprint
                    .insert(existing_id, fingerprint.clone());
                if !metadata.is_empty() {
                    inner.metadata_by_id.insert(existing_id, metadata);
                }
                self.bump_generation();
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
            connect_behavior,
            user_settings: DeviceUserSettings::default(),
        };

        inner.fingerprints.insert(fingerprint.clone(), id);
        inner.id_to_fingerprint.insert(id, fingerprint);
        if !metadata.is_empty() {
            inner.metadata_by_id.insert(id, metadata);
        }
        inner.devices.insert(id, tracked);
        self.bump_generation();

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
            self.bump_generation();
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
            if entry.state == state {
                return true;
            }
            debug!(
                device_id = %id,
                from = %entry.state,
                to = %state,
                "Device state transition"
            );
            entry.state = state;
            self.bump_generation();
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
        apply_user_settings_to_info(&mut updated_info, &entry.user_settings);
        entry.info = updated_info;
        self.bump_generation();

        debug!(device_id = %id, "Updated device metadata in registry");
        Some(entry.clone())
    }

    /// Update user-facing mutable settings for a tracked device.
    ///
    /// Supported updates:
    /// - `name`: display name override
    /// - `enabled`: persisted user preference for whether the device should
    ///   participate in rendering
    /// - `brightness`: per-device output scale (`0.0..=1.0`)
    ///
    /// Returns the updated device snapshot, or `None` if the device ID is
    /// unknown.
    pub async fn update_user_settings(
        &self,
        id: &DeviceId,
        name: Option<String>,
        enabled: Option<bool>,
        brightness: Option<f32>,
    ) -> Option<TrackedDevice> {
        let mut inner = self.inner.write().await;
        let entry = inner.devices.get_mut(id)?;

        if let Some(name) = name {
            entry.user_settings.name = Some(name.clone());
            entry.info.name = name;
        }

        if let Some(enabled) = enabled {
            entry.user_settings.enabled = enabled;
        }

        if let Some(brightness) = brightness {
            entry.user_settings.brightness = brightness.clamp(0.0, 1.0);
        }

        self.bump_generation();
        Some(entry.clone())
    }

    /// Replace all stored user settings for a tracked device.
    pub async fn replace_user_settings(
        &self,
        id: &DeviceId,
        settings: DeviceUserSettings,
    ) -> Option<TrackedDevice> {
        let mut inner = self.inner.write().await;
        let entry = inner.devices.get_mut(id)?;

        entry.user_settings = settings;
        apply_user_settings_to_info(&mut entry.info, &entry.user_settings);

        self.bump_generation();
        Some(entry.clone())
    }

    /// Monotonic mutation counter for cheap cache invalidation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
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

    /// Snapshot per-device brightness scalars keyed by device ID.
    pub async fn brightness_snapshot(&self) -> HashMap<DeviceId, f32> {
        let inner = self.inner.read().await;
        inner
            .devices
            .iter()
            .map(|(device_id, tracked)| {
                (*device_id, tracked.user_settings.brightness.clamp(0.0, 1.0))
            })
            .collect()
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

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_user_settings_to_info(info: &mut DeviceInfo, settings: &DeviceUserSettings) {
    if let Some(name) = settings.name.as_ref() {
        info.name.clone_from(name);
    }
}

fn preserve_renderable_device_shape(
    incoming: &mut DeviceInfo,
    existing: &DeviceInfo,
    state: &DeviceState,
) {
    if !state.is_renderable() {
        return;
    }

    let incoming_has_shape = !incoming.zones.is_empty()
        || incoming.capabilities.led_count > 0
        || incoming.capabilities.has_display;
    let existing_has_shape = !existing.zones.is_empty()
        || existing.capabilities.led_count > 0
        || existing.capabilities.has_display;

    if incoming_has_shape || !existing_has_shape {
        return;
    }

    incoming.zones.clone_from(&existing.zones);
    incoming.capabilities = existing.capabilities;
}
