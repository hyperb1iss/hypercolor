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
