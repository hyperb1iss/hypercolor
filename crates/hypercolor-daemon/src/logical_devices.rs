//! Logical-device segmentation model.
//!
//! A physical controller can expose one or more logical devices, each mapped to
//! a contiguous LED range. Layout zones target these logical IDs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use hypercolor_types::device::DeviceId;

use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteOutcome, PersistenceError,
    serialize_json_pretty,
};

/// Logical-device snapshot reserved at its owning mutation boundary.
#[derive(Debug)]
pub struct LogicalDeviceSave {
    write: AdmittedAtomicWrite,
}

/// One logical device mapped onto a physical device LED range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalDevice {
    /// Stable logical device ID used by layout zones (`Output.device_id`).
    pub id: String,

    /// Back-reference to the physical device in the registry.
    pub physical_device_id: DeviceId,

    /// User-facing logical name.
    pub name: String,

    /// Inclusive LED start index on the physical controller.
    pub led_start: u32,

    /// Number of LEDs assigned to this logical device.
    pub led_count: u32,

    /// Whether this logical device participates in runtime routing.
    pub enabled: bool,

    /// Whether this is the built-in full-device mapping or a user segment.
    pub kind: LogicalDeviceKind,
}

impl LogicalDevice {
    /// Exclusive end index on the physical controller.
    #[must_use]
    pub const fn led_end_exclusive(&self) -> u32 {
        self.led_start.saturating_add(self.led_count)
    }
}

/// Logical-device source type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalDeviceKind {
    /// Auto-created full-range mapping for a physical controller.
    Default,
    /// User-defined segment.
    Segment,
}

/// Insert or refresh the default logical device for a physical device.
///
/// The default ID matches the lifecycle layout device ID so existing layouts
/// stay valid.
pub fn ensure_default_logical_device(
    store: &mut HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
    physical_layout_id: &str,
    physical_name: &str,
    physical_led_count: u32,
) -> LogicalDevice {
    let existing_default_id = store.iter().find_map(|(id, entry)| {
        (entry.physical_device_id == physical_device_id && entry.kind == LogicalDeviceKind::Default)
            .then(|| id.clone())
    });

    if let Some(existing_id) = existing_default_id.as_deref()
        && existing_id != physical_layout_id
    {
        store.remove(existing_id);
    }

    let id = physical_layout_id.to_owned();
    let has_enabled_segments = store.values().any(|entry| {
        entry.physical_device_id == physical_device_id
            && entry.kind == LogicalDeviceKind::Segment
            && entry.enabled
    });

    let entry = LogicalDevice {
        id: id.clone(),
        physical_device_id,
        name: physical_name.to_owned(),
        led_start: 0,
        led_count: physical_led_count,
        enabled: !has_enabled_segments,
        kind: LogicalDeviceKind::Default,
    };
    store.insert(id, entry.clone());
    entry
}

/// Return logical devices for one physical controller, sorted by start index.
#[must_use]
pub fn list_for_physical(
    store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) -> Vec<LogicalDevice> {
    let mut items: Vec<LogicalDevice> = store
        .values()
        .filter(|entry| entry.physical_device_id == physical_device_id)
        .cloned()
        .collect();

    items.sort_by(|left, right| {
        left.led_start
            .cmp(&right.led_start)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });

    items
}

#[derive(Debug, Deserialize)]
struct PersistedLogicalDevice {
    id: String,
    physical_device_id: DeviceId,
    name: String,
    led_start: u32,
    led_count: u32,
    enabled: bool,
    kind: String,
}

impl PersistedLogicalDevice {
    fn into_runtime(self) -> Option<LogicalDevice> {
        (self.kind == "segment").then_some(LogicalDevice {
            id: self.id,
            physical_device_id: self.physical_device_id,
            name: self.name,
            led_start: self.led_start,
            led_count: self.led_count,
            enabled: self.enabled,
            kind: LogicalDeviceKind::Segment,
        })
    }
}

/// Load persisted user-defined logical segment devices from disk.
///
/// Missing files return an empty store.
pub fn load_segments(path: &Path) -> anyhow::Result<HashMap<String, LogicalDevice>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read logical device store at {}", path.display()))?;
    let entries: Vec<PersistedLogicalDevice> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse logical device store at {}", path.display()))?;
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        if let Some(entry) = entry.into_runtime() {
            out.insert(entry.id.clone(), entry);
        }
    }
    Ok(out)
}

/// Persist user-defined logical segment devices to disk.
///
/// Default logical devices are ephemeral and are not persisted.
pub fn save_segments(path: &Path, store: &HashMap<String, LogicalDevice>) -> anyhow::Result<()> {
    let pending = reserve_save_segments(path, store)?;
    save_reserved_segments(pending).map(|_| ())
}

/// Reserve a logical-device snapshot before releasing its mutation lock.
pub fn reserve_save_segments(
    path: &Path,
    store: &HashMap<String, LogicalDevice>,
) -> anyhow::Result<LogicalDeviceSave> {
    let writer = AtomicFileWriter::new(path)?;
    reserve_save_segments_with(&writer, store)
}

/// Initialize a logical-device writer before its owning mutation begins.
pub fn writer(path: &Path) -> Result<AtomicFileWriter, PersistenceError> {
    AtomicFileWriter::new(path)
}

/// Serialize and admit logical segments while their mutation lock is held.
pub fn reserve_save_segments_with(
    writer: &AtomicFileWriter,
    store: &HashMap<String, LogicalDevice>,
) -> anyhow::Result<LogicalDeviceSave> {
    let mut entries: Vec<LogicalDevice> = store
        .values()
        .filter(|entry| entry.kind == LogicalDeviceKind::Segment)
        .cloned()
        .collect();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    let payload =
        serialize_json_pretty(&entries).context("failed to serialize logical device store")?;
    Ok(LogicalDeviceSave {
        write: writer.reserve().admit(payload),
    })
}

/// Commit a previously reserved logical-device snapshot.
pub fn save_reserved_segments(pending: LogicalDeviceSave) -> anyhow::Result<AtomicWriteOutcome> {
    pending
        .write
        .commit()
        .context("failed to persist logical device store")
}

/// Wake a pending retry after a semantic no-op.
pub fn kick_pending(path: &Path) -> Result<(), PersistenceError> {
    AtomicFileWriter::new(path)?.kick();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::{LogicalDevice, LogicalDeviceKind, load_segments, save_segments};
    use crate::logical_devices::ensure_default_logical_device;
    use hypercolor_types::device::DeviceId;

    #[test]
    fn ensure_default_replaces_outdated_default_id() {
        let physical_device_id = DeviceId::new();
        let mut store = HashMap::new();
        store.insert(
            "driver:old-id".to_owned(),
            LogicalDevice {
                id: "driver:old-id".to_owned(),
                physical_device_id,
                name: "Fixture Device".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );

        let canonical = ensure_default_logical_device(
            &mut store,
            physical_device_id,
            "driver:new-id",
            "Fixture Device",
            60,
        );

        assert_eq!(canonical.id, "driver:new-id");
        assert_eq!(canonical.kind, LogicalDeviceKind::Default);
        assert!(!store.contains_key("driver:old-id"));
    }

    #[test]
    fn save_and_load_preserves_segments_but_not_live_defaults() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("logical-devices.json");
        let physical_device_id = DeviceId::new();

        let mut store = HashMap::new();
        store.insert(
            "driver:canonical".to_owned(),
            LogicalDevice {
                id: "driver:canonical".to_owned(),
                physical_device_id,
                name: "Fixture Device".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );
        store.insert(
            "driver:canonical:left".to_owned(),
            LogicalDevice {
                id: "driver:canonical:left".to_owned(),
                physical_device_id,
                name: "Fixture Segment".to_owned(),
                led_start: 0,
                led_count: 20,
                enabled: true,
                kind: LogicalDeviceKind::Segment,
            },
        );

        save_segments(&path, &store).expect("save logical device store");
        let loaded = load_segments(&path).expect("load logical device store");

        assert!(loaded.contains_key("driver:canonical:left"));
        assert!(
            !loaded.contains_key("driver:canonical"),
            "live canonical defaults should still be rebuilt at runtime"
        );
    }

    #[test]
    fn load_segments_ignores_non_segment_entries() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("logical-devices.json");
        let physical_device_id = DeviceId::new();
        let payload = format!(
            r#"[
  {{
    "id": "driver:default",
    "physical_device_id": "{physical_device_id}",
    "name": "Fixture Device",
    "led_start": 0,
    "led_count": 60,
    "enabled": true,
    "kind": "default"
  }},
  {{
    "id": "driver:left",
    "physical_device_id": "{physical_device_id}",
    "name": "Fixture Segment",
    "led_start": 0,
    "led_count": 20,
    "enabled": true,
    "kind": "segment"
  }}
]"#
        );
        std::fs::write(&path, payload).expect("write logical device store");

        let loaded = load_segments(&path).expect("load logical device store");

        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("driver:left"));
    }
}
