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

/// One logical device mapped onto a physical device LED range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalDevice {
    /// Stable logical device ID used by layout zones (`DeviceZone.device_id`).
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
    /// Persisted compatibility alias for an older default layout ID.
    LegacyDefault,
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

    if let Some(existing_id) = existing_default_id.as_deref() {
        if existing_id != physical_layout_id
            && let Some(existing) = store.get_mut(existing_id)
        {
            existing.kind = LogicalDeviceKind::LegacyDefault;
            existing.enabled = false;
        }
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

/// Return legacy default logical IDs that still point at this physical device.
///
/// This lets runtime routing preserve compatibility for layouts saved before
/// the canonical lifecycle layout ID was known.
#[must_use]
pub fn legacy_default_ids_for_physical(
    store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
    canonical_id: &str,
) -> Vec<String> {
    let mut legacy_ids: Vec<String> = store
        .values()
        .filter(|entry| {
            entry.physical_device_id == physical_device_id
                && entry.kind == LogicalDeviceKind::LegacyDefault
                && entry.id != canonical_id
        })
        .map(|entry| entry.id.clone())
        .collect();
    legacy_ids.sort();
    legacy_ids.dedup();
    legacy_ids
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

/// Find the default logical-device ID for a physical controller.
#[must_use]
pub fn default_id_for_physical(
    store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) -> Option<String> {
    store
        .values()
        .find(|entry| {
            entry.physical_device_id == physical_device_id
                && entry.kind == LogicalDeviceKind::Default
        })
        .map(|entry| entry.id.clone())
}

/// Ensure the default logical entry is enabled iff there are no enabled segments.
pub fn reconcile_default_enabled(
    store: &mut HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) {
    let has_enabled_segments = store.values().any(|entry| {
        entry.physical_device_id == physical_device_id
            && entry.kind == LogicalDeviceKind::Segment
            && entry.enabled
    });

    if let Some(default) = store.values_mut().find(|entry| {
        entry.physical_device_id == physical_device_id && entry.kind == LogicalDeviceKind::Default
    }) {
        default.enabled = !has_enabled_segments;
    }
}

/// Validate one logical device range against the physical LED count and peers.
///
/// Overlapping enabled segment ranges are rejected.
pub fn validate_entry(
    store: &HashMap<String, LogicalDevice>,
    candidate: &LogicalDevice,
    physical_led_count: u32,
    ignore_id: Option<&str>,
) -> Result<(), String> {
    if candidate.led_count == 0 {
        return Err("led_count must be greater than 0".to_owned());
    }

    let end = candidate.led_end_exclusive();
    if end > physical_led_count {
        return Err(format!(
            "logical range [{}, {}) exceeds physical LED count {}",
            candidate.led_start, end, physical_led_count
        ));
    }

    if !candidate.enabled {
        return Ok(());
    }

    if candidate.kind == LogicalDeviceKind::LegacyDefault {
        return Ok(());
    }

    if candidate.kind == LogicalDeviceKind::Default {
        let has_enabled_segments = store.values().any(|entry| {
            entry.physical_device_id == candidate.physical_device_id
                && entry.kind == LogicalDeviceKind::Segment
                && entry.enabled
        });
        if has_enabled_segments {
            return Err(
                "default logical device cannot be enabled while segment logical devices are enabled"
                    .to_owned(),
            );
        }
        return Ok(());
    }

    let overlaps = store.values().any(|entry| {
        if entry.physical_device_id != candidate.physical_device_id {
            return false;
        }
        if entry.kind != LogicalDeviceKind::Segment || !entry.enabled {
            return false;
        }
        if let Some(ignore) = ignore_id {
            if entry.id == ignore {
                return false;
            }
        }

        let entry_end = entry.led_end_exclusive();
        candidate.led_start < entry_end && entry.led_start < end
    });

    if overlaps {
        return Err(
            "logical segment overlaps another enabled segment on this physical device".to_owned(),
        );
    }

    Ok(())
}

/// Generate a stable logical ID scoped under a physical layout prefix.
#[must_use]
pub fn allocate_segment_id(
    store: &HashMap<String, LogicalDevice>,
    physical_layout_id: &str,
    raw_name: &str,
) -> String {
    let slug = sanitize_component(raw_name);
    let base = format!("{physical_layout_id}:{slug}");
    if !store.contains_key(&base) {
        return base;
    }

    let mut n = 2_u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !store.contains_key(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

fn sanitize_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_was_dash = false;

    for ch in input.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if mapped == '-' {
            if prev_was_dash {
                continue;
            }
            prev_was_dash = true;
            out.push(mapped);
        } else {
            prev_was_dash = false;
            out.push(mapped);
        }
    }

    if out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "segment".to_owned()
    } else {
        out
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
    let mut entries: Vec<LogicalDevice> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse logical device store at {}", path.display()))?;
    entries.retain(|entry| {
        matches!(
            entry.kind,
            LogicalDeviceKind::Segment | LogicalDeviceKind::LegacyDefault
        )
    });

    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        out.insert(entry.id.clone(), entry);
    }
    Ok(out)
}

/// Persist user-defined logical segment devices to disk.
///
/// Default logical devices are ephemeral and are not persisted.
pub fn save_segments(path: &Path, store: &HashMap<String, LogicalDevice>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create logical device store directory {}",
                parent.display()
            )
        })?;
    }

    let mut entries: Vec<LogicalDevice> = store
        .values()
        .filter(|entry| {
            matches!(
                entry.kind,
                LogicalDeviceKind::Segment | LogicalDeviceKind::LegacyDefault
            )
        })
        .cloned()
        .collect();
    entries.sort_by(|left, right| left.id.cmp(&right.id));

    let payload = serde_json::to_string_pretty(&entries)
        .context("failed to serialize logical device store")?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, payload).with_context(|| {
        format!(
            "failed to write temporary logical device store {}",
            tmp_path.display()
        )
    })?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move temporary logical device store {} into {}",
            tmp_path.display(),
            path.display()
        )
    })?;

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
    fn ensure_default_promotes_previous_default_to_legacy_alias() {
        let physical_device_id = DeviceId::new();
        let mut store = HashMap::new();
        store.insert(
            "wled:old-id".to_owned(),
            LogicalDevice {
                id: "wled:old-id".to_owned(),
                physical_device_id,
                name: "Desk Strip".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );

        let canonical = ensure_default_logical_device(
            &mut store,
            physical_device_id,
            "wled:new-id",
            "Desk Strip",
            60,
        );

        assert_eq!(canonical.id, "wled:new-id");
        assert_eq!(canonical.kind, LogicalDeviceKind::Default);

        let legacy = store
            .get("wled:old-id")
            .expect("previous default should remain as a legacy alias");
        assert_eq!(legacy.kind, LogicalDeviceKind::LegacyDefault);
        assert!(!legacy.enabled);
    }

    #[test]
    fn save_and_load_preserves_legacy_aliases_but_not_live_defaults() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("logical-devices.json");
        let physical_device_id = DeviceId::new();

        let mut store = HashMap::new();
        store.insert(
            "wled:canonical".to_owned(),
            LogicalDevice {
                id: "wled:canonical".to_owned(),
                physical_device_id,
                name: "Desk Strip".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );
        store.insert(
            "wled:legacy".to_owned(),
            LogicalDevice {
                id: "wled:legacy".to_owned(),
                physical_device_id,
                name: "Desk Strip".to_owned(),
                led_start: 0,
                led_count: 60,
                enabled: false,
                kind: LogicalDeviceKind::LegacyDefault,
            },
        );
        store.insert(
            "wled:canonical:left".to_owned(),
            LogicalDevice {
                id: "wled:canonical:left".to_owned(),
                physical_device_id,
                name: "Desk Left".to_owned(),
                led_start: 0,
                led_count: 20,
                enabled: true,
                kind: LogicalDeviceKind::Segment,
            },
        );

        save_segments(&path, &store).expect("save logical device store");
        let loaded = load_segments(&path).expect("load logical device store");

        assert!(loaded.contains_key("wled:legacy"));
        assert!(loaded.contains_key("wled:canonical:left"));
        assert!(
            !loaded.contains_key("wled:canonical"),
            "live canonical defaults should still be rebuilt at runtime"
        );
    }
}
