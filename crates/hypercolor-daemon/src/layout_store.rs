//! Persisted spatial layout store.
//!
//! Layouts are stored as a JSON array in `layouts.json` within the XDG data
//! directory. Atomic-replace semantics prevent partial writes.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use hypercolor_types::spatial::SpatialLayout;

/// Load persisted spatial layouts from disk.
///
/// Missing files return an empty store.
pub fn load(path: &Path) -> anyhow::Result<HashMap<String, SpatialLayout>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read layout store at {}", path.display()))?;
    let entries: Vec<SpatialLayout> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse layout store at {}", path.display()))?;

    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        out.insert(entry.id.clone(), entry);
    }
    Ok(out)
}

/// Persist spatial layouts to disk using atomic-replace semantics.
pub fn save(path: &Path, store: &HashMap<String, SpatialLayout>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create layout store directory {}",
                parent.display()
            )
        })?;
    }

    let mut entries: Vec<&SpatialLayout> = store.values().collect();
    entries.sort_by(|left, right| left.id.cmp(&right.id));

    let payload =
        serde_json::to_string_pretty(&entries).context("failed to serialize layout store")?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, payload).with_context(|| {
        format!(
            "failed to write temporary layout store {}",
            tmp_path.display()
        )
    })?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move temporary layout store {} into {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection, ZoneGroup,
    };
    use tempfile::TempDir;

    use super::{load, save};

    fn sample_layout() -> SpatialLayout {
        SpatialLayout {
            id: "layout_saved".into(),
            name: "Saved Layout".into(),
            description: Some("Persisted for restore".into()),
            canvas_width: 320,
            canvas_height: 200,
            zones: vec![DeviceZone {
                id: "zone-1".into(),
                name: "Desk Strip".into(),
                device_id: "wled:desk".into(),
                zone_name: None,
                group_id: Some("desk".into()),
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(0.4, 0.1),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 30,
                    direction: StripDirection::LeftToRight,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: None,
                edge_behavior: None,
                shape: None,
                shape_preset: None,
                attachment: None,
            }],
            groups: vec![ZoneGroup {
                id: "desk".into(),
                name: "Desk".into(),
                color: Some("#80ffea".into()),
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    #[test]
    fn load_returns_empty_store_when_file_is_missing() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("layouts.json");

        let loaded = load(&path).expect("missing layout store should load as empty");

        assert!(loaded.is_empty());
    }

    #[test]
    fn save_and_load_round_trip_layouts_with_groups() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("layouts.json");
        let layout = sample_layout();
        let mut store = HashMap::new();
        store.insert(layout.id.clone(), layout.clone());

        save(&path, &store).expect("save layout store");
        let loaded = load(&path).expect("load layout store");
        let recovered = loaded
            .get(&layout.id)
            .expect("saved layout should round-trip");

        assert_eq!(recovered.name, layout.name);
        assert_eq!(recovered.groups.len(), 1);
        assert_eq!(recovered.groups[0].id, "desk");
        assert_eq!(recovered.zones[0].group_id.as_deref(), Some("desk"));
    }
}
