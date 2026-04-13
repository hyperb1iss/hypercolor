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
use hypercolor_core::device::wled::WledKnownTarget;
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::scene::SceneManager;
use hypercolor_types::device::{DeviceColorFormat, DeviceFamily};
use hypercolor_types::effect::{ControlBinding, ControlValue};
use hypercolor_types::scene::{RenderGroup, SceneId};

/// Process-local counter to guarantee per-save temp file uniqueness.
static SNAPSHOT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Runtime session snapshot persisted to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSessionSnapshot {
    /// Active scene ID, including the synthesized default scene.
    pub active_scene_id: Option<String>,

    /// Full render groups for the synthesized default scene.
    pub default_scene_groups: Vec<RenderGroup>,

    /// Active effect ID (UUID string), if any.
    pub active_effect_id: Option<String>,

    /// Active preset ID, if one is currently applied.
    pub active_preset_id: Option<String>,

    /// Current active control values for the running effect.
    pub control_values: HashMap<String, ControlValue>,

    /// Live sensor bindings attached to active controls.
    pub control_bindings: HashMap<String, ControlBinding>,

    /// Active layout ID, if one was applied to the spatial engine.
    pub active_layout_id: Option<String>,

    /// User-configured global output brightness.
    pub global_brightness: f32,

    /// Last-known WLED IPs discovered in previous sessions.
    pub wled_probe_ips: Vec<IpAddr>,

    /// Last-known WLED device identity hints used when probe enrichment fails.
    pub wled_probe_targets: Vec<WledKnownTarget>,
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
        active_scene_id: None,
        default_scene_groups: Vec::new(),
        active_effect_id,
        active_preset_id: engine.active_preset_id().map(ToOwned::to_owned),
        control_values: engine.active_controls().clone(),
        control_bindings: engine
            .active_metadata()
            .map(|metadata| {
                metadata
                    .controls
                    .iter()
                    .filter_map(|control| {
                        control
                            .binding
                            .clone()
                            .map(|binding| (control.control_id().to_owned(), binding))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        active_layout_id: None, // Populated by the caller with spatial engine state.
        global_brightness: 1.0,
        wled_probe_ips: Vec::new(),
        wled_probe_targets: Vec::new(),
    }
}

#[must_use]
pub fn snapshot_from_scene_manager(manager: &SceneManager) -> RuntimeSessionSnapshot {
    let active_scene_id = manager.active_scene_id().map(ToString::to_string);
    let default_scene_groups = manager
        .get(&SceneId::DEFAULT)
        .map(|scene| scene.groups.clone())
        .unwrap_or_default();
    let primary_group = manager
        .active_scene()
        .and_then(|scene| scene.primary_group());

    RuntimeSessionSnapshot {
        active_scene_id,
        default_scene_groups,
        active_effect_id: primary_group
            .and_then(|group| group.effect_id)
            .map(|effect_id| effect_id.to_string()),
        active_preset_id: primary_group
            .and_then(|group| group.preset_id)
            .map(|preset| preset.to_string()),
        control_values: primary_group.map_or_else(HashMap::new, |group| group.controls.clone()),
        control_bindings: primary_group
            .map_or_else(HashMap::new, |group| group.control_bindings.clone()),
        active_layout_id: None,
        global_brightness: 1.0,
        wled_probe_ips: Vec::new(),
        wled_probe_targets: Vec::new(),
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
    let Some(snapshot) = load(path)? else {
        return Ok(Vec::new());
    };

    let mut probe_ips = snapshot.wled_probe_ips;
    probe_ips.extend(
        snapshot
            .wled_probe_targets
            .into_iter()
            .map(|target| target.ip),
    );
    probe_ips.sort_unstable();
    probe_ips.dedup();
    Ok(probe_ips)
}

/// Load the cached WLED identity hints from `path`.
pub fn load_wled_probe_targets(path: &Path) -> Result<Vec<WledKnownTarget>, RuntimeSessionError> {
    Ok(load(path)?.map_or_else(Vec::new, |snapshot| snapshot.wled_probe_targets))
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

/// Collect the last-known WLED identity hints from the registry.
pub async fn collect_wled_probe_targets(device_registry: &DeviceRegistry) -> Vec<WledKnownTarget> {
    let tracked_devices = device_registry.list().await;
    let mut targets = Vec::new();

    for tracked in tracked_devices {
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

        let rgbw = tracked
            .info
            .zones
            .first()
            .map(|zone| matches!(zone.color_format, DeviceColorFormat::Rgbw));
        let fingerprint = device_registry.fingerprint_for_id(&tracked.info.id).await;

        targets.push(WledKnownTarget {
            ip,
            hostname: metadata.get("hostname").cloned(),
            fingerprint,
            name: Some(tracked.info.name.clone()),
            led_count: Some(tracked.info.total_led_count()),
            firmware_version: tracked.info.firmware_version.clone(),
            max_fps: Some(tracked.info.capabilities.max_fps),
            rgbw,
        });
    }

    targets.sort_by_key(|target| target.ip);
    targets.dedup_by_key(|target| target.ip);
    targets
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
    use hypercolor_types::effect::{ControlBinding, ControlValue};
    use hypercolor_types::scene::SceneId;

    #[test]
    fn round_trip_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");

        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(0.72));
        let expected = RuntimeSessionSnapshot {
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_groups: Vec::new(),
            active_effect_id: Some("0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3".to_owned()),
            active_preset_id: Some("preset_42".to_owned()),
            control_values: controls,
            control_bindings: HashMap::from([(
                "speed".to_owned(),
                ControlBinding {
                    sensor: "cpu_temp".to_owned(),
                    sensor_min: 30.0,
                    sensor_max: 100.0,
                    target_min: 0.0,
                    target_max: 1.0,
                    deadband: 0.5,
                    smoothing: 0.2,
                },
            )]),
            active_layout_id: Some("layout_abc123".to_owned()),
            global_brightness: 0.42,
            wled_probe_ips: vec![
                "10.0.0.8".parse().expect("valid IP"),
                "10.0.0.9".parse().expect("valid IP"),
            ],
            wled_probe_targets: Vec::new(),
        };

        save(&path, &expected).expect("save snapshot");
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_effect_id, expected.active_effect_id);
        assert_eq!(loaded.active_preset_id, expected.active_preset_id);
        assert_eq!(loaded.active_scene_id, expected.active_scene_id);
        assert_eq!(loaded.default_scene_groups, expected.default_scene_groups);
        assert_eq!(loaded.control_values, expected.control_values);
        assert_eq!(loaded.control_bindings, expected.control_bindings);
        assert!((loaded.global_brightness - expected.global_brightness).abs() < f32::EPSILON);
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
            active_scene_id: Some(SceneId::DEFAULT.to_string()),
            default_scene_groups: Vec::new(),
            active_effect_id: Some("0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3".to_owned()),
            active_preset_id: Some("preset_42".to_owned()),
            control_values: HashMap::new(),
            control_bindings: HashMap::new(),
            active_layout_id: None,
            global_brightness: 1.0,
            wled_probe_ips: vec!["10.0.0.42".parse().expect("valid IP")],
            wled_probe_targets: Vec::new(),
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
