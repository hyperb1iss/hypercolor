//! Persisted device attachment profile store.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use hypercolor_types::attachment::{AttachmentSlot, DeviceAttachmentProfile};
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
            format!(
                "failed to read attachment profile store at {}",
                path.display()
            )
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

        if let Some(profile) = self.profiles.get(&device_id) {
            let slots = device.default_attachment_profile().slots;
            let mut profile = profile.clone();
            profile.slots = merge_slots_preserving_ids(&profile.slots, &slots);
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

fn merge_slots_preserving_ids(
    stored_slots: &[AttachmentSlot],
    current_slots: &[AttachmentSlot],
) -> Vec<AttachmentSlot> {
    let stored_by_range = stored_slots
        .iter()
        .map(|slot| ((slot.led_start, slot.led_count), slot))
        .collect::<HashMap<_, _>>();

    current_slots
        .iter()
        .map(|slot| {
            let Some(previous_slot) = stored_by_range.get(&(slot.led_start, slot.led_count)) else {
                return slot.clone();
            };

            let mut merged = slot.clone();
            merged.id.clone_from(&previous_slot.id);
            merged
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hypercolor_types::attachment::AttachmentBinding;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
        DeviceOrigin, DeviceTopologyHint, ZoneInfo,
    };

    use super::AttachmentProfileStore;

    #[test]
    fn get_or_default_preserves_slot_ids_when_zone_names_change() {
        let original = DeviceInfo {
            id: DeviceId::new(),
            name: "Hue Area".to_owned(),
            vendor: "Philips Hue".to_owned(),
            family: DeviceFamily::new_static("hue", "Philips Hue"),
            model: Some("Bridge".to_owned()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("hue", "hue", ConnectionType::Network),
            zones: vec![
                ZoneInfo {
                    name: "Channel 0".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                },
                ZoneInfo {
                    name: "Channel 1".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                },
            ],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        };
        let renamed = DeviceInfo {
            zones: vec![
                ZoneInfo {
                    name: "Left Lamp".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                },
                ZoneInfo {
                    name: "Right Lamp".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                },
            ],
            ..original.clone()
        };
        let mut store = AttachmentProfileStore::new(PathBuf::from("attachment-profiles-test.json"));
        let mut profile = original.default_attachment_profile();
        let original_slot_ids = profile
            .slots
            .iter()
            .map(|slot| slot.id.clone())
            .collect::<Vec<_>>();
        profile.bindings = vec![AttachmentBinding {
            slot_id: original_slot_ids[0].clone(),
            template_id: "dummy-template".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        }];
        store.update(&original.id.to_string(), profile);

        let resolved = store.get_or_default(&renamed);

        assert_eq!(resolved.slots[0].name, "Left Lamp");
        assert_eq!(resolved.slots[1].name, "Right Lamp");
        assert_eq!(resolved.slots[0].id, original_slot_ids[0]);
        assert_eq!(resolved.slots[1].id, original_slot_ids[1]);
        assert!(
            resolved
                .slots
                .iter()
                .any(|slot| slot.id == resolved.bindings[0].slot_id)
        );
    }
}
