//! Persisted runtime session state for startup restoration.
//!
//! Stores the active scene snapshot so daemon startup can restore the previous user session.

use std::path::{Path, PathBuf};

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{SceneId, Zone};
use serde::{Deserialize, Serialize};

use crate::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, AtomicWriteReservation, PersistenceError,
    serialize_json_pretty,
};

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active scene ID, including the synthesized default scene.
    pub active_scene_id: Option<String>,

    /// Full zones for the synthesized default scene.
    pub default_scene_groups: Vec<Zone>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// User-configured global output brightness.
    pub global_brightness: f32,

    /// Explicit user pause state. Transient OS sleep is never persisted.
    pub manual_paused: bool,
}

/// A runtime snapshot write ordered at the owning mutation boundary.
#[derive(Debug)]
pub struct RuntimeSnapshotSave {
    path: PathBuf,
    write: AtomicWriteReservation,
}

/// Errors produced while loading/saving runtime snapshots.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSessionError {
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
    let default_scene_groups = manager
        .get(&SceneId::DEFAULT)
        .map(|scene| scene.zones.clone())
        .unwrap_or_default();

    RuntimeSessionSnapshot {
        active_scene_id,
        default_scene_groups,
        active_layout_id: None,
        global_brightness: 1.0,
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

    let raw = std::fs::read_to_string(path).map_err(|source| RuntimeSessionError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let original: serde_json::Value =
        serde_json::from_str(&raw).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let snapshot: RuntimeSessionSnapshot =
        serde_json::from_value(original.clone()).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let normalized = serde_json::to_value(&snapshot).map_err(RuntimeSessionError::Serialize)?;
    if normalized != original {
        save(path, &snapshot)?;
    }
    Ok(Some(snapshot))
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
    let bytes = serialize_json_pretty(snapshot).map_err(RuntimeSessionError::Serialize)?;
    let outcome =
        pending
            .write
            .admit(bytes)
            .commit()
            .map_err(|source| RuntimeSessionError::Persist {
                path: pending.path,
                source,
            })?;
    Ok(outcome)
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
            default_scene_groups: Vec::new(),
            active_layout_id: Some("layout_abc123".to_owned()),
            global_brightness: 0.42,
            manual_paused: true,
        };

        save(&path, &expected).expect("save snapshot");
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_scene_id, expected.active_scene_id);
        assert_eq!(loaded.default_scene_groups, expected.default_scene_groups);
        assert!((loaded.global_brightness - expected.global_brightness).abs() < f32::EPSILON);
        assert_eq!(loaded.manual_paused, expected.manual_paused);
    }

    #[test]
    fn runtime_snapshot_persists_a_fresh_legacy_layer_id() {
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
            default_scene_groups: vec![zone],
            ..RuntimeSessionSnapshot::default()
        };
        let payload = serde_json::to_value(snapshot).expect("snapshot should serialize");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&payload).expect("legacy snapshot should serialize"),
        )
        .expect("legacy snapshot should write");

        let loaded = load(&path)
            .expect("legacy snapshot should migrate")
            .expect("snapshot should exist");
        let migrated_id = loaded.default_scene_groups[0].layers[0].id;
        assert_ne!(migrated_id.as_uuid(), zone_id.0);

        let reloaded = load(&path)
            .expect("migrated snapshot should reload")
            .expect("snapshot should exist");
        assert_eq!(reloaded.default_scene_groups[0].layers[0].id, migrated_id);
    }

    #[test]
    fn a_snapshot_carrying_the_retired_driver_cache_is_refused_not_read() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_groups": [],
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
    /// it wrote last boot. Removing a field is the other direction and is
    /// a refusal, covered above.
    #[test]
    fn an_absent_field_defaults_rather_than_failing_the_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_groups": [],
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
            default_scene_groups: Vec::new(),
            active_layout_id: None,
            global_brightness: 1.0,
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
                "default_scene_groups": [],
                "active_effect_id": "0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3",
            }))
            .expect("snapshot json should serialize"),
        )
        .expect("snapshot json should write");

        let error = load(&path).expect_err("removed fields should fail to load");
        assert!(matches!(error, super::RuntimeSessionError::Parse { .. }));
    }
}
