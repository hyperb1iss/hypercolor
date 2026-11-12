//! Persisted runtime session state for startup restoration.
//!
//! Stores the active scene snapshot so daemon startup can restore the previous user session.

mod default_scene;
pub use default_scene::RuntimeDefaultSceneSettings;

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
    AtomicWriteReservation, PersistenceError, WriteBehind, serialize_json_pretty,
};

const STORE_SUBJECT: &str = "runtime session state";

/// The runtime session schema this build writes, in the file's top-level
/// `schema_version`.
///
/// The file had no version before scene layers gained an output scope, and
/// a file without the field is schema 0. Schema 1 has the same shape with
/// the field added, which is the point of it: a build from before the
/// field refuses the file on the unknown key instead of reading a scoped
/// layer and dropping the scope it does not know.
pub(crate) const RUNTIME_SESSION_SCHEMA_VERSION: u32 = 1;
const SCHEMA_FIELD: &str = "schema_version";

/// The snapshot as the file holds it: its schema, then its fields.
#[derive(Serialize)]
struct VersionedSnapshot<'a> {
    schema_version: u32,
    #[serde(flatten)]
    snapshot: &'a RuntimeSessionSnapshot,
}

impl<'a> VersionedSnapshot<'a> {
    const fn current(snapshot: &'a RuntimeSessionSnapshot) -> Self {
        Self {
            schema_version: RUNTIME_SESSION_SCHEMA_VERSION,
            snapshot,
        }
    }
}

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active scene ID, including the synthesized default scene.
    pub active_scene_id: Option<String>,

    /// Full zones for the synthesized default scene.
    pub default_scene_zones: Vec<Zone>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_scene_settings: Option<RuntimeDefaultSceneSettings>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// Explicit user pause state. Transient OS sleep is never persisted.
    pub manual_paused: bool,

    /// Explicitly stopped output, independent from transient OS sleep.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_stopped: bool,
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
    #[error(
        "runtime snapshot at {path} declares schema {found}; this release reads schemas 0 \
         through {newest}"
    )]
    UnsupportedSchema {
        path: PathBuf,
        found: String,
        newest: u32,
    },
    #[error("failed to serialize runtime snapshot: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to persist runtime snapshot {path}: {source}")]
    Persist {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },
    /// The admitted write's task stopped with the runtime before it
    /// finished; the retry supervisor still holds the admitted payload.
    #[error("runtime snapshot write for {path} stopped before it finished: {detail}")]
    WriteTask { path: PathBuf, detail: String },
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
        default_scene_settings: manager.get(&SceneId::DEFAULT).map(Into::into),
        active_layout_id: None,
        manual_paused: false,
        output_stopped: false,
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
    let mut schema = 0;
    if let Some(object) = decoded.as_object_mut() {
        object.remove("global_brightness");
        if let Some(declared) = object.remove(SCHEMA_FIELD) {
            schema = declared
                .as_u64()
                .and_then(|schema| u32::try_from(schema).ok())
                .filter(|schema| *schema <= RUNTIME_SESSION_SCHEMA_VERSION)
                .ok_or_else(|| RuntimeSessionError::UnsupportedSchema {
                    path: path.to_path_buf(),
                    found: declared.to_string(),
                    newest: RUNTIME_SESSION_SCHEMA_VERSION,
                })?;
        }
    }
    // Schema 0 and schema 1 share every field but the version itself, so
    // one decode reads both and the rewrite below is the whole upgrade.
    let snapshot: RuntimeSessionSnapshot =
        serde_json::from_value(decoded).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let normalized = serde_json::to_value(VersionedSnapshot::current(&snapshot))
        .map_err(RuntimeSessionError::Serialize)?;
    Ok(RuntimeSessionDocument {
        snapshot,
        schema,
        needs_rewrite: normalized != original,
    })
}

#[derive(Debug, Clone)]
struct RuntimeSessionDocument {
    snapshot: RuntimeSessionSnapshot,
    /// The schema the file declared; 0 for a file with no version field.
    schema: u32,
    needs_rewrite: bool,
}

impl RuntimeSessionDocument {
    fn versioned(self) -> VersionedDocument<Self> {
        VersionedDocument::new(self.schema, self)
    }
}

struct RuntimeSessionCodec;

impl MigratedStore for RuntimeSessionCodec {
    type Document = RuntimeSessionDocument;
    type Error = RuntimeSessionError;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        read_document(path).map(RuntimeSessionDocument::versioned)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        read_document(path)
            .map(RuntimeSessionDocument::versioned)
            .map(Some)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_json_pretty(&VersionedSnapshot::current(&document.snapshot))
            .map_err(RuntimeSessionError::Serialize)
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
    admit_reserved(pending, snapshot)?.commit()
}

/// A serialized runtime snapshot admitted as the newest durable intent,
/// whose replacement can run on whichever thread the caller picks.
#[derive(Debug)]
pub(crate) struct AdmittedRuntimeSnapshotWrite {
    path: PathBuf,
    write: AdmittedAtomicWrite,
}

/// Serialize and admit a reserved runtime snapshot without writing it.
pub(crate) fn admit_reserved(
    pending: RuntimeSnapshotSave,
    snapshot: &RuntimeSessionSnapshot,
) -> Result<AdmittedRuntimeSnapshotWrite, RuntimeSessionError> {
    let path = pending.path.clone();
    let prepared = prepare_reserved(pending, snapshot)?;
    Ok(AdmittedRuntimeSnapshotWrite {
        path,
        write: prepared.admit().write,
    })
}

impl AdmittedRuntimeSnapshotWrite {
    /// The destination this write replaces.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Attempt the admitted replacement. This blocks on file I/O.
    pub(crate) fn commit(self) -> Result<AtomicWriteOutcome, RuntimeSessionError> {
        let Self { path, write } = self;
        write
            .commit()
            .map_err(|source| RuntimeSessionError::Persist { path, source })
    }
}

/// A serialized runtime snapshot admitted to be written behind its caller
/// (see [`crate::persistence::WriteBehind`]).
#[derive(Debug)]
pub(crate) struct RuntimeSnapshotWriteBehind {
    path: PathBuf,
    write: WriteBehind,
}

/// Serialize and admit a reserved runtime snapshot for a write the caller
/// does not wait on.
pub(crate) fn admit_reserved_write_behind(
    pending: RuntimeSnapshotSave,
    snapshot: &RuntimeSessionSnapshot,
) -> Result<RuntimeSnapshotWriteBehind, RuntimeSessionError> {
    let path = pending.path.clone();
    let prepared = prepare_reserved(pending, snapshot)?;
    Ok(RuntimeSnapshotWriteBehind {
        path,
        write: prepared.write.admit_write_behind(prepared.payload),
    })
}

impl RuntimeSnapshotWriteBehind {
    /// Attempt the replacement unless a newer admission stood it down.
    /// This blocks on file I/O.
    pub(crate) fn commit(self) -> Result<AtomicWriteOutcome, RuntimeSessionError> {
        let Self { path, write } = self;
        write
            .commit()
            .map_err(|source| RuntimeSessionError::Persist { path, source })
    }
}

pub(crate) fn prepare_reserved(
    pending: RuntimeSnapshotSave,
    snapshot: &RuntimeSessionSnapshot,
) -> Result<PreparedRuntimeSnapshotSave, RuntimeSessionError> {
    let payload = serialize_json_pretty(&VersionedSnapshot::current(snapshot))
        .map_err(RuntimeSessionError::Serialize)?;
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
    pub(crate) fn commit_and_wait_durable_or_shutdown(
        self,
        shutdown: &crate::persistence::PersistenceShutdown,
    ) -> crate::persistence::AtomicWriteWaitOutcome {
        self.write.commit_and_wait_durable_or_shutdown(shutdown)
    }

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
            output_stopped: false,
            default_scene_settings: None,
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

    /// `runtime-state.json` as the real v0.9.0 daemon wrote it, with a
    /// named scene active and a default scene that holds a primary zone and
    /// two display zones. It has no version field. Captured by running the
    /// v0.9.0 scene manager and `runtime_state::save`. Frozen: never edit
    /// it to make a test pass.
    const V0_9_0_SESSION: &str =
        include_str!("../tests/fixtures/storage_v0_9_0/runtime-state.json");

    /// The top-level shape the v0.9.0 reader accepted. It denied unknown
    /// fields, so the reader's verdict on a file is whether this decodes.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, default)]
    #[derive(Default)]
    #[allow(dead_code, reason = "the fields exist to be decoded, not read")]
    struct V090RuntimeSession {
        active_scene_id: Option<String>,
        default_scene_zones: Vec<serde_json::Value>,
        default_scene_settings: Option<serde_json::Value>,
        active_layout_id: Option<String>,
        manual_paused: bool,
        output_stopped: bool,
    }

    fn file_json(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).expect("snapshot should read"))
            .expect("snapshot should parse")
    }

    #[test]
    fn a_saved_snapshot_declares_schema_1_ahead_of_its_fields() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");

        save(&path, &RuntimeSessionSnapshot::default()).expect("save snapshot");

        let serialized = std::fs::read_to_string(&path).expect("saved snapshot should read");
        assert!(
            serialized.starts_with("{\n  \"schema_version\": 1,"),
            "{serialized}"
        );
        assert_eq!(
            load(&path).expect("load snapshot"),
            Some(RuntimeSessionSnapshot::default())
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("loaded snapshot should read"),
            serialized,
            "a current file is not rewritten on load"
        );
    }

    #[test]
    fn the_session_captured_from_v0_9_0_loads_as_schema_0_and_is_rewritten_as_schema_1() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(&path, V0_9_0_SESSION).expect("captured session should write");
        let captured: serde_json::Value =
            serde_json::from_str(V0_9_0_SESSION).expect("captured session is valid JSON");
        assert!(captured.get("schema_version").is_none());
        serde_json::from_value::<V090RuntimeSession>(captured.clone())
            .expect("the v0.9.0 reader accepts its own file");

        let loaded = load(&path)
            .expect("a file with no version is schema 0 and loads")
            .expect("snapshot should exist");

        assert_eq!(loaded.default_scene_zones.len(), 3);
        assert_eq!(loaded.active_layout_id.as_deref(), Some("default"));
        assert_eq!(
            loaded,
            serde_json::from_value(captured).expect("the captured session decodes as it is"),
            "the upgrade changes no session content"
        );
        let mut rewritten = file_json(&path);
        let version = rewritten
            .as_object_mut()
            .expect("snapshot is an object")
            .remove("schema_version");
        assert_eq!(version, Some(serde_json::json!(1)));
        assert_eq!(
            loaded,
            serde_json::from_value(rewritten).expect("the rewritten session decodes"),
            "only the version is added on disk"
        );
    }

    #[test]
    fn a_build_from_before_the_version_field_refuses_a_current_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let manager = SceneManager::with_default();
        let mut zone = manager
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default scene should have a primary zone");
        let mut layer = SceneLayer::from_effect(
            SceneLayerId::new(),
            EffectId::from(Uuid::now_v7()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            None,
        );
        layer.scope = Some(hypercolor_types::layer::LayerScope {
            outputs: vec!["usb:keyboard-main".to_owned()],
        });
        zone.layers = vec![layer];
        let snapshot = RuntimeSessionSnapshot {
            default_scene_zones: vec![zone],
            ..RuntimeSessionSnapshot::default()
        };
        save(&path, &snapshot).expect("save snapshot");

        let error = serde_json::from_value::<V090RuntimeSession>(file_json(&path))
            .err()
            .expect("the unknown version field must stop the older reader");
        assert!(
            error.to_string().contains("unknown field `schema_version`"),
            "{error}"
        );
        // Without the boundary the same reader takes the file, scope and all,
        // and its layer type silently drops the scope it does not know.
        let mut unversioned = file_json(&path);
        unversioned
            .as_object_mut()
            .expect("snapshot is an object")
            .remove("schema_version");
        serde_json::from_value::<V090RuntimeSession>(unversioned)
            .expect("the version field is the only thing the older reader refuses");

        assert_eq!(
            load(&path).expect("load snapshot"),
            Some(snapshot),
            "this build reads the scope back"
        );
    }

    #[test]
    fn a_snapshot_from_a_newer_release_is_refused_and_left_alone() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        for (declared, named) in [
            (serde_json::json!(2), "schema 2"),
            (serde_json::json!("1"), "schema \"1\""),
            (serde_json::json!(-1), "schema -1"),
            (serde_json::json!(1.5), "schema 1.5"),
        ] {
            let payload = serde_json::to_vec(&serde_json::json!({
                "schema_version": declared,
                "active_scene_id": null,
                "default_scene_zones": [],
                "a_field_from_the_future": true,
            }))
            .expect("snapshot should serialize");
            std::fs::write(&path, &payload).expect("snapshot should be written");

            let error = load(&path).expect_err("an unreadable schema must be refused");
            assert!(
                matches!(error, RuntimeSessionError::UnsupportedSchema { .. }),
                "{error:?}"
            );
            let message = error.to_string();
            assert!(message.contains(named), "{message}");
            assert!(message.contains("schemas 0 through 1"), "{message}");
            assert_eq!(
                std::fs::read(&path).expect("snapshot should read"),
                payload,
                "a refused snapshot is never rewritten"
            );
        }
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
            output_stopped: false,
            default_scene_settings: None,
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
