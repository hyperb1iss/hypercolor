//! Persisted runtime session state for startup restoration.
//!
//! Stores the currently active effect, control values, and selected preset so
//! daemon startup can restore the previous user session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use hypercolor_core::effect::EffectEngine;
use hypercolor_types::effect::ControlValue;

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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| RuntimeSessionError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let bytes = serde_json::to_vec_pretty(snapshot).map_err(RuntimeSessionError::Serialize)?;
    let tmp_path = path.with_extension("json.tmp");

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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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
        };

        save(&path, &expected).expect("save snapshot");
        let loaded = load(&path).expect("load snapshot");
        let loaded = loaded.expect("snapshot should exist");

        assert_eq!(loaded.active_effect_id, expected.active_effect_id);
        assert_eq!(loaded.active_preset_id, expected.active_preset_id);
        assert_eq!(loaded.control_values, expected.control_values);
    }

    #[test]
    fn load_returns_none_when_missing() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("runtime-state.json");
        let loaded = load(&path).expect("load should succeed");
        assert!(loaded.is_none());
    }
}
