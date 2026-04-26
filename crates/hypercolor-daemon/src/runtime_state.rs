//! Persisted runtime session state for startup restoration.
//!
//! Stores the active scene snapshot so daemon startup can restore the previous user session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use hypercolor_core::scene::SceneManager;
use hypercolor_driver_api::DriverHost;
use hypercolor_network::DriverRegistry;
use hypercolor_types::scene::{RenderGroup, SceneId};

/// Process-local counter to guarantee per-save temp file uniqueness.
static SNAPSHOT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active scene ID, including the synthesized default scene.
    pub active_scene_id: Option<String>,

    /// Full render groups for the synthesized default scene.
    pub default_scene_groups: Vec<RenderGroup>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// User-configured global output brightness.
    pub global_brightness: f32,

    /// Driver-scoped runtime cache payloads.
    pub driver_runtime_cache: BTreeMap<String, BTreeMap<String, Value>>,
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
    #[error("failed to create runtime snapshot directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write temporary runtime snapshot {path}: {source}")]
    WriteTemp {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to replace runtime snapshot {path}: {source}")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
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

/// Load one driver-scoped cached JSON payload from `path`.
pub fn load_driver_cached_json(
    path: &Path,
    driver_id: &str,
    key: &str,
) -> Result<Option<Value>, RuntimeSessionError> {
    Ok(load(path)?
        .and_then(|mut snapshot| snapshot.driver_runtime_cache.remove(driver_id))
        .and_then(|mut cache| cache.remove(key)))
}

/// Collect all driver-owned runtime cache payloads.
pub async fn collect_driver_runtime_cache(
    driver_registry: &DriverRegistry,
    host: &dyn DriverHost,
) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut cache = BTreeMap::new();

    for driver_id in driver_registry.ids() {
        let Some(driver) = driver_registry.get(&driver_id) else {
            continue;
        };
        let Some(provider) = driver.runtime_cache() else {
            continue;
        };

        match provider.snapshot(host).await {
            Ok(values) if !values.is_empty() => {
                cache.insert(driver_id, values);
            }
            Ok(_) => {}
            Err(error) => {
                warn!(driver_id, %error, "Failed to collect driver runtime cache");
            }
        }
    }

    cache
}

/// Persist a runtime snapshot to `path` using atomic replace semantics.
pub fn save(path: &Path, snapshot: &RuntimeSessionSnapshot) -> Result<(), RuntimeSessionError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| RuntimeSessionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let bytes = serde_json::to_vec_pretty(snapshot).map_err(RuntimeSessionError::Serialize)?;
    let tmp_path = unique_temp_path(path);

    std::fs::write(&tmp_path, bytes).map_err(|source| RuntimeSessionError::WriteTemp {
        path: tmp_path.clone(),
        source,
    })?;
    std::fs::rename(&tmp_path, path).map_err(|source| RuntimeSessionError::Replace {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn unique_temp_path(path: &Path) -> PathBuf {
    let counter = SNAPSHOT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let pid = std::process::id();

    let file_name = path.file_name().map_or_else(
        || "runtime-state.json".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    path.with_file_name(format!("{file_name}.tmp-{pid}-{nanos}-{counter}"))
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
            driver_runtime_cache: BTreeMap::from([(
                "wled".to_owned(),
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
    fn concurrent_saves_share_path_without_colliding_temp_files() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = Arc::new(tempdir.path().join("runtime-state.json"));
        let snapshot = Arc::new(RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_groups: Vec::new(),
            active_layout_id: None,
            global_brightness: 1.0,
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
