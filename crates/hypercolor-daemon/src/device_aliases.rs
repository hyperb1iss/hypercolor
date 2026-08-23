//! Machine-scoped portable-key overlay: `device-aliases.json`.
//!
//! The alias table is the durable half of portable device identity. It
//! records which fingerprint each portable key is pinned to, so a claimed
//! device that re-attaches after a restart resolves back to the identity
//! its layouts reference, and it preserves the local collision evidence
//! that justifies a key quarantine. It is engine state that stands on its
//! own without any cloud account: it survives cable moves and BIOS
//! renumbering for a user who never signs in, and it never syncs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use hypercolor_core::device::{DeviceLifecycleManager, DeviceRegistry, PortableKeyCollision};
use hypercolor_types::device::DeviceFingerprint;
use hypercolor_types::portable::{
    PortableDeviceKey, PortableIdentityClaim, PortableIdentitySource,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::path_migration::{
    MigratedStore, MigrationOutcome, PathMigrationEntry, VersionedDocument, migrate,
};
use crate::persistence::{AtomicFileWriter, serialize_json_pretty, write_atomic};

/// File name of the overlay inside the daemon state directory.
pub const DEVICE_ALIASES_FILE: &str = "device-aliases.json";

const SCHEMA_VERSION: u32 = 2;
const STORE_SUBJECT: &str = "device aliases";

/// Persisted portable-key overlay.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAliasFile {
    /// Envelope version used to refuse unknown future documents.
    #[serde(default = "schema_version")]
    pub schema_version: u32,

    /// Portable key -> its pinned local identity.
    #[serde(default)]
    pub aliases: BTreeMap<PortableDeviceKey, DeviceAliasRecord>,

    /// Keys proven to name more than one physical device. Their alias
    /// records are kept, untouched: they are what a user needs to
    /// disentangle the devices, and quarantine only gates resolution.
    #[serde(default)]
    pub quarantined_keys: BTreeSet<PortableDeviceKey>,

    /// The local observations that justify each quarantine. Raw
    /// attachment evidence is machine-scoped and lives only here; uploads
    /// carry a derived assertion, never this data.
    #[serde(default)]
    pub collisions: Vec<PersistedKeyCollision>,
}

const fn schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Deserialize)]
struct AliasSchemaProbe {
    #[serde(default = "schema_version")]
    schema_version: u32,
}

/// One portable key's recorded local identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAliasRecord {
    /// Where the identity came from.
    pub source: PortableIdentitySource,

    /// The identity value as the device last reported it.
    pub raw: String,

    /// The fingerprint the key is pinned to: the durable local identity
    /// every later observation of this key resolves to.
    pub fingerprint: String,

    /// The layout binding id the pinned fingerprint derives, recorded so
    /// a re-bind request can match an orphaned layout binding back to
    /// this key after the hardware itself is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_device_id: Option<String>,

    /// Unix seconds when this key was first recorded on this machine.
    pub first_seen_epoch_s: u64,

    /// Unix seconds when hardware carrying this key was last observed.
    pub last_seen_epoch_s: u64,
}

/// A proven same-key collision, persisted with its full local evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedKeyCollision {
    /// The key both units claimed.
    pub key: PortableDeviceKey,

    /// Unix seconds when the collision was observed.
    pub observed_epoch_s: u64,

    /// The fingerprint of the unit that held the key.
    pub existing_fingerprint: String,

    /// The holder's claim, with its attachment evidence.
    pub existing_claim: PortableIdentityClaim,

    /// The fingerprint of the second unit.
    pub incoming_fingerprint: String,

    /// The second unit's claim, whose evidence differed.
    pub incoming_claim: PortableIdentityClaim,
}

/// Load the overlay from disk. A missing file is an empty overlay.
pub fn load(path: &Path) -> anyhow::Result<DeviceAliasFile> {
    if !path.exists() {
        return Ok(empty_alias_file());
    }

    DeviceAliasCodec::read(path).map(|document| document.document)
}

/// Relocate a legacy data-tier overlay and load the state-tier document.
///
/// # Errors
///
/// Returns an error when either document is unreadable or invalid, the state
/// destination cannot be prepared, or a durable import cannot be retired.
pub fn load_migrated(
    legacy_path: &Path,
    canonical_path: &Path,
) -> anyhow::Result<(DeviceAliasFile, MigrationOutcome)> {
    let writer = AtomicFileWriter::new(canonical_path).with_context(|| {
        format!(
            "failed to prepare device alias store at {}",
            canonical_path.display()
        )
    })?;
    let entry = PathMigrationEntry::new(
        STORE_SUBJECT,
        legacy_path.to_path_buf(),
        canonical_path.to_path_buf(),
    );
    let migrated = migrate(&DeviceAliasCodec, &entry, &writer)?;
    Ok((
        migrated.document.unwrap_or_else(empty_alias_file),
        migrated.outcome,
    ))
}

/// Persist the overlay with atomic-replace semantics.
pub fn save(path: &Path, file: &DeviceAliasFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create device alias directory {}",
                parent.display()
            )
        })?;
    }

    let payload =
        serde_json::to_string_pretty(file).context("failed to serialize device aliases")?;
    write_atomic(path, payload.as_bytes()).context("failed to persist device aliases")?;

    Ok(())
}

/// Seed the registry's portable identity state from the overlay.
///
/// Runs once at startup, before the first scan, so a claimed device's
/// first attach of the session already resolves to its pinned identity.
pub async fn seed_registry(path: &Path, registry: &DeviceRegistry) {
    let file = match load(path) {
        Ok(file) => file,
        Err(error) => {
            warn!(
                path = %path.display(),
                %error,
                "Failed to load device aliases; starting with an empty overlay"
            );
            return;
        }
    };

    seed_registry_document(path, file, registry).await;
}

/// Seed the registry from an already-authoritative overlay document.
pub async fn seed_registry_document(path: &Path, file: DeviceAliasFile, registry: &DeviceRegistry) {
    let pins: HashMap<PortableDeviceKey, DeviceFingerprint> = file
        .aliases
        .iter()
        .map(|(key, record)| {
            (
                key.clone(),
                DeviceFingerprint::from_persisted(record.fingerprint.clone()),
            )
        })
        .collect();
    let quarantined: HashSet<PortableDeviceKey> = file.quarantined_keys.into_iter().collect();

    let pin_count = pins.len();
    let quarantined_count = quarantined.len();
    registry.seed_portable_identity(pins, quarantined).await;
    info!(
        path = %path.display(),
        pins = pin_count,
        quarantined = quarantined_count,
        "Device alias overlay seeded into the registry"
    );
}

/// Fold the registry's live portable identity state into the overlay
/// file, after a discovery sweep.
///
/// Returns whether anything was written. Collision observations are
/// drained from the registry before the write; if the write then fails,
/// the observation detail survives only in the log, while the quarantine
/// itself is re-persisted by the next successful sync because the
/// registry still holds the key.
pub async fn sync_from_registry(path: &Path, registry: &DeviceRegistry) -> anyhow::Result<bool> {
    let pins = registry.portable_key_pins().await;
    let quarantined = registry.quarantined_portable_keys().await;
    let collisions = registry.drain_portable_key_collisions().await;
    let mut claims_by_key: HashMap<PortableDeviceKey, (PortableIdentityClaim, Option<String>)> =
        HashMap::new();
    for (device_id, claim) in registry.claims_snapshot().await {
        let layout_device_id = if let Some(tracked) = registry.get(&device_id).await {
            let fingerprint = registry.fingerprint_for_id(&device_id).await;
            Some(DeviceLifecycleManager::canonical_layout_device_id(
                &tracked.info,
                fingerprint.as_ref(),
            ))
        } else {
            None
        };
        claims_by_key.insert(claim.key().clone(), (claim, layout_device_id));
    }

    let mut file = load(path)?;
    let now = unix_now_s();
    let mut changed = false;

    for (key, pinned_fingerprint) in pins {
        let live = claims_by_key.get(&key);
        if let Some(record) = file.aliases.get_mut(&key) {
            if record.fingerprint != pinned_fingerprint.as_str() {
                record.fingerprint = pinned_fingerprint.into_string();
                changed = true;
            }
            if let Some((claim, layout_device_id)) = live {
                if record.raw != claim.raw() {
                    claim.raw().clone_into(&mut record.raw);
                    changed = true;
                }
                if layout_device_id.is_some() && record.layout_device_id != *layout_device_id {
                    record.layout_device_id.clone_from(layout_device_id);
                    changed = true;
                }
                if record.last_seen_epoch_s != now {
                    record.last_seen_epoch_s = now;
                    changed = true;
                }
            }
        } else {
            // A pin with no live claim and no record cannot say what the
            // identity was; skip rather than invent one.
            let Some((claim, layout_device_id)) = live else {
                continue;
            };
            file.aliases.insert(
                key,
                DeviceAliasRecord {
                    source: claim.source(),
                    raw: claim.raw().to_owned(),
                    fingerprint: pinned_fingerprint.into_string(),
                    layout_device_id: layout_device_id.clone(),
                    first_seen_epoch_s: now,
                    last_seen_epoch_s: now,
                },
            );
            changed = true;
        }
    }

    for key in quarantined {
        changed |= file.quarantined_keys.insert(key);
    }

    for collision in collisions {
        changed = true;
        file.collisions.push(persisted_collision(collision, now));
    }

    if changed {
        save(path, &file)?;
    }
    Ok(changed)
}

fn persisted_collision(collision: PortableKeyCollision, now: u64) -> PersistedKeyCollision {
    PersistedKeyCollision {
        key: collision.key,
        observed_epoch_s: now,
        existing_fingerprint: collision.existing_fingerprint.into_string(),
        existing_claim: collision.existing_claim,
        incoming_fingerprint: collision.incoming_fingerprint.into_string(),
        incoming_claim: collision.incoming_claim,
    }
}

fn unix_now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn empty_alias_file() -> DeviceAliasFile {
    DeviceAliasFile {
        schema_version: SCHEMA_VERSION,
        ..DeviceAliasFile::default()
    }
}

struct DeviceAliasCodec;

impl DeviceAliasCodec {
    fn read(path: &Path) -> anyhow::Result<VersionedDocument<DeviceAliasFile>> {
        let payload = fs::read_to_string(path)
            .with_context(|| format!("failed to read device aliases at {}", path.display()))?;
        let probe: AliasSchemaProbe = serde_json::from_str(&payload)
            .with_context(|| format!("failed to probe device aliases at {}", path.display()))?;
        if probe.schema_version != SCHEMA_VERSION {
            let relation = if probe.schema_version > SCHEMA_VERSION {
                "newer"
            } else {
                "older"
            };
            anyhow::bail!(
                "device-aliases.json is schema v{}, {relation} than supported v{}; refusing to read or rewrite it",
                probe.schema_version,
                SCHEMA_VERSION
            );
        }
        let file: DeviceAliasFile = serde_json::from_str(&payload)
            .with_context(|| format!("failed to parse device aliases at {}", path.display()))?;
        Ok(VersionedDocument::new(file.schema_version, file))
    }
}

impl MigratedStore for DeviceAliasCodec {
    type Document = DeviceAliasFile;
    type Error = anyhow::Error;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        Self::read(path)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        Self::read(path).map(Some)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_json_pretty(document).context("failed to serialize device aliases")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
    use hypercolor_types::device::DeviceState;
    use hypercolor_types::portable::NetworkAttachment;
    use tempfile::TempDir;

    use super::*;

    fn overlay_path(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join(DEVICE_ALIASES_FILE)
    }

    fn discovered(name: &str, fingerprint: &str, mac: &str, peer_octet: u8) -> DiscoveredDevice {
        DiscoveredDevice {
            fingerprint: DeviceFingerprint::from_persisted(fingerprint.to_owned()),
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info: mock_info(name),
            metadata: HashMap::new(),
            claim: PortableIdentityClaim::mac_address(
                mac,
                NetworkAttachment::Peer(IpAddr::V4(Ipv4Addr::new(192, 168, 1, peer_octet))),
            ),
        }
    }

    fn mock_info(name: &str) -> hypercolor_types::device::DeviceInfo {
        use hypercolor_types::device::{
            ConnectionType, DeviceCapabilities, DeviceFamily, DeviceId, DeviceInfo, DeviceOrigin,
        };

        DeviceInfo {
            id: DeviceId::new(),
            name: name.to_owned(),
            vendor: "MockVendor".to_owned(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("test", "test", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[tokio::test]
    async fn sync_records_claimed_devices_and_round_trips() {
        let dir = TempDir::new().expect("tempdir");
        let path = overlay_path(&dir);
        let registry = DeviceRegistry::new();

        registry
            .add_discovered(discovered(
                "Desk Strip",
                "net:wled:aaa",
                "2C:F4:32:11:22:33",
                40,
            ))
            .await;

        let changed = sync_from_registry(&path, &registry)
            .await
            .expect("sync succeeds");
        assert!(changed);

        let file = load(&path).expect("overlay loads");
        assert_eq!(file.aliases.len(), 1);
        let record = file.aliases.values().next().expect("one record");
        assert_eq!(record.fingerprint, "net:wled:aaa");
        assert_eq!(record.raw, "2C:F4:32:11:22:33");
        assert!(file.quarantined_keys.is_empty());
    }

    #[tokio::test]
    async fn seeded_overlay_restores_pins_across_registry_generations() {
        let dir = TempDir::new().expect("tempdir");
        let path = overlay_path(&dir);

        let first_session = DeviceRegistry::new();
        first_session
            .add_discovered(discovered(
                "Desk Strip",
                "net:wled:aaa",
                "2C:F4:32:11:22:33",
                40,
            ))
            .await;
        sync_from_registry(&path, &first_session)
            .await
            .expect("sync succeeds");

        // A fresh registry, as after a restart: the device re-attaches
        // under a new fingerprint and must resolve to the recorded one.
        let second_session = DeviceRegistry::new();
        seed_registry(&path, &second_session).await;
        let id = second_session
            .add_discovered(discovered(
                "Desk Strip",
                "net:wled:bbb",
                "2C:F4:32:11:22:33",
                41,
            ))
            .await;

        assert_eq!(
            second_session.fingerprint_for_id(&id).await,
            Some(DeviceFingerprint::from_persisted("net:wled:aaa".to_owned()))
        );
    }

    #[tokio::test]
    async fn collisions_persist_with_evidence_and_quarantine_survives_seeding() {
        let dir = TempDir::new().expect("tempdir");
        let path = overlay_path(&dir);
        let registry = DeviceRegistry::new();

        let first = registry
            .add_discovered(discovered(
                "Unit A",
                "net:wled:unit-a",
                "2C:F4:32:11:22:33",
                40,
            ))
            .await;
        registry.set_state(&first, DeviceState::Connected).await;
        registry
            .add_discovered(discovered(
                "Unit B",
                "net:wled:unit-b",
                "2C:F4:32:11:22:33",
                41,
            ))
            .await;

        sync_from_registry(&path, &registry)
            .await
            .expect("sync succeeds");

        let file = load(&path).expect("overlay loads");
        assert_eq!(file.collisions.len(), 1);
        assert_eq!(file.quarantined_keys.len(), 1);
        assert_eq!(file.collisions[0].existing_fingerprint, "net:wled:unit-a");
        assert_eq!(file.collisions[0].incoming_fingerprint, "net:wled:unit-b");

        // The quarantine reaches the next session through seeding: a
        // fresh observation of the key stays on its raw fingerprint.
        let next_session = DeviceRegistry::new();
        seed_registry(&path, &next_session).await;
        let id = next_session
            .add_discovered(discovered(
                "Unit C",
                "net:wled:unit-c",
                "2C:F4:32:11:22:33",
                42,
            ))
            .await;
        assert_eq!(
            next_session.fingerprint_for_id(&id).await,
            Some(DeviceFingerprint::from_persisted(
                "net:wled:unit-c".to_owned()
            ))
        );
    }

    #[test]
    fn missing_file_loads_as_empty_overlay() {
        let dir = TempDir::new().expect("tempdir");
        let file = load(&overlay_path(&dir)).expect("missing file is empty");
        assert!(file.aliases.is_empty());
        assert!(file.quarantined_keys.is_empty());
        assert!(file.collisions.is_empty());
        assert_eq!(file.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn overlay_moves_to_state_with_a_durable_backup() {
        let dir = TempDir::new().expect("tempdir");
        let legacy = dir.path().join("data/device-aliases.json");
        let canonical = dir.path().join("state/device-aliases.json");
        save(&legacy, &empty_alias_file()).expect("seed legacy overlay");

        let (file, outcome) =
            load_migrated(&legacy, &canonical).expect("overlay migration succeeds");
        let MigrationOutcome::Imported {
            backup: Some(backup),
        } = outcome
        else {
            panic!("expected an imported backup, got {outcome:?}");
        };

        assert_eq!(file.schema_version, SCHEMA_VERSION);
        assert!(canonical.exists());
        assert!(!legacy.exists());
        assert!(backup.exists());

        let (_, second) = load_migrated(&legacy, &canonical).expect("restart is idempotent");
        assert_eq!(second, MigrationOutcome::AlreadyMigrated);
    }

    #[test]
    fn newer_overlay_schema_is_refused_without_rewrite() {
        let dir = TempDir::new().expect("tempdir");
        let path = overlay_path(&dir);
        let payload = br#"{"schema_version":3,"aliases":{},"quarantined_keys":[],"collisions":[],"future":true}"#;
        fs::write(&path, payload).expect("write future overlay");

        let error = load(&path).expect_err("future schema is refused");

        assert!(error.to_string().contains("schema v3"));
        assert_eq!(fs::read(&path).expect("future overlay survives"), payload);
    }

    #[test]
    fn older_vendor_specific_overlay_is_refused_without_rewrite() {
        let dir = TempDir::new().expect("tempdir");
        let path = overlay_path(&dir);
        let payload = br#"{"schema_version":1,"aliases":{},"quarantined_keys":[],"collisions":[]}"#;
        fs::write(&path, payload).expect("write older overlay");

        let error = load(&path).expect_err("older schema is refused");

        assert!(error.to_string().contains("schema v1"));
        assert!(error.to_string().contains("older than supported v2"));
        assert_eq!(fs::read(&path).expect("older overlay survives"), payload);
    }

    #[test]
    fn invalid_legacy_overlay_never_replaces_state() {
        let dir = TempDir::new().expect("tempdir");
        let legacy = dir.path().join("data/device-aliases.json");
        let canonical = dir.path().join("state/device-aliases.json");
        fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
        fs::write(&legacy, b"not json").expect("write invalid overlay");

        let error = load_migrated(&legacy, &canonical).expect_err("invalid legacy is refused");

        assert!(error.to_string().contains("failed to probe"));
        assert_eq!(fs::read(&legacy).expect("legacy survives"), b"not json");
        assert!(!canonical.exists());
    }
}
