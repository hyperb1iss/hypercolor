//! Per-display default face preferences (spec 69 §3.6).
//!
//! Stores the face a display should show whenever the active scene does not
//! target it, keyed by the device's fingerprint-stable [`DeviceId`]. The
//! daemon materializes each preference into a runtime-only default zone on
//! the scene manager; this store is only the persistence layer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::scene::DisplayFaceBlendMode;
use serde::{Deserialize, Serialize};

fn default_opacity() -> f32 {
    1.0
}

/// A display's stored default face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayPreference {
    pub effect_id: EffectId,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub blend_mode: DisplayFaceBlendMode,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

/// JSON-file-backed store of per-display default faces.
#[derive(Debug)]
pub struct DisplayPreferencesStore {
    preferences: HashMap<DeviceId, DisplayPreference>,
    path: PathBuf,
}

impl DisplayPreferencesStore {
    /// Create an empty store for the given file path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            preferences: HashMap::new(),
            path,
        }
    }

    /// Load the store, returning an empty one when the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read display preferences store at {}",
                path.display()
            )
        })?;
        let preferences: HashMap<DeviceId, DisplayPreference> = serde_json::from_str(&raw)
            .with_context(|| {
                format!(
                    "failed to parse display preferences store at {}",
                    path.display()
                )
            })?;

        Ok(Self {
            preferences,
            path: path.to_path_buf(),
        })
    }

    /// Persist the store to its file path.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or the file
    /// cannot be written.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create display preferences directory {}",
                    parent.display()
                )
            })?;
        }
        let raw = serde_json::to_string_pretty(&self.preferences)
            .context("failed to serialize display preferences")?;
        fs::write(&self.path, raw).with_context(|| {
            format!(
                "failed to write display preferences store at {}",
                self.path.display()
            )
        })
    }

    #[must_use]
    pub fn get(&self, device_id: DeviceId) -> Option<&DisplayPreference> {
        self.preferences.get(&device_id)
    }

    pub fn set(&mut self, device_id: DeviceId, preference: DisplayPreference) {
        self.preferences.insert(device_id, preference);
    }

    pub fn remove(&mut self, device_id: DeviceId) -> Option<DisplayPreference> {
        self.preferences.remove(&device_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (DeviceId, &DisplayPreference)> {
        self.preferences
            .iter()
            .map(|(device_id, preference)| (*device_id, preference))
    }
}
