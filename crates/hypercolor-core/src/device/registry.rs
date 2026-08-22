//! Thread-safe device registry for tracking known devices.
//!
//! The [`DeviceRegistry`] stores all devices the engine knows about — both
//! actively connected and previously seen. It is the single source of truth
//! for device identity, state, and metadata within a running daemon session.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceUserSettings,
    FingerprintNamespace,
};
use hypercolor_types::portable::{PortableDeviceKey, PortableIdentityClaim};

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

    /// Monotonic mutation counter for this specific device.
    pub revision: u64,
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

    /// Latest portable identity observation per device.
    claims_by_id: HashMap<DeviceId, PortableIdentityClaim>,

    /// Portable key -> the fingerprint the key was first observed with.
    ///
    /// The pin is the durable identity for claimed hardware: a later
    /// observation of the same key under a different fingerprint (cable
    /// move, BIOS renumbering, restart) resolves to the pinned fingerprint
    /// instead of minting a fresh device.
    portable_key_pins: HashMap<PortableDeviceKey, DeviceFingerprint>,

    /// Keys proven to name more than one physical device. A quarantined
    /// key resolves no pins; each unit lives under its raw fingerprint.
    quarantined_keys: HashSet<PortableDeviceKey>,

    /// Proven same-key collisions awaiting persistence by the host.
    collision_log: Vec<PortableKeyCollision>,
}

/// Why a user-driven portable re-bind was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableRebindError {
    /// The device to re-bind is not in the registry.
    UnknownDevice,

    /// The device carries no portable identity claim, so no pin could
    /// make the re-bind durable. Claimless hardware re-binds by editing
    /// the layout instead.
    Unclaimed,

    /// The fingerprint to inherit belongs to a device that is currently
    /// renderable, which means it was not replaced.
    TargetActive,
}

/// One installation observing two simultaneously present units that claim
/// the same portable key, with evidence strong enough to distinguish them.
///
/// This is the only situation that proves a collision: sequential
/// attachment cannot rule out one device moving between ports, and
/// observations from two machines describe the dual-boot case.
#[derive(Debug, Clone)]
pub struct PortableKeyCollision {
    /// The key both units claimed.
    pub key: PortableDeviceKey,

    /// The device already holding the key.
    pub existing_device: DeviceId,

    /// The holder's fingerprint at observation time.
    pub existing_fingerprint: DeviceFingerprint,

    /// The holder's claim, carrying its attachment evidence.
    pub existing_claim: PortableIdentityClaim,

    /// The second unit's fingerprint.
    pub incoming_fingerprint: DeviceFingerprint,

    /// The second unit's claim, whose evidence differed.
    pub incoming_claim: PortableIdentityClaim,
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
        let fallback_fingerprint = DeviceFingerprint::mint(
            FingerprintNamespace::Bridge,
            "registry",
            &info.id.as_uuid().to_string(),
        );
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
            None,
        )
        .await
    }

    /// Register a scanner-produced device with explicit connection behavior.
    pub async fn add_discovered(&self, discovered: DiscoveredDevice) -> DeviceId {
        self.add_entry(
            discovered.info,
            discovered.fingerprint,
            discovered.metadata,
            discovered.connect_behavior,
            discovered.claim,
        )
        .await
    }

    async fn add_entry(
        &self,
        info: DeviceInfo,
        fingerprint: DeviceFingerprint,
        metadata: HashMap<String, String>,
        connect_behavior: DiscoveryConnectBehavior,
        claim: Option<PortableIdentityClaim>,
    ) -> DeviceId {
        let mut inner = self.inner.write().await;
        let fingerprint = resolve_portable_fingerprint(&mut inner, fingerprint, claim.as_ref());

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
                bump_device_revision(entry);
                inner
                    .id_to_fingerprint
                    .insert(existing_id, fingerprint.clone());
                if !metadata.is_empty() {
                    inner.metadata_by_id.insert(existing_id, metadata);
                }
                store_portable_claim(&mut inner, existing_id, claim);
                self.bump_generation();
                return existing_id;
            }

            // Stale fingerprint index entry (ID no longer exists).
            inner.fingerprints.remove(&fingerprint);
        }

        if let Some(existing_id) = find_single_renderable_smbus_dram_match(&inner, &info, &metadata)
        {
            if let Some(previous_fingerprint) = inner
                .id_to_fingerprint
                .insert(existing_id, fingerprint.clone())
            {
                inner.fingerprints.remove(&previous_fingerprint);
            }
            inner.fingerprints.insert(fingerprint.clone(), existing_id);

            if let Some(entry) = inner.devices.get_mut(&existing_id) {
                let mut updated_info = info;
                updated_info.id = existing_id;
                preserve_renderable_device_shape(&mut updated_info, &entry.info, &entry.state);
                apply_user_settings_to_info(&mut updated_info, &entry.user_settings);
                debug!(
                    device_id = %existing_id,
                    name = %updated_info.name,
                    "Updating existing SMBus DRAM device after remap address change"
                );
                entry.info = updated_info;
                entry.connect_behavior = connect_behavior;
                bump_device_revision(entry);
                if !metadata.is_empty() {
                    inner.metadata_by_id.insert(existing_id, metadata);
                }
                store_portable_claim(&mut inner, existing_id, claim);
                self.bump_generation();
                return existing_id;
            }

            inner.fingerprints.remove(&fingerprint);
            inner.id_to_fingerprint.remove(&existing_id);
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
            revision: 0,
        };

        inner.fingerprints.insert(fingerprint.clone(), id);
        inner.id_to_fingerprint.insert(id, fingerprint);
        if !metadata.is_empty() {
            inner.metadata_by_id.insert(id, metadata);
        }
        inner.devices.insert(id, tracked);
        store_portable_claim(&mut inner, id, claim);
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
                let fallback = DeviceFingerprint::mint(
                    FingerprintNamespace::Bridge,
                    "registry",
                    &id.as_uuid().to_string(),
                );
                inner.fingerprints.remove(&fallback);
            }
            inner.fingerprints.retain(|_, mapped_id| mapped_id != id);
            inner.metadata_by_id.remove(id);
            // The key pin outlives the device on purpose: pins are the
            // durable identity that lets the same hardware resolve back
            // after a vanish/reappear cycle or a restart.
            inner.claims_by_id.remove(id);
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
            bump_device_revision(entry);
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
        bump_device_revision(entry);
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

        bump_device_revision(entry);
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

        bump_device_revision(entry);
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

    /// The latest portable identity observation for a device, if any.
    pub async fn claim_for_id(&self, id: &DeviceId) -> Option<PortableIdentityClaim> {
        let inner = self.inner.read().await;
        inner.claims_by_id.get(id).cloned()
    }

    /// Snapshot of every stored portable identity claim, keyed by device.
    pub async fn claims_snapshot(&self) -> HashMap<DeviceId, PortableIdentityClaim> {
        let inner = self.inner.read().await;
        inner.claims_by_id.clone()
    }

    /// Seed portable identity state persisted by the host, before scans.
    ///
    /// Pins recorded in earlier sessions are what let a claimed device
    /// reattach under a new fingerprint and still resolve to the identity
    /// its layouts reference. Existing in-session state wins on conflict.
    pub async fn seed_portable_identity(
        &self,
        pins: HashMap<PortableDeviceKey, DeviceFingerprint>,
        quarantined_keys: HashSet<PortableDeviceKey>,
    ) {
        let mut inner = self.inner.write().await;
        for (key, fingerprint) in pins {
            inner.portable_key_pins.entry(key).or_insert(fingerprint);
        }
        inner.quarantined_keys.extend(quarantined_keys);
    }

    /// Snapshot of the portable key pins (`key -> pinned fingerprint`).
    pub async fn portable_key_pins(&self) -> HashMap<PortableDeviceKey, DeviceFingerprint> {
        let inner = self.inner.read().await;
        inner.portable_key_pins.clone()
    }

    /// Keys proven to name more than one physical device.
    pub async fn quarantined_portable_keys(&self) -> HashSet<PortableDeviceKey> {
        let inner = self.inner.read().await;
        inner.quarantined_keys.clone()
    }

    /// Whether a key is proven to name more than one physical device.
    pub async fn is_portable_key_quarantined(&self, key: &PortableDeviceKey) -> bool {
        let inner = self.inner.read().await;
        inner.quarantined_keys.contains(key)
    }

    /// Take the collisions proven since the last drain, for persistence.
    pub async fn drain_portable_key_collisions(&self) -> Vec<PortableKeyCollision> {
        let mut inner = self.inner.write().await;
        std::mem::take(&mut inner.collision_log)
    }

    /// Record a collision proven outside the registry, quarantining the
    /// key.
    ///
    /// Two units sharing a constant serial or MAC usually share a
    /// fingerprint too, so the discovery orchestrator's aggregation
    /// collapses them before the registry could compare their evidence.
    /// Detection has to run before deduplication, and aggregation is the
    /// only place both observations are still visible; this is its
    /// channel to the quarantine.
    pub async fn report_portable_key_collision(
        &self,
        device_id: DeviceId,
        fingerprint: DeviceFingerprint,
        existing_claim: PortableIdentityClaim,
        incoming_claim: PortableIdentityClaim,
    ) {
        let mut inner = self.inner.write().await;
        let key = existing_claim.key().clone();
        if inner.quarantined_keys.contains(&key) {
            return;
        }

        warn!(
            key = %key,
            device_id = %device_id,
            existing_evidence = ?existing_claim.evidence(),
            incoming_evidence = ?incoming_claim.evidence(),
            "Two simultaneously present devices share one fingerprint and portable key; quarantining"
        );
        inner.collision_log.push(PortableKeyCollision {
            key: key.clone(),
            existing_device: device_id,
            existing_fingerprint: fingerprint.clone(),
            existing_claim,
            incoming_fingerprint: fingerprint,
            incoming_claim,
        });
        inner.quarantined_keys.insert(key.clone());
        inner.portable_key_pins.remove(&key);
        self.bump_generation();
    }

    /// Re-point a claimed device onto another fingerprint, adopting the
    /// identity that fingerprint's layouts reference.
    ///
    /// This is the user-driven half of attach-time resolution: replaced
    /// hardware arrives under a fresh key, so no pin can heal the old
    /// binding automatically, and the user names the device that should
    /// inherit it. The device's key is re-pinned to the adopted
    /// fingerprint, so the decision holds across restarts once the
    /// overlay is persisted.
    ///
    /// A non-renderable device already holding the target fingerprint is
    /// treated as the replaced predecessor: it is removed, and its user
    /// settings (name, enabled, brightness) migrate to the device
    /// inheriting its identity.
    pub async fn rebind_portable_identity(
        &self,
        id: &DeviceId,
        fingerprint: DeviceFingerprint,
    ) -> Result<TrackedDevice, PortableRebindError> {
        let mut inner = self.inner.write().await;

        if !inner.devices.contains_key(id) {
            return Err(PortableRebindError::UnknownDevice);
        }
        let claim = inner
            .claims_by_id
            .get(id)
            .cloned()
            .ok_or(PortableRebindError::Unclaimed)?;

        let mut inherited_settings = None;
        if let Some(&holder_id) = inner.fingerprints.get(&fingerprint)
            && holder_id != *id
        {
            let holder_is_renderable = inner
                .devices
                .get(&holder_id)
                .is_some_and(|holder| holder.state.is_renderable());
            if holder_is_renderable {
                return Err(PortableRebindError::TargetActive);
            }
            if let Some(replaced) = inner.devices.remove(&holder_id) {
                inherited_settings = Some(replaced.user_settings);
            }
            inner.id_to_fingerprint.remove(&holder_id);
            inner.fingerprints.retain(|_, mapped| *mapped != holder_id);
            inner.metadata_by_id.remove(&holder_id);
            inner.claims_by_id.remove(&holder_id);
        }

        if let Some(previous) = inner.id_to_fingerprint.insert(*id, fingerprint.clone()) {
            inner.fingerprints.remove(&previous);
        }
        inner.fingerprints.insert(fingerprint.clone(), *id);
        inner
            .portable_key_pins
            .insert(claim.key().clone(), fingerprint);

        let entry = inner
            .devices
            .get_mut(id)
            .expect("device presence was checked above");
        if let Some(settings) = inherited_settings {
            entry.user_settings = settings;
            apply_user_settings_to_info(&mut entry.info, &entry.user_settings);
        }
        bump_device_revision(entry);
        let snapshot = entry.clone();
        self.bump_generation();

        info!(
            device_id = %id,
            key = %claim.key(),
            "Device re-bound onto an inherited portable identity"
        );
        Ok(snapshot)
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

/// Resolves the effective fingerprint for a claimed device.
///
/// The pin for a portable key is the durable identity: a claimed device
/// arriving under a new fingerprint adopts the pinned one, which is what
/// makes a cable move or a restart land on the identity its layouts
/// reference. Collision detection runs first, because pin resolution is
/// deduplication and the RFC requires detection before dedup: two
/// simultaneously present units claiming one key must quarantine the key
/// rather than silently merge.
fn resolve_portable_fingerprint(
    inner: &mut RegistryInner,
    fingerprint: DeviceFingerprint,
    claim: Option<&PortableIdentityClaim>,
) -> DeviceFingerprint {
    let Some(claim) = claim else {
        return fingerprint;
    };
    if inner.quarantined_keys.contains(claim.key()) {
        return fingerprint;
    }

    let Some(pinned) = inner.portable_key_pins.get(claim.key()).cloned() else {
        inner
            .portable_key_pins
            .insert(claim.key().clone(), fingerprint.clone());
        return fingerprint;
    };
    if pinned == fingerprint {
        return fingerprint;
    }

    // Only a renderable holder proves simultaneous presence. Reconnecting
    // is exactly the state a cable move passes through, and treating it as
    // present would quarantine a healthy key on every port change.
    let holder_present_claim = inner
        .fingerprints
        .get(&pinned)
        .and_then(|holder_id| {
            let holder = inner.devices.get(holder_id)?;
            holder.state.is_renderable().then_some(holder_id)
        })
        .and_then(|holder_id| {
            inner
                .claims_by_id
                .get(holder_id)
                .map(|held| (*holder_id, held.clone()))
        });

    if let Some((holder_id, held_claim)) = holder_present_claim
        && held_claim
            .evidence()
            .proves_distinct_from(&claim.evidence())
    {
        warn!(
            key = %claim.key(),
            existing_device = %holder_id,
            existing_evidence = ?held_claim.evidence(),
            incoming_evidence = ?claim.evidence(),
            "Two simultaneously present devices claim one portable key; quarantining"
        );
        inner.collision_log.push(PortableKeyCollision {
            key: claim.key().clone(),
            existing_device: holder_id,
            existing_fingerprint: pinned,
            existing_claim: held_claim,
            incoming_fingerprint: fingerprint.clone(),
            incoming_claim: claim.clone(),
        });
        inner.quarantined_keys.insert(claim.key().clone());
        inner.portable_key_pins.remove(claim.key());
        return fingerprint;
    }

    debug!(
        key = %claim.key(),
        raw_fingerprint = %fingerprint.as_str(),
        pinned_fingerprint = %pinned.as_str(),
        "Portable key resolved a re-attached device to its pinned identity"
    );
    pinned
}

/// Stores a claim on a device, keeping the key pins consistent.
///
/// `None` preserves whatever was recorded: a scanner that cannot prove
/// identity on this pass must not erase history a previous pass proved.
fn store_portable_claim(
    inner: &mut RegistryInner,
    id: DeviceId,
    claim: Option<PortableIdentityClaim>,
) {
    let Some(claim) = claim else {
        return;
    };

    // A firmware update can change the serial: same device, new key. The
    // old key stops resolving here; lineage is the sync layer's job.
    if let Some(previous) = inner.claims_by_id.get(&id)
        && previous.key() != claim.key()
    {
        inner.portable_key_pins.remove(previous.key());
    }

    if !inner.quarantined_keys.contains(claim.key())
        && !inner.portable_key_pins.contains_key(claim.key())
        && let Some(canonical) = inner.id_to_fingerprint.get(&id)
    {
        inner
            .portable_key_pins
            .insert(claim.key().clone(), canonical.clone());
    }

    inner.claims_by_id.insert(id, claim);
}

fn bump_device_revision(entry: &mut TrackedDevice) {
    entry.revision = entry.revision.saturating_add(1);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SmBusDramIdentity {
    bus_path: String,
    controller_kind: String,
    firmware_name: String,
    led_count: u32,
}

fn find_single_renderable_smbus_dram_match(
    inner: &RegistryInner,
    info: &DeviceInfo,
    metadata: &HashMap<String, String>,
) -> Option<DeviceId> {
    let target = smbus_dram_identity(info, metadata)?;
    let mut matches = inner.devices.iter().filter_map(|(id, tracked)| {
        if !(tracked.state.is_renderable() || tracked.state == DeviceState::Reconnecting) {
            return None;
        }
        let metadata = inner.metadata_by_id.get(id)?;
        (smbus_dram_identity(&tracked.info, metadata).as_ref() == Some(&target)).then_some(*id)
    });

    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn smbus_dram_identity(
    info: &DeviceInfo,
    metadata: &HashMap<String, String>,
) -> Option<SmBusDramIdentity> {
    if info.connection_type != ConnectionType::SmBus
        || info.model.as_deref() != Some("asus_aura_smbus_dram")
    {
        return None;
    }

    let controller_kind = metadata.get("controller_kind")?;
    if controller_kind != "dram" {
        return None;
    }

    Some(SmBusDramIdentity {
        bus_path: metadata.get("bus_path")?.clone(),
        controller_kind: controller_kind.clone(),
        firmware_name: metadata
            .get("firmware_name")
            .cloned()
            .or_else(|| info.firmware_version.clone())?,
        led_count: info.total_led_count(),
    })
}

fn preserve_renderable_device_shape(
    incoming: &mut DeviceInfo,
    existing: &DeviceInfo,
    state: &DeviceState,
) {
    if !state.is_renderable() {
        return;
    }

    let incoming_has_shape = !incoming.segments.is_empty()
        || incoming.capabilities.led_count > 0
        || incoming.capabilities.has_display;
    let existing_has_shape = !existing.segments.is_empty()
        || existing.capabilities.led_count > 0
        || existing.capabilities.has_display;

    if incoming_has_shape || !existing_has_shape {
        return;
    }

    incoming.segments.clone_from(&existing.segments);
    incoming.capabilities = existing.capabilities;
}
