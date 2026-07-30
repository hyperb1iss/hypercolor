//! Persisted effect -> layout association store.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, AtomicWriteReservation, PersistenceError,
};

/// Effect-layout snapshot reserved at its owning mutation boundary.
#[derive(Debug)]
pub struct EffectLayoutSave {
    associations: HashMap<String, String>,
    write: AtomicWriteReservation,
}

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
    let pending = reserve_save(path, associations)?;
    save_reserved(pending).map(|_| ())
}

/// Reserve an effect-layout snapshot before releasing its mutation lock.
pub fn reserve_save(
    path: &Path,
    associations: &HashMap<String, String>,
) -> Result<EffectLayoutSave, PersistenceError> {
    let writer = AtomicFileWriter::new(path)?;
    Ok(EffectLayoutSave {
        associations: associations.clone(),
        write: writer.reserve(),
    })
}

/// Commit a previously reserved effect-layout snapshot.
pub fn save_reserved(pending: EffectLayoutSave) -> anyhow::Result<AtomicWriteOutcome> {
    let payload = serde_json::to_string_pretty(&pending.associations)
        .context("failed to serialize effect layout associations")?;
    let outcome = pending
        .write
        .write(payload.as_bytes())
        .context("failed to persist effect layout associations")?;
    Ok(outcome)
}
