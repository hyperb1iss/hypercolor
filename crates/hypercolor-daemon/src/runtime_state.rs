//! Persisted runtime session state for startup restoration.
//!
//! Stores the active scene snapshot so daemon startup can restore the previous user session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{SceneId, Zone};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    /// Dead payload: the driver inventory owns this data outright now.
    ///
    /// Nothing reads it. It cannot simply be deleted because the snapshot
    /// denies unknown fields, so dropping the field would refuse every
    /// existing file and silently cost a session restore; retiring it
    /// needs a versioned runtime-state envelope.
    pub driver_runtime_cache: BTreeMap<String, BTreeMap<String, Value>>,
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
        .map(|scene| scene.groups.clone())
        .unwrap_or_default();

    RuntimeSessionSnapshot {
        active_scene_id,
        default_scene_groups,
        active_layout_id: None,
        global_brightness: 1.0,
        manual_paused: false,
        driver_runtime_cache: BTreeMap::new(),
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
    let snapshot: RuntimeSessionSnapshot =
        serde_json::from_str(&raw).map_err(|source| RuntimeSessionError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
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
    use std::collections::BTreeMap;
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;

    use super::{RuntimeSessionSnapshot, load, save};
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
            driver_runtime_cache: BTreeMap::from([(
                "cache-driver".to_owned(),
                BTreeMap::from([(
                    "probe_ips".to_owned(),
                    serde_json::json!(["10.0.0.8", "10.0.0.9"]),
                )]),
            )]),
        };

        save(&path, &expected).expect("save snapshot");
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_scene_id, expected.active_scene_id);
        assert_eq!(loaded.default_scene_groups, expected.default_scene_groups);
        assert!((loaded.global_brightness - expected.global_brightness).abs() < f32::EPSILON);
        assert_eq!(loaded.manual_paused, expected.manual_paused);
        assert_eq!(loaded.driver_runtime_cache, expected.driver_runtime_cache);
    }

    #[test]
    fn load_returns_none_when_missing() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let loaded = load(&path).expect("load should succeed");
        assert!(loaded.is_none());
    }

    #[test]
    fn older_snapshot_defaults_manual_pause_to_false() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "active_scene_id": null,
                "default_scene_groups": [],
                "active_layout_id": null,
                "global_brightness": 1.0,
                "driver_runtime_cache": {},
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
            driver_runtime_cache: BTreeMap::new(),
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
