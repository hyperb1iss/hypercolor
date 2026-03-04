//! Persisted effect -> layout association store.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;

/// Load persisted effect layout associations from disk.
///
/// Missing files return an empty map.
pub fn load(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read effect layout associations at {}",
            path.display()
        )
    })?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "failed to parse effect layout associations at {}",
            path.display()
        )
    })
}

/// Persist effect layout associations to disk.
pub fn save(path: &Path, associations: &HashMap<String, String>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create effect layout association directory {}",
                parent.display()
            )
        })?;
    }

    let payload = serde_json::to_string_pretty(associations)
        .context("failed to serialize effect layout associations")?;
    let tmp_path = path.with_extension("tmp");

    fs::write(&tmp_path, payload).with_context(|| {
        format!(
            "failed to write temporary effect layout association file {}",
            tmp_path.display()
        )
    })?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move temporary effect layout association file {} into {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}
