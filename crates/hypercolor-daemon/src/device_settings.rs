//! Persisted output settings: global brightness plus per-device user settings.

mod migration;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use hypercolor_core::device::DeviceRegistry;
use hypercolor_types::controls::ControlValueMap;
use hypercolor_types::device::DeviceId;
use serde::{Deserialize, Serialize};
use tokio::sync::OwnedRwLockWriteGuard;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::domain::device_binding::{DeviceBindingRemaps, MigrationPersistence};
use crate::path_migration::{
    MigratedStore, MigrationOutcome, PathMigrationEntry, VersionedDocument, migrate,
};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, AtomicWriteReservation,
    PersistenceError, serialize_json_pretty, write_atomic,
};

const STORE_SUBJECT: &str = "device settings";

/// Current schema for `device-settings.json`. Version 3 stores driver control
/// values in the canonical control algebra. Device rows live under the
/// portable key when the hardware has one, else the local fingerprint, with
/// UUID-shaped legacy rows retained as machine-scoped configuration.
pub const DEVICE_SETTINGS_SCHEMA_VERSION: u32 = 3;

/// An unversioned file predates the envelope and is always v1, never
/// "current": assuming current is how an old file gets rewritten in a
/// new shape and loses data.
const fn legacy_schema_version() -> u32 {
    1
}

fn default_brightness() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredDeviceSettings {
    pub name: Option<String>,
    pub disabled: bool,
    #[serde(default = "default_brightness")]
    pub brightness: f32,
}

impl Default for StoredDeviceSettings {
    fn default() -> Self {
        Self {
            name: None,
            disabled: false,
            brightness: default_brightness(),
        }
    }
}

impl StoredDeviceSettings {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.name = self
            .name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        self.brightness = self.brightness.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.name.is_none() && !self.disabled && self.brightness >= 0.999
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
struct PersistedSettingsSnapshot {
    #[serde(default = "legacy_schema_version")]
    schema_version: u32,
    #[serde(default = "default_brightness")]
    global_brightness: f32,
    devices: HashMap<String, StoredDeviceSettings>,
    driver_controls: HashMap<String, ControlValueMap>,
}

impl Default for PersistedSettingsSnapshot {
    fn default() -> Self {
        Self {
            schema_version: DEVICE_SETTINGS_SCHEMA_VERSION,
            global_brightness: default_brightness(),
            devices: HashMap::new(),
            driver_controls: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
struct LegacySettingsSnapshot {
    #[serde(default = "legacy_schema_version")]
    schema_version: u32,
    #[serde(default = "default_brightness")]
    global_brightness: f32,
    devices: HashMap<String, StoredDeviceSettings>,
    driver_controls: HashMap<String, BTreeMap<String, serde_json::Value>>,
}

impl Default for LegacySettingsSnapshot {
    fn default() -> Self {
        Self {
            schema_version: legacy_schema_version(),
            global_brightness: default_brightness(),
            devices: HashMap::new(),
            driver_controls: HashMap::new(),
        }
    }
}

/// Version probe read before the full parse, so a newer file's contents
/// are never interpreted through this build's field definitions.
#[derive(Deserialize)]
struct SchemaProbe {
    #[serde(default = "legacy_schema_version")]
    schema_version: u32,
}

#[derive(Debug, Clone, Copy)]
enum FutureDocumentPolicy {
    ReadOnly,
    Reject,
}

#[derive(Debug, Clone)]
struct DeviceSettingsDocument {
    snapshot: PersistedSettingsSnapshot,
    source_version: u32,
    refuse_writes: Option<String>,
}

/// JSON-backed per-device settings store.
#[derive(Debug, Clone)]
pub struct DeviceSettingsStore {
    path: PathBuf,
    snapshot: PersistedSettingsSnapshot,
    /// Set when the on-disk file must not be rewritten (its schema is
    /// newer than this build understands). Every save refuses loudly:
    /// defaulting fields we cannot parse and writing them back would
    /// destroy the newer client's data.
    refuse_writes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeviceSettingsAccess {
    store: Arc<RwLock<DeviceSettingsStore>>,
}

pub(crate) struct DeviceSettingsBindingMigration {
    source: PersistedSettingsSnapshot,
    candidate: PersistedSettingsSnapshot,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
    migrated: usize,
}

pub(crate) struct AdmittedDeviceSettingsBindingMigration {
    source: PersistedSettingsSnapshot,
    candidate: PersistedSettingsSnapshot,
    write: AdmittedAtomicWrite,
    migrated: usize,
}

pub(crate) struct PersistedDeviceSettingsBindingMigration {
    source: PersistedSettingsSnapshot,
    candidate: PersistedSettingsSnapshot,
    migrated: usize,
}

pub(crate) struct DeviceSettingsBindingPublication {
    store: OwnedRwLockWriteGuard<DeviceSettingsStore>,
    candidate: Option<PersistedSettingsSnapshot>,
    migrated: usize,
}

#[derive(Debug)]
pub(crate) enum BrightnessPersistence {
    Durable,
    Retrying(PersistenceError),
}

impl DeviceSettingsAccess {
    pub(crate) fn new(store: Arc<RwLock<DeviceSettingsStore>>) -> Self {
        Self { store }
    }

    pub async fn device_settings_for_key(&self, key: &str) -> Option<StoredDeviceSettings> {
        self.store.read().await.device_settings_for_key(key)
    }

    pub async fn persist_device_settings(
        &self,
        key: &str,
        settings: StoredDeviceSettings,
    ) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        store.set_device_settings(key, settings);
        store.save()
    }

    pub async fn driver_control_values_for_key(
        &self,
        key: &str,
    ) -> anyhow::Result<ControlValueMap> {
        self.store.read().await.driver_control_values_for_key(key)
    }

    pub async fn persist_driver_control_values(
        &self,
        key: &str,
        values: ControlValueMap,
    ) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        store.set_driver_control_values(key, values)?;
        store.save()
    }

    pub(crate) async fn prepare_binding_migration(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<Option<DeviceSettingsBindingMigration>> {
        self.store.read().await.prepare_binding_migration(remaps)
    }

    pub(crate) async fn persisted_binding_keys(&self) -> std::collections::HashSet<String> {
        let store = self.store.read().await;
        store
            .snapshot
            .devices
            .keys()
            .chain(store.snapshot.driver_controls.keys())
            .cloned()
            .collect()
    }

    pub(crate) async fn prepare_binding_publication(
        &self,
        migration: PersistedDeviceSettingsBindingMigration,
    ) -> anyhow::Result<DeviceSettingsBindingPublication> {
        let store = Arc::clone(&self.store).write_owned().await;
        anyhow::ensure!(
            store.snapshot == migration.source,
            "device binding migration was superseded by newer device settings"
        );
        Ok(DeviceSettingsBindingPublication {
            store,
            candidate: Some(migration.candidate),
            migrated: migration.migrated,
        })
    }
}

impl DeviceSettingsStore {
    /// Create an empty store rooted at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            snapshot: PersistedSettingsSnapshot::default(),
            refuse_writes: None,
        }
    }

    /// Load an existing store or create an empty one when absent.
    ///
    /// A v1 or v2 file migrates to the current schema, leaving the previous
    /// file as `device-settings.pre-v3.bak`; keys are not
    /// reclassified from disk, because a persisted string cannot prove
    /// what it was derived from — rows move to canonical keys as live
    /// devices prove their identities. A file whose schema is newer
    /// than this build loads as an empty read-only store: it is not
    /// parsed, not migrated, and never written back.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let document = read_settings_document(path, FutureDocumentPolicy::ReadOnly)?;
        let loaded_version = document.source_version;
        let mut store = Self {
            path: path.to_path_buf(),
            snapshot: document.snapshot,
            refuse_writes: document.refuse_writes,
        };
        store.normalize()?;

        if loaded_version < DEVICE_SETTINGS_SCHEMA_VERSION {
            let backup = back_up_content_schema(path)?;
            store.snapshot.schema_version = DEVICE_SETTINGS_SCHEMA_VERSION;
            store
                .save()
                .context("failed to persist migrated device settings")?;
            info!(
                path = %path.display(),
                backup = %backup.display(),
                from = loaded_version,
                to = DEVICE_SETTINGS_SCHEMA_VERSION,
                "Migrated device settings schema"
            );
        }

        Ok(store)
    }

    /// Relocate a legacy data-tier store and open the state-tier document.
    ///
    /// Content schemas v1 and v2 canonicalize before the path import. A future
    /// canonical schema remains byte-preserved and read-only, while a future
    /// legacy schema is refused because this build cannot safely rewrite it.
    ///
    /// # Errors
    ///
    /// Returns an error when either document cannot be read or canonicalized,
    /// the state destination cannot be prepared, or legacy retirement fails.
    pub fn load_migrated(
        legacy_path: &Path,
        canonical_path: &Path,
    ) -> anyhow::Result<(Self, MigrationOutcome)> {
        let writer = AtomicFileWriter::new(canonical_path).with_context(|| {
            format!(
                "failed to prepare device settings store at {}",
                canonical_path.display()
            )
        })?;
        let entry = PathMigrationEntry::new(
            STORE_SUBJECT,
            legacy_path.to_path_buf(),
            canonical_path.to_path_buf(),
        );
        let migrated = migrate(&DeviceSettingsCodec, &entry, &writer)?;
        let outcome = migrated.outcome;
        let document = migrated.document.unwrap_or_else(empty_settings_document);
        let source_version = document.source_version;
        let mut store = Self {
            path: canonical_path.to_path_buf(),
            snapshot: document.snapshot,
            refuse_writes: document.refuse_writes,
        };
        store.normalize()?;

        if matches!(outcome, MigrationOutcome::AlreadyMigrated)
            && source_version < DEVICE_SETTINGS_SCHEMA_VERSION
        {
            back_up_content_schema(canonical_path)?;
            store
                .save()
                .context("failed to persist migrated device settings")?;
        }

        Ok((store, outcome))
    }

    /// Return the configured global brightness scalar.
    #[must_use]
    pub fn global_brightness(&self) -> f32 {
        self.snapshot.global_brightness.clamp(0.0, 1.0)
    }

    pub(crate) fn persist_global_brightness(
        &mut self,
        _authority: &crate::output_power::BrightnessMutationAuthority,
        brightness: f32,
    ) -> anyhow::Result<BrightnessPersistence> {
        if let Some(reason) = &self.refuse_writes {
            anyhow::bail!("device settings were not persisted: {reason}");
        }

        let mut next = self.snapshot.clone();
        next.global_brightness = brightness.clamp(0.0, 1.0);
        let payload = serialize_settings_snapshot(&next)?;
        let rollback_payload = serialize_settings_snapshot(&self.snapshot)?;
        let writer = AtomicFileWriter::new(&self.path).with_context(|| {
            format!(
                "failed to prepare device settings persistence at {}",
                self.path.display()
            )
        })?;
        let outcome = writer.reserve().admit(payload).commit_stage_aware();

        match outcome {
            AtomicWriteCommitResult::DurableWritten => {
                self.snapshot = next;
                Ok(BrightnessPersistence::Durable)
            }
            AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                self.snapshot = next;
                Ok(BrightnessPersistence::Retrying(error))
            }
            AtomicWriteCommitResult::FailedBeforeReplacement(error) => {
                let rollback = writer
                    .reserve()
                    .admit(rollback_payload)
                    .commit_stage_aware();
                if matches!(rollback, AtomicWriteCommitResult::Superseded) {
                    anyhow::bail!(
                        "global brightness rollback was superseded after persistence failed: \
                         {error}"
                    );
                }
                Err(error).context("failed to persist global brightness")
            }
            AtomicWriteCommitResult::Superseded => {
                anyhow::bail!("global brightness persistence was superseded before publication")
            }
        }
    }

    /// Return stored settings for a persisted device settings key.
    #[must_use]
    pub fn device_settings_for_key(&self, key: &str) -> Option<StoredDeviceSettings> {
        self.snapshot
            .devices
            .get(key)
            .cloned()
            .map(StoredDeviceSettings::normalized)
    }

    /// Update all persisted settings for a persisted device settings key.
    pub fn set_device_settings(&mut self, key: &str, settings: StoredDeviceSettings) {
        let normalized = settings.normalized();
        if normalized.is_default() {
            self.snapshot.devices.remove(key);
        } else {
            self.snapshot.devices.insert(key.to_owned(), normalized);
        }
    }

    /// Persist just the device brightness scalar.
    pub fn set_device_brightness(&mut self, key: &str, brightness: f32) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.brightness = brightness;
        self.set_device_settings(key, settings);
    }

    /// Persist just the device name override.
    pub fn set_device_name(&mut self, key: &str, name: Option<String>) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.name = name;
        self.set_device_settings(key, settings);
    }

    /// Persist just the device enabled flag.
    pub fn set_device_enabled(&mut self, key: &str, enabled: bool) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.disabled = !enabled;
        self.set_device_settings(key, settings);
    }

    pub fn driver_control_values_for_key(&self, key: &str) -> anyhow::Result<ControlValueMap> {
        Ok(self
            .snapshot
            .driver_controls
            .get(key)
            .cloned()
            .unwrap_or_default())
    }

    pub fn set_driver_control_values(
        &mut self,
        key: &str,
        values: ControlValueMap,
    ) -> anyhow::Result<()> {
        if values.is_empty() {
            self.snapshot.driver_controls.remove(key);
        } else {
            validate_control_map(key, &values)?;
            self.snapshot.driver_controls.insert(key.to_owned(), values);
        }
        Ok(())
    }

    /// Move a device row persisted under a legacy key to its canonical
    /// key. Returns whether a row moved, in which case the caller
    /// should save.
    ///
    /// A row already present under the canonical key wins outright, and
    /// legacy rows are then left in place rather than deleted: they are
    /// stale, but discarding them would silently drop configuration.
    pub fn adopt_legacy_device_key(&mut self, canonical: &str, legacy_keys: &[String]) -> bool {
        if self.snapshot.devices.contains_key(canonical) {
            return false;
        }
        for legacy in legacy_keys {
            if legacy == canonical {
                continue;
            }
            if let Some(settings) = self.snapshot.devices.remove(legacy) {
                self.snapshot.devices.insert(canonical.to_owned(), settings);
                info!(
                    from = %legacy,
                    to = %canonical,
                    "Migrated device settings row to its canonical key"
                );
                return true;
            }
        }
        false
    }

    /// Save the current snapshot to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(reason) = &self.refuse_writes {
            anyhow::bail!("device settings were not persisted: {reason}");
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create device settings directory {}",
                    parent.display()
                )
            })?;
        }

        let payload = serialize_settings_snapshot(&self.snapshot)?;
        write_atomic(&self.path, &payload).context("failed to persist device settings")?;

        Ok(())
    }

    fn prepare_binding_migration(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<Option<DeviceSettingsBindingMigration>> {
        if let Some(reason) = &self.refuse_writes {
            anyhow::bail!("device settings were not migrated: {reason}");
        }
        let source = self.snapshot.clone();
        let mut candidate = source.clone();
        let migrated = remap_settings_keys(&mut candidate.devices, &remaps.persisted_setting_keys)
            + remap_settings_keys(
                &mut candidate.driver_controls,
                &remaps.persisted_setting_keys,
            );
        if migrated == 0 {
            return Ok(None);
        }
        let payload = serialize_settings_snapshot(&candidate)?;
        let writer = AtomicFileWriter::new(&self.path)?;
        Ok(Some(DeviceSettingsBindingMigration {
            source,
            candidate,
            write: writer.reserve(),
            payload,
            migrated,
        }))
    }

    fn normalize(&mut self) -> anyhow::Result<()> {
        self.snapshot.global_brightness = self.snapshot.global_brightness.clamp(0.0, 1.0);
        self.snapshot.devices.retain(|_, settings| {
            *settings = settings.clone().normalized();
            !settings.is_default()
        });
        self.snapshot
            .driver_controls
            .retain(|_, values| !values.is_empty());
        for (device_key, values) in &self.snapshot.driver_controls {
            validate_control_map(device_key, values)?;
        }
        Ok(())
    }
}

impl DeviceSettingsBindingMigration {
    pub(crate) fn admit(self) -> AdmittedDeviceSettingsBindingMigration {
        AdmittedDeviceSettingsBindingMigration {
            source: self.source,
            candidate: self.candidate,
            write: self.write.admit(self.payload),
            migrated: self.migrated,
        }
    }
}

impl AdmittedDeviceSettingsBindingMigration {
    pub(crate) fn persist(
        self,
    ) -> (
        PersistedDeviceSettingsBindingMigration,
        MigrationPersistence,
    ) {
        let persistence = MigrationPersistence::from_commit(self.write.commit_stage_aware());
        (
            PersistedDeviceSettingsBindingMigration {
                source: self.source,
                candidate: self.candidate,
                migrated: self.migrated,
            },
            persistence,
        )
    }
}

impl DeviceSettingsBindingPublication {
    pub(crate) fn publish(&mut self) -> usize {
        self.store.snapshot = self
            .candidate
            .take()
            .expect("device settings binding migration must publish exactly once");
        self.migrated
    }
}

fn remap_settings_keys<T>(
    values: &mut HashMap<String, T>,
    remaps: &HashMap<String, String>,
) -> usize {
    let mut migrated = 0;
    for (legacy, canonical) in remaps {
        let Some(value) = values.remove(legacy) else {
            continue;
        };
        values.entry(canonical.clone()).or_insert(value);
        migrated += 1;
    }
    migrated
}

fn empty_settings_document() -> DeviceSettingsDocument {
    DeviceSettingsDocument {
        snapshot: PersistedSettingsSnapshot::default(),
        source_version: DEVICE_SETTINGS_SCHEMA_VERSION,
        refuse_writes: None,
    }
}

fn read_settings_document(
    path: &Path,
    future_policy: FutureDocumentPolicy,
) -> anyhow::Result<DeviceSettingsDocument> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read device settings at {}", path.display()))?;
    let probe = serde_json::from_str::<SchemaProbe>(&raw)
        .with_context(|| format!("failed to probe device settings at {}", path.display()))?;
    let source_version = probe.schema_version;
    if source_version > DEVICE_SETTINGS_SCHEMA_VERSION {
        let reason = format!(
            "device-settings.json is schema v{source_version}, newer than the supported v{DEVICE_SETTINGS_SCHEMA_VERSION}; refusing to read or rewrite it"
        );
        return match future_policy {
            FutureDocumentPolicy::ReadOnly => {
                tracing::error!(path = %path.display(), "{reason}");
                Ok(DeviceSettingsDocument {
                    snapshot: PersistedSettingsSnapshot::default(),
                    source_version,
                    refuse_writes: Some(reason),
                })
            }
            FutureDocumentPolicy::Reject => anyhow::bail!("{reason}"),
        };
    }

    let snapshot = if source_version < DEVICE_SETTINGS_SCHEMA_VERSION {
        let legacy = serde_json::from_str::<LegacySettingsSnapshot>(&raw)
            .with_context(|| format!("failed to parse device settings at {}", path.display()))?;
        PersistedSettingsSnapshot {
            schema_version: DEVICE_SETTINGS_SCHEMA_VERSION,
            global_brightness: legacy.global_brightness,
            devices: legacy.devices,
            driver_controls: migration::migrate_v2_driver_controls(legacy.driver_controls)?,
        }
    } else {
        serde_json::from_str::<PersistedSettingsSnapshot>(&raw)
            .with_context(|| format!("failed to parse device settings at {}", path.display()))?
    };

    Ok(DeviceSettingsDocument {
        snapshot,
        source_version,
        refuse_writes: None,
    })
}

fn serialize_settings_snapshot(snapshot: &PersistedSettingsSnapshot) -> anyhow::Result<Vec<u8>> {
    serialize_json_pretty(&PersistedSettingsSnapshot {
        schema_version: DEVICE_SETTINGS_SCHEMA_VERSION,
        global_brightness: snapshot.global_brightness.clamp(0.0, 1.0),
        devices: snapshot
            .devices
            .iter()
            .map(|(key, settings)| (key.clone(), settings.clone().normalized()))
            .collect(),
        driver_controls: snapshot.driver_controls.clone(),
    })
    .context("failed to serialize device settings")
}

struct DeviceSettingsCodec;

impl MigratedStore for DeviceSettingsCodec {
    type Document = DeviceSettingsDocument;
    type Error = anyhow::Error;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        let document = read_settings_document(path, FutureDocumentPolicy::ReadOnly)?;
        Ok(VersionedDocument::new(document.source_version, document))
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        let document = read_settings_document(path, FutureDocumentPolicy::Reject)?;
        Ok(Some(VersionedDocument::new(
            document.source_version,
            document,
        )))
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_settings_snapshot(&document.snapshot)
    }
}

fn back_up_content_schema(path: &Path) -> anyhow::Result<PathBuf> {
    let backup = migration_backup_path(path);
    fs::copy(path, &backup)
        .with_context(|| format!("failed to back up device settings to {}", backup.display()))?;
    Ok(backup)
}

fn validate_control_map(device_key: &str, values: &ControlValueMap) -> anyhow::Result<()> {
    for (control_id, value) in values {
        value.validate().with_context(|| {
            format!("device settings control '{control_id}' for '{device_key}' is invalid")
        })?;
    }
    Ok(())
}

/// The backup name for a schema migration, suffixed when it already
/// exists: an existing backup is from the user's last upgrade, and
/// overwriting it would destroy the only copy of an older state.
fn migration_backup_path(path: &Path) -> PathBuf {
    let base = path.with_extension(format!("pre-v{DEVICE_SETTINGS_SCHEMA_VERSION}.bak"));
    if !base.exists() {
        return base;
    }
    for attempt in 2_u32.. {
        let candidate = path.with_extension(format!(
            "pre-v{DEVICE_SETTINGS_SCHEMA_VERSION}.{attempt}.bak"
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused backup suffix always exists")
}

/// The canonical persisted-settings key for a device, with the legacy
/// keys a row may still live under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSettingsKeys {
    /// Portable key when the device has a claim, else the local
    /// fingerprint, else the raw device id. A UUID-shaped key is
    /// unattributable to hardware but still names a row some user's
    /// configuration lives under; it stays machine-scoped and is never
    /// promoted.
    pub canonical: String,

    /// Keys the same device's row may have been persisted under by
    /// earlier builds, in preference order.
    pub legacy: Vec<String>,
}

/// Derive the settings keys for a device from live registry state.
pub async fn device_settings_keys(
    registry: &DeviceRegistry,
    device_id: DeviceId,
) -> DeviceSettingsKeys {
    // A quarantined key is proven to name two devices, so it cannot key
    // either one's settings: sharing the row would let each unit
    // overwrite the other's configuration. Both fall back to their
    // fingerprints, and any row already under the quarantined key is
    // left untouched as ambiguous history.
    let claim_key = match registry.claim_for_id(&device_id).await {
        Some(claim) if !registry.is_portable_key_quarantined(claim.key()).await => {
            Some(claim.key().to_string())
        }
        _ => None,
    };
    let fingerprint = registry
        .fingerprint_for_id(&device_id)
        .await
        .map(|fingerprint| fingerprint.to_string());
    let raw_id = device_id.to_string();

    match (claim_key, fingerprint) {
        (Some(canonical), Some(fingerprint)) => {
            let mut legacy = Vec::new();
            if fingerprint != canonical {
                legacy.push(fingerprint);
            }
            legacy.push(raw_id);
            DeviceSettingsKeys { canonical, legacy }
        }
        (Some(canonical), None) | (None, Some(canonical)) => DeviceSettingsKeys {
            canonical,
            legacy: vec![raw_id],
        },
        (None, None) => DeviceSettingsKeys {
            canonical: raw_id,
            legacy: Vec::new(),
        },
    }
}

/// Derive the canonical settings key for a device and migrate any row
/// persisted under one of its legacy keys, saving when a row moved.
///
/// This is the live half of the v2 key normalization: disk-time
/// migration cannot classify keys, so rows move to canonical keys as
/// attached hardware proves what it is.
pub async fn resolve_device_settings_key(
    registry: &DeviceRegistry,
    access: &DeviceSettingsAccess,
    device_id: DeviceId,
) -> String {
    let keys = device_settings_keys(registry, device_id).await;
    if !keys.legacy.is_empty() {
        let mut guard = access.store.write().await;
        if guard.adopt_legacy_device_key(&keys.canonical, &keys.legacy)
            && let Err(error) = guard.save()
        {
            warn!(%error, "Migrated device settings key was not persisted");
        }
    }
    keys.canonical
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceFamily, DeviceFingerprint, DeviceInfo,
        DeviceOrigin,
    };
    use hypercolor_types::portable::{NetworkAttachment, PortableIdentityClaim};
    use tempfile::TempDir;

    use super::*;

    fn store_path(dir: &TempDir) -> PathBuf {
        dir.path().join("device-settings.json")
    }

    fn v1_payload() -> &'static str {
        r#"{
            "global_brightness": 0.8,
            "devices": {
                "net:wled:aaa": { "name": "Shelf", "disabled": false, "brightness": 0.5 },
                "5f2b1f9e-8f1a-4d0a-9a37-0f4c7c2f9d11": { "name": "Mystery", "disabled": true, "brightness": 1.0 }
            },
            "driver_controls": {}
        }"#
    }

    async fn claimed_registry(fingerprint: &str) -> (DeviceRegistry, DeviceId, String) {
        let registry = DeviceRegistry::new();
        let info = DeviceInfo {
            id: DeviceId::new(),
            name: "Shelf".to_owned(),
            vendor: "test".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        };
        let claim = PortableIdentityClaim::mac_address(
            "2C:F4:32:44:55:66",
            NetworkAttachment::Peer(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))),
        )
        .expect("valid MAC");
        let portable_key = claim.key().to_string();
        let id = registry
            .add_discovered(DiscoveredDevice {
                fingerprint: DeviceFingerprint::from_persisted(fingerprint.to_owned()),
                connect_behavior: DiscoveryConnectBehavior::Deferred,
                info,
                metadata: HashMap::new(),
                claim: Some(claim),
            })
            .await;
        (registry, id, portable_key)
    }

    #[test]
    fn v1_file_migrates_with_backup_and_keeps_rows() {
        let dir = TempDir::new().expect("tempdir");
        let path = store_path(&dir);
        fs::write(&path, v1_payload()).expect("seed v1 file");

        let store = DeviceSettingsStore::load(&path).expect("v1 loads");
        assert!(store.device_settings_for_key("net:wled:aaa").is_some());
        assert!(
            store
                .device_settings_for_key("5f2b1f9e-8f1a-4d0a-9a37-0f4c7c2f9d11")
                .is_some(),
            "UUID rows are retained as machine-scoped configuration"
        );

        let migrated = fs::read_to_string(&path).expect("file rewritten");
        assert!(migrated.contains("\"schema_version\": 3"));
        assert!(dir.path().join("device-settings.pre-v3.bak").exists());

        // A second v1 appearance must not clobber the first backup.
        fs::write(&path, v1_payload()).expect("reseed v1 file");
        DeviceSettingsStore::load(&path).expect("second migration");
        assert!(dir.path().join("device-settings.pre-v3.2.bak").exists());
        assert!(dir.path().join("device-settings.pre-v3.bak").exists());
    }

    #[test]
    fn newer_schema_loads_read_only_and_never_writes_back() {
        let dir = TempDir::new().expect("tempdir");
        let path = store_path(&dir);
        let newer = r#"{ "schema_version": 4, "future_field": true }"#;
        fs::write(&path, newer).expect("seed newer file");

        let mut store = DeviceSettingsStore::load(&path).expect("newer file loads safely");
        store.set_device_brightness("net:wled:aaa", 0.5);
        let error = store.save().expect_err("writes are refused");
        assert!(error.to_string().contains("schema v4"));
        assert_eq!(
            fs::read_to_string(&path).expect("file survives"),
            newer,
            "the newer client's data must remain byte-identical"
        );
    }

    #[tokio::test]
    async fn resolve_migrates_legacy_row_to_portable_key() {
        let dir = TempDir::new().expect("tempdir");
        let path = store_path(&dir);
        let mut seeded = DeviceSettingsStore::new(path.clone());
        seeded.set_device_settings(
            "net:wled:aaa",
            StoredDeviceSettings {
                name: Some("Shelf".to_owned()),
                disabled: false,
                brightness: 0.5,
            },
        );
        seeded.save().expect("seed saves");

        let (registry, device_id, portable_key) = claimed_registry("net:wled:aaa").await;
        let access = DeviceSettingsAccess::new(Arc::new(RwLock::new(
            DeviceSettingsStore::load(&path).expect("store loads"),
        )));

        let key = resolve_device_settings_key(&registry, &access, device_id).await;
        assert_eq!(key, portable_key);

        assert!(
            access
                .device_settings_for_key(&portable_key)
                .await
                .is_some(),
            "the row follows the device onto its canonical key"
        );
        assert!(
            access
                .device_settings_for_key("net:wled:aaa")
                .await
                .is_none()
        );

        let persisted = DeviceSettingsStore::load(&path).expect("reload");
        assert!(persisted.device_settings_for_key(&portable_key).is_some());
    }

    #[tokio::test]
    async fn claimless_device_resolves_to_its_fingerprint() {
        let registry = DeviceRegistry::new();
        let info = DeviceInfo {
            id: DeviceId::new(),
            name: "Anon".to_owned(),
            vendor: "test".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        };
        let id = registry
            .add_discovered(DiscoveredDevice {
                fingerprint: DeviceFingerprint::from_persisted("net:wled:anon".to_owned()),
                connect_behavior: DiscoveryConnectBehavior::Deferred,
                info,
                metadata: HashMap::new(),
                claim: None,
            })
            .await;

        let keys = device_settings_keys(&registry, id).await;
        assert_eq!(keys.canonical, "net:wled:anon");
        assert_eq!(keys.legacy, vec![id.to_string()]);
    }

    #[tokio::test]
    async fn quarantined_key_devices_fall_back_to_their_fingerprints() {
        // Two simultaneously present units claim one key: the registry
        // quarantines it, and neither unit may key settings by it.
        let registry = DeviceRegistry::new();
        let discovered = |name: &str, fingerprint: &str, peer_octet: u8| DiscoveredDevice {
            fingerprint: DeviceFingerprint::from_persisted(fingerprint.to_owned()),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            info: DeviceInfo {
                id: DeviceId::new(),
                name: name.to_owned(),
                vendor: "test".to_owned(),
                family: DeviceFamily::new_static("wled", "WLED"),
                model: None,
                connection_type: ConnectionType::Network,
                origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
                segments: Vec::new(),
                firmware_version: None,
                capabilities: DeviceCapabilities::default(),
            },
            metadata: HashMap::new(),
            claim: PortableIdentityClaim::mac_address(
                "2C:F4:32:77:88:99",
                NetworkAttachment::Peer(IpAddr::V4(Ipv4Addr::new(192, 168, 1, peer_octet))),
            ),
        };

        let unit_a = registry
            .add_discovered(discovered("Unit A", "net:wled:unit-a", 40))
            .await;
        registry
            .set_state(&unit_a, hypercolor_types::device::DeviceState::Connected)
            .await;
        let unit_b = registry
            .add_discovered(discovered("Unit B", "net:wled:unit-b", 41))
            .await;

        let keys_a = device_settings_keys(&registry, unit_a).await;
        let keys_b = device_settings_keys(&registry, unit_b).await;
        assert_eq!(keys_a.canonical, "net:wled:unit-a");
        assert_eq!(keys_b.canonical, "net:wled:unit-b");
        assert_ne!(
            keys_a.canonical, keys_b.canonical,
            "colliding units must never share a settings row"
        );
    }

    #[test]
    fn adopt_prefers_existing_canonical_row() {
        let dir = TempDir::new().expect("tempdir");
        let mut store = DeviceSettingsStore::new(store_path(&dir));
        store.set_device_settings(
            "net:aabbccddeeff",
            StoredDeviceSettings {
                name: Some("Canonical".to_owned()),
                disabled: false,
                brightness: 0.7,
            },
        );
        store.set_device_settings(
            "net:wled:legacy",
            StoredDeviceSettings {
                name: Some("Legacy".to_owned()),
                disabled: false,
                brightness: 0.3,
            },
        );

        let moved =
            store.adopt_legacy_device_key("net:aabbccddeeff", &["net:wled:legacy".to_owned()]);
        assert!(!moved, "an existing canonical row wins outright");
        assert_eq!(
            store
                .device_settings_for_key("net:aabbccddeeff")
                .and_then(|settings| settings.name),
            Some("Canonical".to_owned())
        );
        assert!(
            store.device_settings_for_key("net:wled:legacy").is_some(),
            "stale legacy rows are kept rather than deleted"
        );
    }
}
