//! Persisted device attachment profile store.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_hal::drivers::prismrgb::{PrismSConfig, PrismSGpuCable};
use hypercolor_types::attachment::DeviceAttachmentProfile;
use hypercolor_types::device::{DeviceFamily, DeviceInfo};
use tracing::warn;

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
        let slots = device.default_attachment_profile().slots;

        if let Some(profile) = self.profiles.get(&device_id) {
            let mut profile = profile.clone();
            profile.slots = slots;
            return profile;
        }

        device.default_attachment_profile()
    }

    #[must_use]
    pub fn prism_s_config_for_device(
        &self,
        device: &DeviceInfo,
        registry: &AttachmentRegistry,
    ) -> Option<PrismSConfig> {
        if device.family != DeviceFamily::PrismRgb || device.model.as_deref() != Some("prism_s") {
            return None;
        }

        let profile = self.get_or_default(device);
        let has_enabled_bindings = profile.bindings.iter().any(|binding| binding.enabled);
        if !has_enabled_bindings {
            return Some(PrismSConfig::default());
        }

        let mut config = PrismSConfig {
            atx_present: false,
            gpu_cable: None,
        };

        for binding in profile.bindings.iter().filter(|binding| binding.enabled) {
            match binding.slot_id.as_str() {
                "atx-strimer" => config.atx_present = true,
                "gpu-strimer" => {
                    let Some(template) = registry.get(&binding.template_id) else {
                        warn!(
                            device_id = %device.id,
                            template_id = %binding.template_id,
                            "attachment profile references unknown Prism S template; skipping GPU binding"
                        );
                        continue;
                    };

                    let effective_led_count = binding.effective_led_count(template);
                    config.gpu_cable = match effective_led_count {
                        108 => Some(PrismSGpuCable::Dual8Pin),
                        162 => Some(PrismSGpuCable::Triple8Pin),
                        _ => {
                            warn!(
                                device_id = %device.id,
                                template_id = %binding.template_id,
                                effective_led_count,
                                "attachment profile template does not match a supported Prism S GPU cable"
                            );
                            config.gpu_cable
                        }
                    };
                }
                _ => {}
            }
        }

        Some(config)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hypercolor_core::attachment::AttachmentRegistry;
    use hypercolor_types::attachment::{AttachmentBinding, AttachmentSlot};
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceId, DeviceInfo,
        DeviceTopologyHint, ZoneInfo,
    };

    use super::AttachmentProfileStore;

    fn prism_s_info() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "PrismRGB Prism S".to_owned(),
            vendor: "PrismRGB".to_owned(),
            family: hypercolor_types::device::DeviceFamily::PrismRgb,
            model: Some("prism_s".to_owned()),
            connection_type: ConnectionType::Usb,
            zones: vec![
                ZoneInfo {
                    name: "ATX Strimer".to_owned(),
                    led_count: 120,
                    topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                    color_format: DeviceColorFormat::Rgb,
                },
                ZoneInfo {
                    name: "GPU Strimer".to_owned(),
                    led_count: 162,
                    topology: DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
                    color_format: DeviceColorFormat::Rgb,
                },
            ],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    #[test]
    fn prism_s_config_defaults_to_legacy_full_topology_without_bindings() {
        let info = prism_s_info();
        let store = AttachmentProfileStore::new(PathBuf::from("attachment-profiles-test.json"));
        let mut registry = AttachmentRegistry::new();
        registry
            .load_builtins()
            .expect("built-in attachments should load");

        let config = store
            .prism_s_config_for_device(&info, &registry)
            .expect("Prism S config should be derived");

        assert!(config.atx_present);
        assert_eq!(
            config.gpu_cable,
            Some(hypercolor_hal::drivers::prismrgb::PrismSGpuCable::Triple8Pin)
        );
    }

    #[test]
    fn prism_s_config_derives_dual_gpu_from_attachment_binding() {
        let info = prism_s_info();
        let mut store = AttachmentProfileStore::new(PathBuf::from("attachment-profiles-test.json"));
        let mut profile = info.default_attachment_profile();
        profile.bindings = vec![
            AttachmentBinding {
                slot_id: "atx-strimer".to_owned(),
                template_id: "lian-li-atx-strimer".to_owned(),
                name: None,
                enabled: true,
                instances: 1,
                led_offset: 0,
            },
            AttachmentBinding {
                slot_id: "gpu-strimer".to_owned(),
                template_id: "lian-li-gpu-strimer-4x27".to_owned(),
                name: None,
                enabled: true,
                instances: 1,
                led_offset: 0,
            },
        ];
        store.update(&info.id.to_string(), profile);

        let mut registry = AttachmentRegistry::new();
        registry
            .load_builtins()
            .expect("built-in attachments should load");

        let config = store
            .prism_s_config_for_device(&info, &registry)
            .expect("Prism S config should be derived");

        assert!(config.atx_present);
        assert_eq!(
            config.gpu_cable,
            Some(hypercolor_hal::drivers::prismrgb::PrismSGpuCable::Dual8Pin)
        );
    }

    #[test]
    fn prism_s_config_supports_gpu_only_profiles() {
        let info = prism_s_info();
        let mut store = AttachmentProfileStore::new(PathBuf::from("attachment-profiles-test.json"));
        let profile = hypercolor_types::attachment::DeviceAttachmentProfile {
            schema_version: 1,
            slots: vec![AttachmentSlot {
                id: "gpu-strimer".to_owned(),
                name: "GPU Strimer".to_owned(),
                led_start: 0,
                led_count: 162,
                suggested_categories: vec![],
                allowed_templates: vec![],
                allow_custom: true,
            }],
            bindings: vec![AttachmentBinding {
                slot_id: "gpu-strimer".to_owned(),
                template_id: "lian-li-gpu-strimer-4x27".to_owned(),
                name: None,
                enabled: true,
                instances: 1,
                led_offset: 0,
            }],
            suggested_zones: vec![],
        };
        store.update(&info.id.to_string(), profile);

        let mut registry = AttachmentRegistry::new();
        registry
            .load_builtins()
            .expect("built-in attachments should load");

        let config = store
            .prism_s_config_for_device(&info, &registry)
            .expect("Prism S config should be derived");

        assert!(!config.atx_present);
        assert_eq!(
            config.gpu_cable,
            Some(hypercolor_hal::drivers::prismrgb::PrismSGpuCable::Dual8Pin)
        );
    }
}
