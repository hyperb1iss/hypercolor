//! Persisted effect -> layout association store.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteOutcome, PersistenceError,
    serialize_json_pretty,
};

/// Effect-layout snapshot reserved at its owning mutation boundary.
#[derive(Debug)]
pub struct EffectLayoutSave {
    write: AdmittedAtomicWrite,
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
) -> anyhow::Result<EffectLayoutSave> {
    let writer = AtomicFileWriter::new(path)?;
    reserve_save_with(&writer, associations)
}

/// Initialize an effect-layout writer before its owning mutation begins.
pub fn writer(path: &Path) -> Result<AtomicFileWriter, PersistenceError> {
    AtomicFileWriter::new(path)
}

/// Serialize and admit an effect-layout snapshot while its mutation lock is held.
pub fn reserve_save_with(
    writer: &AtomicFileWriter,
    associations: &HashMap<String, String>,
) -> anyhow::Result<EffectLayoutSave> {
    let payload = serialize_json_pretty(associations)
        .context("failed to serialize effect layout associations")?;
    Ok(EffectLayoutSave {
        write: writer.reserve().admit(payload),
    })
}

/// Commit a previously reserved effect-layout snapshot.
pub fn save_reserved(pending: EffectLayoutSave) -> anyhow::Result<AtomicWriteOutcome> {
    let outcome = pending
        .write
        .commit()
        .context("failed to persist effect layout associations")?;
    Ok(outcome)
}
