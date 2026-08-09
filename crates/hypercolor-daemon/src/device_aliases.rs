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
use hypercolor_core::device::{DeviceRegistry, PortableKeyCollision};
use hypercolor_types::device::DeviceFingerprint;
use hypercolor_types::portable::{
    PortableDeviceKey, PortableIdentityClaim, PortableIdentitySource,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::persistence::write_atomic;

/// File name of the overlay inside the daemon data directory.
pub const DEVICE_ALIASES_FILE: &str = "device-aliases.json";

const SCHEMA_VERSION: u32 = 1;

/// Persisted portable-key overlay.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceAliasFile {
    /// Envelope version for forward-compatible reads.
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
        return Ok(DeviceAliasFile {
            schema_version: SCHEMA_VERSION,
            ..DeviceAliasFile::default()
        });
    }

    let payload = fs::read_to_string(path)
        .with_context(|| format!("failed to read device aliases at {}", path.display()))?;
    serde_json::from_str(&payload)
        .with_context(|| format!("failed to parse device aliases at {}", path.display()))
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

    let pins: HashMap<PortableDeviceKey, DeviceFingerprint> = file
        .aliases
        .iter()
        .map(|(key, record)| (key.clone(), DeviceFingerprint(record.fingerprint.clone())))
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
    let claims_by_key: HashMap<PortableDeviceKey, PortableIdentityClaim> = registry
        .claims_snapshot()
        .await
        .into_values()
        .map(|claim| (claim.key().clone(), claim))
        .collect();

    let mut file = load(path)?;
    let now = unix_now_s();
    let mut changed = false;

    for (key, pinned_fingerprint) in pins {
        let live_claim = claims_by_key.get(&key);
        if let Some(record) = file.aliases.get_mut(&key) {
            if record.fingerprint != pinned_fingerprint.0 {
                record.fingerprint = pinned_fingerprint.0;
                changed = true;
            }
            if let Some(claim) = live_claim {
                if record.raw != claim.raw() {
                    claim.raw().clone_into(&mut record.raw);
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
            let Some(claim) = live_claim else {
                continue;
            };
            file.aliases.insert(
                key,
                DeviceAliasRecord {
                    source: claim.source(),
                    raw: claim.raw().to_owned(),
                    fingerprint: pinned_fingerprint.0,
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
        existing_fingerprint: collision.existing_fingerprint.0,
        existing_claim: collision.existing_claim,
        incoming_fingerprint: collision.incoming_fingerprint.0,
        incoming_claim: collision.incoming_claim,
    }
}

fn unix_now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use hypercolor_core::device::{DiscoveredDevice, DiscoveryConnectBehavior};
    use hypercolor_types::device::DeviceState;
    use hypercolor_types::portable::NetworkAttachment;
    use tempfile::TempDir;

    use super::*;

    fn overlay_path(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join(DEVICE_ALIASES_FILE)
    }

    fn discovered(name: &str, fingerprint: &str, mac: &str, peer_octet: u8) -> DiscoveredDevice {
        DiscoveredDevice {
            fingerprint: DeviceFingerprint(fingerprint.to_owned()),
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
            zones: Vec::new(),
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
            Some(DeviceFingerprint("net:wled:aaa".to_owned()))
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
            Some(DeviceFingerprint("net:wled:unit-c".to_owned()))
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
}
