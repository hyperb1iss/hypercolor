//! Persisted layout-specific exclusions for discovery-driven layout reconciliation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Context;
use hypercolor_types::spatial::DeviceZone;
use serde::{Deserialize, Serialize};

/// In-memory layout exclusion store keyed by layout ID.
pub type LayoutAutoExclusionStore = HashMap<String, HashSet<String>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedLayoutAutoExclusionEntry {
    layout_id: String,
    #[serde(default)]
    excluded_device_ids: Vec<String>,
}

/// Load persisted layout auto-exclusions from disk.
///
/// Missing files return an empty store.
pub fn load(path: &Path) -> anyhow::Result<LayoutAutoExclusionStore> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read layout auto-exclusions at {}",
            path.display()
        )
    })?;
    let entries: Vec<PersistedLayoutAutoExclusionEntry> =
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "failed to parse layout auto-exclusions at {}",
                path.display()
            )
        })?;

    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let excluded_device_ids = entry
            .excluded_device_ids
            .into_iter()
            .filter(|device_id| !device_id.trim().is_empty())
            .collect::<HashSet<_>>();
        if !excluded_device_ids.is_empty() {
            out.insert(entry.layout_id, excluded_device_ids);
        }
    }

    Ok(out)
}

/// Persist layout auto-exclusions to disk using atomic-replace semantics.
pub fn save(path: &Path, store: &LayoutAutoExclusionStore) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create layout auto-exclusion directory {}",
                parent.display()
            )
        })?;
    }

    let mut entries = store
        .iter()
        .filter_map(|(layout_id, device_ids)| {
            if device_ids.is_empty() {
                return None;
            }

            let mut excluded_device_ids = device_ids.iter().cloned().collect::<Vec<_>>();
            excluded_device_ids.sort();
            Some(PersistedLayoutAutoExclusionEntry {
                layout_id: layout_id.clone(),
                excluded_device_ids,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.layout_id.cmp(&right.layout_id));

    let payload = serde_json::to_string_pretty(&entries)
        .context("failed to serialize layout auto-exclusions")?;
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, payload).with_context(|| {
        format!(
            "failed to write temporary layout auto-exclusion file {}",
            tmp_path.display()
        )
    })?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move temporary layout auto-exclusion file {} into {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

/// Merge intentional device removals from a saved layout edit into the exclusion set.
///
/// Devices that were present before the save and are absent afterward are treated as
/// intentionally removed and become excluded from future discovery-driven layout
/// reconciliation. Any device that appears in the saved layout is removed from the
/// exclusion set.
#[must_use]
pub fn reconcile_layout_device_exclusions(
    previous_zones: &[DeviceZone],
    updated_zones: &[DeviceZone],
    existing_exclusions: &HashSet<String>,
) -> HashSet<String> {
    let previous_device_ids = zone_device_ids(previous_zones);
    let updated_device_ids = zone_device_ids(updated_zones);
    let mut next = existing_exclusions.clone();

    for device_id in previous_device_ids.difference(&updated_device_ids) {
        next.insert((*device_id).clone());
    }

    for device_id in &updated_device_ids {
        next.remove(device_id);
    }

    next
}

fn zone_device_ids(zones: &[DeviceZone]) -> HashSet<String> {
    zones
        .iter()
        .map(|zone| zone.device_id.clone())
        .collect::<HashSet<_>>()
}
