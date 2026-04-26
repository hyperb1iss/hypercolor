//! Persisted output settings: global brightness plus per-device user settings.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hypercolor_types::controls::ControlValueMap;
use serde::{Deserialize, Serialize};

fn default_brightness() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredDeviceSettings {
    pub name: Option<String>,
    pub disabled: bool,
    #[serde(default = "default_brightness")]
    pub brightness: f32,
}

impl Default for StoredDeviceSettings {
    fn default() -> Self {
        Self {
            name: None,
            disabled: false,
            brightness: default_brightness(),
        }
    }
}

impl StoredDeviceSettings {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.name = self
            .name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        self.brightness = self.brightness.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        self.name.is_none() && !self.disabled && self.brightness >= 0.999
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
struct PersistedSettingsSnapshot {
    #[serde(default = "default_brightness")]
    global_brightness: f32,
    devices: HashMap<String, StoredDeviceSettings>,
    driver_controls: HashMap<String, ControlValueMap>,
}

impl Default for PersistedSettingsSnapshot {
    fn default() -> Self {
        Self {
            global_brightness: default_brightness(),
            devices: HashMap::new(),
            driver_controls: HashMap::new(),
        }
    }
}

/// JSON-backed per-device settings store.
#[derive(Debug, Clone)]
pub struct DeviceSettingsStore {
    path: PathBuf,
    snapshot: PersistedSettingsSnapshot,
}

impl DeviceSettingsStore {
    /// Create an empty store rooted at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            snapshot: PersistedSettingsSnapshot::default(),
        }
    }

    /// Load an existing store or create an empty one when absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read device settings at {}", path.display()))?;
        let snapshot = serde_json::from_str::<PersistedSettingsSnapshot>(&raw)
            .with_context(|| format!("failed to parse device settings at {}", path.display()))?;

        let mut store = Self {
            path: path.to_path_buf(),
            snapshot,
        };
        store.normalize();

        Ok(store)
    }

    /// Return the configured global brightness scalar.
    #[must_use]
    pub fn global_brightness(&self) -> f32 {
        self.snapshot.global_brightness.clamp(0.0, 1.0)
    }

    /// Persist a global brightness scalar.
    pub fn set_global_brightness(&mut self, brightness: f32) {
        self.snapshot.global_brightness = brightness.clamp(0.0, 1.0);
    }

    /// Return stored settings for a persisted device settings key.
    #[must_use]
    pub fn device_settings_for_key(&self, key: &str) -> Option<StoredDeviceSettings> {
        self.snapshot
            .devices
            .get(key)
            .cloned()
            .map(StoredDeviceSettings::normalized)
    }

    /// Update all persisted settings for a persisted device settings key.
    pub fn set_device_settings(&mut self, key: &str, settings: StoredDeviceSettings) {
        let normalized = settings.normalized();
        if normalized.is_default() {
            self.snapshot.devices.remove(key);
        } else {
            self.snapshot.devices.insert(key.to_owned(), normalized);
        }
    }

    /// Persist just the device brightness scalar.
    pub fn set_device_brightness(&mut self, key: &str, brightness: f32) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.brightness = brightness;
        self.set_device_settings(key, settings);
    }

    /// Persist just the device name override.
    pub fn set_device_name(&mut self, key: &str, name: Option<String>) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.name = name;
        self.set_device_settings(key, settings);
    }

    /// Persist just the device enabled flag.
    pub fn set_device_enabled(&mut self, key: &str, enabled: bool) {
        let mut settings = self.device_settings_for_key(key).unwrap_or_default();
        settings.disabled = !enabled;
        self.set_device_settings(key, settings);
    }

    #[must_use]
    pub fn driver_control_values_for_key(&self, key: &str) -> ControlValueMap {
        self.snapshot
            .driver_controls
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_driver_control_values(&mut self, key: &str, values: ControlValueMap) {
        if values.is_empty() {
            self.snapshot.driver_controls.remove(key);
        } else {
            self.snapshot.driver_controls.insert(key.to_owned(), values);
        }
    }

    /// Save the current snapshot to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create device settings directory {}",
                    parent.display()
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(&PersistedSettingsSnapshot {
            global_brightness: self.global_brightness(),
            devices: self
                .snapshot
                .devices
                .iter()
                .map(|(key, settings)| (key.clone(), settings.clone().normalized()))
                .collect(),
            driver_controls: self.snapshot.driver_controls.clone(),
        })
        .context("failed to serialize device settings")?;
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, payload).with_context(|| {
            format!(
                "failed to write temporary device settings {}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to move temporary device settings {} into {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;

        Ok(())
    }

    fn normalize(&mut self) {
        self.snapshot.global_brightness = self.snapshot.global_brightness.clamp(0.0, 1.0);
        self.snapshot.devices.retain(|_, settings| {
            *settings = settings.clone().normalized();
            !settings.is_default()
        });
        self.snapshot
            .driver_controls
            .retain(|_, values| !values.is_empty());
    }
}
