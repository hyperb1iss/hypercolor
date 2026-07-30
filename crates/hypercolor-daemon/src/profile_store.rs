//! Persisted lighting profiles: named snapshots of runtime state.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::library::PresetId;

use crate::persistence::{AtomicFileWriter, serialize_json_pretty};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveProfileError {
    AmbiguousName(String),
}

/// Serializable lighting profile snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<ProfilePrimary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub displays: Vec<ProfileDisplay>,
    pub brightness: Option<u8>,
    pub layout_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePrimary {
    pub effect_id: EffectId,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset_id: Option<PresetId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDisplay {
    pub device_id: DeviceId,
    pub effect_id: EffectId,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub controls: HashMap<String, ControlValue>,
}

impl Profile {
    #[must_use]
    pub fn named(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn normalized(mut self) -> Self {
        let trimmed_name = self.name.trim().to_owned();
        self.name.clone_from(&trimmed_name);
        self.description = self
            .description
            .map(|description| description.trim().to_owned())
            .filter(|description| !description.is_empty());
        self.brightness = self.brightness.map(|brightness| brightness.min(100));
        let mut seen_displays = HashSet::new();
        self.displays
            .retain(|display| seen_displays.insert(display.device_id));
        self
    }
}

/// JSON-backed profile store.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    path: PathBuf,
    profiles: HashMap<String, Profile>,
    writer: AtomicFileWriter,
}

impl ProfileStore {
    /// Create an empty store rooted at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the persistence destination cannot be prepared.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let writer = AtomicFileWriter::new(&path)
            .with_context(|| format!("failed to prepare profiles at {}", path.display()))?;
        Ok(Self {
            path,
            profiles: HashMap::new(),
            writer,
        })
    }

    /// Load an existing store or create an empty one when absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Self::new(path.to_path_buf());
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read profiles at {}", path.display()))?;
        let profiles = serde_json::from_str::<HashMap<String, Profile>>(&raw)
            .with_context(|| format!("failed to parse profiles at {}", path.display()))?;

        let mut store = Self {
            path: path.to_path_buf(),
            profiles,
            writer: AtomicFileWriter::new(path)
                .with_context(|| format!("failed to prepare profiles at {}", path.display()))?,
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
            serialize_json_pretty(&self.profiles).context("failed to serialize profiles")?;
        self.writer
            .write(&payload)
            .context("failed to persist profiles")?;
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

    pub fn resolve_key(&self, id_or_name: &str) -> Result<Option<String>, ResolveProfileError> {
        if self.profiles.contains_key(id_or_name) {
            return Ok(Some(id_or_name.to_owned()));
        }

        let matches = self.matching_name_keys(id_or_name, None);
        match matches.as_slice() {
            [] => Ok(None),
            [key] => Ok(Some(key.clone())),
            _ => Err(ResolveProfileError::AmbiguousName(id_or_name.to_owned())),
        }
    }

    pub fn find_existing_name_key(
        &self,
        name: &str,
        excluding_id: Option<&str>,
    ) -> Result<Option<String>, ResolveProfileError> {
        let matches = self.matching_name_keys(name, excluding_id);
        match matches.as_slice() {
            [] => Ok(None),
            [key] => Ok(Some(key.clone())),
            _ => Err(ResolveProfileError::AmbiguousName(name.to_owned())),
        }
    }

    /// Serialize and admit an upsert before changing the live store.
    ///
    /// # Errors
    ///
    /// Returns an error when the candidate snapshot cannot be serialized.
    pub fn insert(&mut self, profile: Profile) -> anyhow::Result<()> {
        let profile = profile.normalized();
        let mut candidate = self.profiles.clone();
        candidate.insert(profile.id.clone(), profile);
        self.install_candidate(candidate)
    }

    /// Serialize and admit a removal before changing the live store.
    ///
    /// # Errors
    ///
    /// Returns an error when the candidate snapshot cannot be serialized.
    pub fn remove(&mut self, key: &str) -> anyhow::Result<Option<Profile>> {
        let mut candidate = self.profiles.clone();
        let removed = candidate.remove(key);
        if removed.is_some() {
            self.install_candidate(candidate)?;
        }
        Ok(removed)
    }

    fn install_candidate(&mut self, candidate: HashMap<String, Profile>) -> anyhow::Result<()> {
        let payload = serialize_json_pretty(&candidate).context("failed to serialize profiles")?;
        let pending = self.writer.reserve().admit(payload);
        self.profiles = candidate;
        if let Err(error) = pending.commit() {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist profiles; retry remains active"
            );
        }
        Ok(())
    }

    fn normalize(&mut self) {
        self.profiles.retain(|_, profile| {
            *profile = profile.clone().normalized();
            !profile.id.is_empty() && !profile.name.is_empty()
        });
    }

    fn matching_name_keys(&self, name: &str, excluding_id: Option<&str>) -> Vec<String> {
        self.profiles
            .iter()
            .filter(|(id, profile)| {
                profile.name.eq_ignore_ascii_case(name)
                    && excluding_id.is_none_or(|excluded| excluded != id.as_str())
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use uuid::Uuid;

    use super::ProfileStore;
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::library::PresetId;

    #[test]
    fn load_rejects_unknown_fields_from_pre_final_profile_shape() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("profiles.json");
        let effect_id = EffectId::from(Uuid::now_v7());
        let payload = serde_json::json!({
            "prof_evening": {
                "id": "prof_evening",
                "name": "Evening",
                "effect_id": effect_id,
            }
        });
        fs::write(
            &path,
            serde_json::to_string_pretty(&payload).expect("profile json should serialize"),
        )
        .expect("profile json should be written");

        let error = ProfileStore::load(&path).expect_err("old profile shapes should fail");
        let causes = error.chain().map(ToString::to_string).collect::<Vec<_>>();
        assert!(
            causes
                .iter()
                .any(|cause| cause.contains("unknown field `effect_id`")),
            "expected unknown-field parse failure, got error chain {causes:?}"
        );
    }

    #[test]
    fn load_normalizes_profile_shape() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("profiles.json");
        let effect_id = EffectId::from(Uuid::now_v7());
        let preset_id = PresetId(Uuid::now_v7());
        let device_id = DeviceId::new();
        let payload = serde_json::json!({
            "prof_evening": {
                "id": "prof_evening",
                "name": "  Evening  ",
                "description": "  Cozy lights  ",
                "primary": {
                    "effect_id": effect_id,
                    "controls": {
                        "speed": { "float": 12.5 }
                    },
                    "active_preset_id": preset_id
                },
                "displays": [{
                    "device_id": device_id,
                    "effect_id": effect_id,
                    "controls": {}
                }, {
                    "device_id": device_id,
                    "effect_id": effect_id,
                    "controls": {
                        "accent": { "float": 0.25 }
                    }
                }],
                "brightness": 140,
                "layout_id": "layout_evening"
            }
        });
        fs::write(
            &path,
            serde_json::to_string_pretty(&payload).expect("profile json should serialize"),
        )
        .expect("profile json should be written");

        let store = ProfileStore::load(&path).expect("profile store should load");
        let profile = store
            .get("prof_evening")
            .expect("normalized profile should exist");
        assert_eq!(profile.name, "Evening");
        assert_eq!(profile.description.as_deref(), Some("Cozy lights"));
        assert_eq!(profile.brightness, Some(100));
        assert_eq!(
            profile
                .primary
                .as_ref()
                .and_then(|primary| primary.active_preset_id),
            Some(preset_id)
        );
        assert_eq!(profile.displays.len(), 1);
    }
}
