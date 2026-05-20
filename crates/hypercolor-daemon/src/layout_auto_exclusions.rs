//! Persisted exclusions for discovery-driven layout reconciliation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::Context;
use hypercolor_types::scene::{SceneId, ZoneId};
use hypercolor_types::spatial::Output;
use serde::{Deserialize, Serialize};

/// Discovery auto-sync exclusion scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LayoutAutoExclusionKey {
    LegacyLayout(String),
    Zone { scene_id: SceneId, zone_id: ZoneId },
}

impl LayoutAutoExclusionKey {
    pub fn layout(layout_id: impl Into<String>) -> Self {
        Self::LegacyLayout(layout_id.into())
    }

    #[must_use]
    pub const fn zone(scene_id: SceneId, zone_id: ZoneId) -> Self {
        Self::Zone { scene_id, zone_id }
    }

    fn sort_key(&self) -> String {
        match self {
            Self::LegacyLayout(layout_id) => format!("layout:{layout_id}"),
            Self::Zone { scene_id, zone_id } => format!("zone:{scene_id}:{zone_id}"),
        }
    }
}

impl fmt::Display for LayoutAutoExclusionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegacyLayout(layout_id) => write!(f, "layout:{layout_id}"),
            Self::Zone { scene_id, zone_id } => write!(f, "zone:{scene_id}:{zone_id}"),
        }
    }
}

/// In-memory layout exclusion store keyed by legacy layout or scene-zone scope.
pub type LayoutAutoExclusionStore = HashMap<LayoutAutoExclusionKey, HashSet<String>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedLayoutAutoExclusionEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene_id: Option<SceneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    zone_id: Option<ZoneId>,
    #[serde(default)]
    excluded_device_ids: Vec<String>,
}

impl PersistedLayoutAutoExclusionEntry {
    fn key(&self) -> Option<LayoutAutoExclusionKey> {
        if self.scope.as_deref() == Some("zone") {
            return Some(LayoutAutoExclusionKey::zone(self.scene_id?, self.zone_id?));
        }

        if let (Some(scene_id), Some(zone_id)) = (self.scene_id, self.zone_id) {
            return Some(LayoutAutoExclusionKey::zone(scene_id, zone_id));
        }

        self.layout_id
            .as_ref()
            .filter(|layout_id| !layout_id.trim().is_empty())
            .map(|layout_id| LayoutAutoExclusionKey::layout(layout_id.clone()))
    }

    fn from_key(key: &LayoutAutoExclusionKey, excluded_device_ids: Vec<String>) -> Self {
        match key {
            LayoutAutoExclusionKey::LegacyLayout(layout_id) => Self {
                scope: None,
                layout_id: Some(layout_id.clone()),
                scene_id: None,
                zone_id: None,
                excluded_device_ids,
            },
            LayoutAutoExclusionKey::Zone { scene_id, zone_id } => Self {
                scope: Some("zone".to_owned()),
                layout_id: None,
                scene_id: Some(*scene_id),
                zone_id: Some(*zone_id),
                excluded_device_ids,
            },
        }
    }

    fn sort_key(&self) -> String {
        self.key().map_or_else(String::new, |key| key.sort_key())
    }
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
        let Some(key) = entry.key() else {
            continue;
        };
        let excluded_device_ids = entry
            .excluded_device_ids
            .into_iter()
            .filter(|device_id| !device_id.trim().is_empty())
            .collect::<HashSet<_>>();
        if !excluded_device_ids.is_empty() {
            out.insert(key, excluded_device_ids);
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
        .filter_map(|(key, device_ids)| {
            if device_ids.is_empty() {
                return None;
            }

            let mut excluded_device_ids = device_ids.iter().cloned().collect::<Vec<_>>();
            excluded_device_ids.sort();
            Some(PersistedLayoutAutoExclusionEntry::from_key(
                key,
                excluded_device_ids,
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(PersistedLayoutAutoExclusionEntry::sort_key);

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
    previous_zones: &[Output],
    updated_zones: &[Output],
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

fn zone_device_ids(zones: &[Output]) -> HashSet<String> {
    zones
        .iter()
        .map(|zone| zone.device_id.clone())
        .collect::<HashSet<_>>()
}
