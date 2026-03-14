//! Persisted lighting profiles: named snapshots of runtime state.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use hypercolor_types::effect::ControlValue;

/// Serializable lighting profile snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
    pub effect_id: Option<String>,
    pub effect_name: Option<String>,
    pub active_preset_id: Option<String>,
    pub controls: HashMap<String, ControlValue>,
    pub layout_id: Option<String>,
}

impl Profile {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_owned();
        self.description = self
            .description
            .map(|description| description.trim().to_owned())
            .filter(|description| !description.is_empty());
        self.brightness = self.brightness.map(|brightness| brightness.min(100));
        self
    }
}

/// JSON-backed profile store.
#[derive(Debug, Clone, Default)]
pub struct ProfileStore {
    path: PathBuf,
    profiles: HashMap<String, Profile>,
}

impl ProfileStore {
    /// Create an empty store rooted at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            profiles: HashMap::new(),
        }
    }

    /// Load an existing store or create an empty one when absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read profiles at {}", path.display()))?;
        let profiles = serde_json::from_str::<HashMap<String, Profile>>(&raw)
            .with_context(|| format!("failed to parse profiles at {}", path.display()))?;

        let mut store = Self {
            path: path.to_path_buf(),
            profiles,
        };
        store.normalize();
        Ok(store)
    }

    /// Save the current snapshot to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let payload =
            serde_json::to_string_pretty(&self.profiles).context("failed to serialize profiles")?;
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, payload)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to move {} into {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = &Profile> {
        self.profiles.values()
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Profile> {
        self.profiles.get(key)
    }

    #[must_use]
    pub fn resolve_key(&self, id_or_name: &str) -> Option<String> {
        if self.profiles.contains_key(id_or_name) {
            return Some(id_or_name.to_owned());
        }

        self.profiles
            .iter()
            .find(|(_, profile)| profile.name.eq_ignore_ascii_case(id_or_name))
            .map(|(id, _)| id.clone())
    }

    pub fn insert(&mut self, profile: Profile) {
        let profile = profile.normalized();
        self.profiles.insert(profile.id.clone(), profile);
    }

    pub fn remove(&mut self, key: &str) -> Option<Profile> {
        self.profiles.remove(key)
    }

    fn normalize(&mut self) {
        self.profiles.retain(|_, profile| {
            *profile = profile.clone().normalized();
            !profile.id.is_empty() && !profile.name.is_empty()
        });
    }
}
