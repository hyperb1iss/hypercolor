//! Persisted runtime session state for startup restoration.
//!
//! Stores the currently active effect, control values, and selected preset so
//! daemon startup can restore the previous user session.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use hypercolor_core::device::DeviceRegistry;
use hypercolor_core::effect::EffectEngine;
use hypercolor_types::device::DeviceFamily;
use hypercolor_types::effect::ControlValue;

/// Process-local counter to guarantee per-save temp file uniqueness.
static SNAPSHOT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active effect ID (UUID string), if any.
    pub active_effect_id: Option<String>,

    /// Active preset ID, if one is currently applied.
    pub active_preset_id: Option<String>,

    /// Current active control values for the running effect.
    pub control_values: HashMap<String, ControlValue>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// User-configured global output brightness.
    pub global_brightness: f32,

    /// Last-known WLED IPs discovered in previous sessions.
    pub wled_probe_ips: Vec<IpAddr>,
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

/// Build a snapshot from the current effect engine state.
#[must_use]
pub fn snapshot_from_engine(engine: &EffectEngine) -> RuntimeSessionSnapshot {
    let active_effect_id = engine.active_metadata().map(|meta| meta.id.to_string());
    if active_effect_id.is_none() {
        return RuntimeSessionSnapshot::default();
    }

    RuntimeSessionSnapshot {
        active_effect_id,
        active_preset_id: engine.active_preset_id().map(ToOwned::to_owned),
        control_values: engine.active_controls().clone(),
        active_layout_id: None, // Populated by the caller with spatial engine state.
        global_brightness: 1.0,
        wled_probe_ips: Vec::new(),
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

/// Load the cached WLED probe IPs from `path`.
pub fn load_wled_probe_ips(path: &Path) -> Result<Vec<IpAddr>, RuntimeSessionError> {
    Ok(load(path)?.map_or_else(Vec::new, |snapshot| snapshot.wled_probe_ips))
}

/// Collect the last-known WLED probe IPs from registry metadata.
pub async fn collect_wled_probe_ips(device_registry: &DeviceRegistry) -> Vec<IpAddr> {
    let mut probe_ips = HashSet::new();

    for tracked in device_registry.list().await {
        if tracked.info.family != DeviceFamily::Wled {
            continue;
        }

        let Some(metadata) = device_registry.metadata_for_id(&tracked.info.id).await else {
            continue;
        };
        let Some(ip_raw) = metadata.get("ip") else {
            continue;
        };
        let Ok(ip) = ip_raw.parse::<IpAddr>() else {
            continue;
        };

        probe_ips.insert(ip);
    }

    let mut resolved: Vec<IpAddr> = probe_ips.into_iter().collect();
    resolved.sort_unstable();
    resolved
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
    use std::collections::HashMap;
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;

    use super::{RuntimeSessionSnapshot, load, save};
    use hypercolor_types::effect::ControlValue;

    #[test]
    fn round_trip_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");

        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(0.72));
        let expected = RuntimeSessionSnapshot {
            active_effect_id: Some("0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3".to_owned()),
            active_preset_id: Some("preset_42".to_owned()),
            control_values: controls,
            active_layout_id: Some("layout_abc123".to_owned()),
            global_brightness: 0.42,
            wled_probe_ips: vec![
                "10.0.0.8".parse().expect("valid IP"),
                "10.0.0.9".parse().expect("valid IP"),
            ],
        };

        save(&path, &expected).expect("save snapshot");
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_effect_id, expected.active_effect_id);
        assert_eq!(loaded.active_preset_id, expected.active_preset_id);
        assert_eq!(loaded.control_values, expected.control_values);
        assert_eq!(loaded.global_brightness, expected.global_brightness);
        assert_eq!(loaded.wled_probe_ips, expected.wled_probe_ips);
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
            active_effect_id: Some("0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3".to_owned()),
            active_preset_id: Some("preset_42".to_owned()),
            control_values: HashMap::new(),
            active_layout_id: None,
            global_brightness: 1.0,
            wled_probe_ips: vec!["10.0.0.42".parse().expect("valid IP")],
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
}
