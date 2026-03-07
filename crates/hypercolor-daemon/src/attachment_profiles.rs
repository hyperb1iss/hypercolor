//! Persisted device attachment profile store.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use hypercolor_types::attachment::DeviceAttachmentProfile;
use hypercolor_types::device::DeviceInfo;

/// Persistent attachment profile store keyed by physical device ID.
#[derive(Debug, Clone)]
pub struct AttachmentProfileStore {
    profiles: HashMap<String, DeviceAttachmentProfile>,
    path: PathBuf,
}

impl AttachmentProfileStore {
    /// Create an empty attachment profile store for the given file path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            profiles: HashMap::new(),
            path,
        }
    }

    /// Load persisted attachment profiles from disk.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path).with_context(|| {
            format!("failed to read attachment profile store at {}", path.display())
        })?;
        let profiles: HashMap<String, DeviceAttachmentProfile> = serde_json::from_str(&raw)
            .with_context(|| {
                format!(
                    "failed to parse attachment profile store at {}",
                    path.display()
                )
            })?;

        Ok(Self {
            profiles,
            path: path.to_path_buf(),
        })
    }

    /// Persist attachment profiles with atomic replace semantics.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create attachment profile store directory {}",
                    parent.display()
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(
            &self
                .profiles
                .iter()
                .map(|(device_id, profile)| (device_id.clone(), profile.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
        .context("failed to serialize attachment profile store")?;

        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, payload).with_context(|| {
            format!(
                "failed to write temporary attachment profile store {}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to move temporary attachment profile store {} into {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;

        Ok(())
    }

    /// Get a stored profile by physical device ID.
    #[must_use]
    pub fn get(&self, device_id: &str) -> Option<&DeviceAttachmentProfile> {
        self.profiles.get(device_id)
    }

    /// Get the stored profile for a device, or derive a default one from current zones.
    #[must_use]
    pub fn get_or_default(&self, device: &DeviceInfo) -> DeviceAttachmentProfile {
        let device_id = device.id.to_string();
        let slots = device.default_attachment_profile().slots;

        if let Some(profile) = self.profiles.get(&device_id) {
            let mut profile = profile.clone();
            profile.slots = slots;
            return profile;
        }

        device.default_attachment_profile()
    }

    /// Insert or replace a stored profile.
    pub fn update(&mut self, device_id: &str, profile: DeviceAttachmentProfile) {
        self.profiles.insert(device_id.to_owned(), profile);
    }

    /// Remove a stored profile.
    pub fn remove(&mut self, device_id: &str) -> Option<DeviceAttachmentProfile> {
        self.profiles.remove(device_id)
    }

    /// Whether any stored profile binds the given template ID.
    #[must_use]
    pub fn uses_template(&self, template_id: &str) -> bool {
        self.profiles.values().any(|profile| {
            profile
                .bindings
                .iter()
                .any(|binding| binding.template_id == template_id)
        })
    }
}
