//! Persisted effect -> layout association store.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::persistence::write_atomic;

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
    write_atomic(path, payload.as_bytes())
        .context("failed to persist effect layout associations")?;

    Ok(())
}
