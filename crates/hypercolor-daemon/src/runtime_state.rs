//! Persisted runtime session state for startup restoration.
//!
//! Stores the active scene snapshot so daemon startup can restore the previous user session.

use std::path::{Path, PathBuf};

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{SceneId, Zone};
use serde::{Deserialize, Serialize};

use crate::domain::effect::{EffectIdMigrations, remap_zones};
use crate::path_migration::{
    MigratedStore, MigrationOutcome, PathMigrationEntry, PathMigrationError, VersionedDocument,
    migrate,
};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, AtomicWriteOutcome,
    AtomicWriteReservation, PersistenceError, serialize_json_pretty,
};

const STORE_SUBJECT: &str = "runtime session state";

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active scene ID, including the synthesized default scene.
    pub active_scene_id: Option<String>,

    /// Full zones for the synthesized default scene.
    pub default_scene_zones: Vec<Zone>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// Explicit user pause state. Transient OS sleep is never persisted.
    pub manual_paused: bool,
}

impl RuntimeSessionSnapshot {
    /// Rewrite path-derived effect IDs in the persisted default scene.
    pub fn migrate_effect_ids(&mut self, migrations: &EffectIdMigrations) -> usize {
        remap_zones(&mut self.default_scene_zones, migrations)
    }
}

/// A runtime snapshot write ordered at the owning mutation boundary.
#[derive(Debug)]
pub struct RuntimeSnapshotSave {
    path: PathBuf,
    write: AtomicWriteReservation,
}

#[derive(Debug)]
pub(crate) struct PreparedRuntimeSnapshotSave {
    write: AtomicWriteReservation,
    payload: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct AdmittedRuntimeSnapshotSave {
    write: AdmittedAtomicWrite,
}

/// Errors produced while loading/saving runtime snapshots.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSessionError {
    #[error(transparent)]
    Migration(#[from] PathMigrationError),
    #[error("failed to read runtime snapshot at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse runtime snapshot at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize runtime snapshot: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to persist runtime snapshot {path}: {source}")]
    Persist {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },
}

#[must_use]
pub fn snapshot_from_scene_manager(manager: &SceneManager) -> RuntimeSessionSnapshot {
    let active_scene_id = manager.active_scene_id().map(ToString::to_string);
    let default_scene_zones = manager
        .get(&SceneId::DEFAULT)
        .map(|scene| scene.zones.clone())
        .unwrap_or_default();

    RuntimeSessionSnapshot {
        active_scene_id,
        default_scene_zones,
        active_layout_id: None,
        manual_paused: false,
    }
}

/// Load a runtime snapshot from `path`.
///
/// Returns `Ok(None)` if no snapshot exists yet.
pub fn load(path: &Path) -> Result<Option<RuntimeSessionSnapshot>, RuntimeSessionError> {
    if !path.exists() {
        return Ok(None);
    }

    let document = read_document(path)?;
    if document.needs_rewrite {
        save(path, &document.snapshot)?;
    }
    Ok(Some(document.snapshot))
}

/// Relocate a legacy data-tier snapshot and load the state-tier document.
///
/// # Errors
///
/// Returns an error when either snapshot cannot be read or decoded, the state
/// destination cannot be prepared, or a durable import cannot be retired.
pub fn load_migrated(
    legacy_path: &Path,
    canonical_path: &Path,
) -> Result<(Option<RuntimeSessionSnapshot>, MigrationOutcome), RuntimeSessionError> {
    let writer =
        AtomicFileWriter::new(canonical_path).map_err(|source| RuntimeSessionError::Persist {
            path: canonical_path.to_path_buf(),
            source,
        })?;
    let entry = PathMigrationEntry::new(
        STORE_SUBJECT,
        legacy_path.to_path_buf(),
        canonical_path.to_path_buf(),
    );
    let migrated = migrate(&RuntimeSessionCodec, &entry, &writer)?;
    let outcome = migrated.outcome;
    let document = migrated.document;
    if matches!(outcome, MigrationOutcome::AlreadyMigrated)
        && let Some(document) = document.as_ref()
        && document.needs_rewrite
    {
        save(canonical_path, &document.snapshot)?;
    }
    Ok((document.map(|document| document.snapshot), outcome))
}

fn read_document(path: &Path) -> Result<RuntimeSessionDocument, RuntimeSessionError> {
    let raw = std::fs::read_to_string(path).map_err(|source| RuntimeSessionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let original: serde_json::Value =
        serde_json::from_str(&raw).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let mut decoded = original.clone();
    if let Some(object) = decoded.as_object_mut() {
        object.remove("global_brightness");
    }
    let snapshot: RuntimeSessionSnapshot =
        serde_json::from_value(decoded).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let normalized = serde_json::to_value(&snapshot).map_err(RuntimeSessionError::Serialize)?;
    Ok(RuntimeSessionDocument {
        snapshot,
        needs_rewrite: normalized != original,
    })
}

#[derive(Debug, Clone)]
struct RuntimeSessionDocument {
    snapshot: RuntimeSessionSnapshot,
    needs_rewrite: bool,
}

struct RuntimeSessionCodec;

impl MigratedStore for RuntimeSessionCodec {
    type Document = RuntimeSessionDocument;
    type Error = RuntimeSessionError;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        read_document(path).map(VersionedDocument::unversioned)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        read_document(path)
            .map(VersionedDocument::unversioned)
            .map(Some)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_json_pretty(&document.snapshot).map_err(RuntimeSessionError::Serialize)
    }
}

/// Persist a runtime snapshot to `path` using atomic replace semantics.
pub fn save(path: &Path, snapshot: &RuntimeSessionSnapshot) -> Result<(), RuntimeSessionError> {
    let pending = reserve_save(path)?;
    save_reserved(pending, snapshot).map(|_| ())
}

/// Reserve a runtime snapshot generation before asynchronous assembly begins.
pub fn reserve_save(path: &Path) -> Result<RuntimeSnapshotSave, RuntimeSessionError> {
    let writer = AtomicFileWriter::new(path).map_err(|source| RuntimeSessionError::Persist {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(RuntimeSnapshotSave {
        path: path.to_path_buf(),
        write: writer.reserve(),
    })
}

/// Serialize and commit a previously reserved runtime snapshot generation.
pub fn save_reserved(
    pending: RuntimeSnapshotSave,
    snapshot: &RuntimeSessionSnapshot,
) -> Result<AtomicWriteOutcome, RuntimeSessionError> {
    let path = pending.path.clone();
    let prepared = prepare_reserved(pending, snapshot)?;
    let outcome = prepared
        .admit()
        .write
        .commit()
        .map_err(|source| RuntimeSessionError::Persist { path, source })?;
    Ok(outcome)
}

pub(crate) fn prepare_reserved(
    pending: RuntimeSnapshotSave,
    snapshot: &RuntimeSessionSnapshot,
) -> Result<PreparedRuntimeSnapshotSave, RuntimeSessionError> {
    let payload = serialize_json_pretty(snapshot).map_err(RuntimeSessionError::Serialize)?;
    Ok(PreparedRuntimeSnapshotSave {
        write: pending.write,
        payload,
    })
}

impl PreparedRuntimeSnapshotSave {
    pub(crate) fn admit(self) -> AdmittedRuntimeSnapshotSave {
        AdmittedRuntimeSnapshotSave {
            write: self.write.admit(self.payload),
        }
    }
}

impl AdmittedRuntimeSnapshotSave {
    pub(crate) fn commit_stage_aware(self) -> AtomicWriteCommitResult {
        self.write.commit_stage_aware()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;
    use uuid::Uuid;

    use super::{RuntimeSessionError, RuntimeSessionSnapshot, load, save};
    use hypercolor_core::scene::SceneManager;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{SceneLayer, SceneLayerId};
    use hypercolor_types::scene::SceneId;

    #[test]
    fn round_trip_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");

        let expected = RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_zones: Vec::new(),
            active_layout_id: Some("layout_abc123".to_owned()),
            manual_paused: true,
        };

        save(&path, &expected).expect("save snapshot");
        let serialized = std::fs::read_to_string(&path).expect("saved snapshot should read");
        assert!(serialized.contains("\"default_scene_zones\""));
        assert!(!serialized.contains("default_scene_groups"));
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_scene_id, expected.active_scene_id);
        assert_eq!(loaded.default_scene_zones, expected.default_scene_zones);
        assert_eq!(loaded.manual_paused, expected.manual_paused);
    }

    #[test]
    fn runtime_snapshot_refuses_the_retired_default_scene_groups_key() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_groups": [],
                "active_layout_id": null,
                "manual_paused": false,
            }))
            .expect("retired snapshot should serialize"),
        )
        .expect("retired snapshot should write");

        let error = load(&path).expect_err("retired runtime key must be refused");
        assert!(matches!(error, RuntimeSessionError::Parse { .. }));
    }

    #[test]
    fn runtime_snapshot_preserves_the_authored_layer_id() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let manager = SceneManager::with_default();
        let mut zone = manager
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default scene should have a primary zone");
        let zone_id = zone.id;
        zone.layers = vec![SceneLayer::from_effect(
            SceneLayerId::from_uuid(zone_id.0),
            EffectId::from(Uuid::now_v7()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            None,
        )];
        let snapshot = RuntimeSessionSnapshot {
            default_scene_zones: vec![zone],
            ..RuntimeSessionSnapshot::default()
        };
        let payload = serde_json::to_value(snapshot).expect("snapshot should serialize");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&payload).expect("legacy snapshot should serialize"),
        )
        .expect("legacy snapshot should write");

        let loaded = load(&path)
            .expect("snapshot should load")
            .expect("snapshot should exist");
        let loaded_id = loaded.default_scene_zones[0].layers[0].id;
        assert_eq!(loaded_id.as_uuid(), zone_id.0);

        let reloaded = load(&path)
            .expect("snapshot should reload")
            .expect("snapshot should exist");
        assert_eq!(reloaded.default_scene_zones[0].layers[0].id, loaded_id);
    }

    #[test]
    fn runtime_snapshot_effect_id_migration_is_durable() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let legacy_id = EffectId::from(Uuid::now_v7());
        let canonical_id = EffectId::from(Uuid::now_v7());
        let mut zone = SceneManager::with_default()
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default scene should have a primary zone");
        zone.layers = vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            legacy_id,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            None,
        )];
        let mut snapshot = RuntimeSessionSnapshot {
            default_scene_zones: vec![zone],
            ..RuntimeSessionSnapshot::default()
        };
        save(&path, &snapshot).expect("legacy snapshot should persist");

        assert_eq!(
            snapshot.migrate_effect_ids(&std::collections::HashMap::from([(
                legacy_id,
                canonical_id,
            )])),
            1
        );
        save(&path, &snapshot).expect("migrated snapshot should persist");

        let reopened = load(&path)
            .expect("snapshot should reload")
            .expect("snapshot should exist");
        assert_eq!(
            reopened.default_scene_zones[0]
                .effect_ids()
                .collect::<Vec<_>>(),
            vec![canonical_id]
        );
        assert!(
            !std::fs::read_to_string(path)
                .expect("snapshot should read")
                .contains(&legacy_id.to_string())
        );
    }

    #[test]
    fn a_snapshot_carrying_the_retired_driver_cache_is_refused_not_read() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_zones": [],
                "active_layout_id": null,
                "global_brightness": 1.0,
                "manual_paused": false,
                "driver_runtime_cache": {},
            }))
            .expect("snapshot should serialize"),
        )
        .expect("snapshot should be written");

        // The driver inventory owns this data now. A snapshot written
        // before the field was retired is refused rather than half-read;
        // the daemon logs it and starts fresh, which costs one session
        // restore and never a startup.
        let error = load(&path).expect_err("a retired field must be refused");
        assert!(
            matches!(error, RuntimeSessionError::Parse { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn load_returns_none_when_missing() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let loaded = load(&path).expect("load should succeed");
        assert!(loaded.is_none());
    }

    /// Fields added to the snapshot default when absent.
    ///
    /// This is forward evolution, not legacy-shape support: a new field
    /// lands with a default so the running daemon keeps reading the file
    /// it wrote last boot. Retired authority fields are explicitly discarded;
    /// every other removed field remains a refusal, covered above.
    #[test]
    fn an_absent_field_defaults_rather_than_failing_the_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_zones": [],
                "active_layout_id": null,
                "global_brightness": 1.0,
            }))
            .expect("snapshot should serialize"),
        )
        .expect("snapshot should be written");

        let loaded = load(&path)
            .expect("snapshot should load")
            .expect("snapshot should exist");

        assert!(!loaded.manual_paused);
    }

    #[test]
    fn concurrent_saves_share_path_without_colliding_temp_files() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = Arc::new(tempdir.path().join("runtime-state.json"));
        let snapshot = Arc::new(RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_zones: Vec::new(),
            active_layout_id: None,
            manual_paused: false,
        });

        let worker_count = 8;
        let barrier = Arc::new(Barrier::new(worker_count));
        let mut workers = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let path = Arc::clone(&path);
            let snapshot = Arc::clone(&snapshot);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..64 {
                    save(path.as_path(), &snapshot).expect("concurrent save should succeed");
                }
            }));
        }

        for worker in workers {
            worker.join().expect("worker thread should not panic");
        }

        let loaded = load(path.as_path()).expect("load should succeed");
        assert!(
            loaded.is_some(),
            "snapshot file should exist after concurrent saves"
        );
    }

    #[test]
    fn load_rejects_removed_effect_snapshot_fields() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "active_scene_id": SceneId::DEFAULT.to_string(),
                "default_scene_zones": [],
                "active_effect_id": "0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3",
            }))
            .expect("snapshot json should serialize"),
        )
        .expect("snapshot json should write");

        let error = load(&path).expect_err("removed fields should fail to load");
        assert!(matches!(error, super::RuntimeSessionError::Parse { .. }));
    }
}
