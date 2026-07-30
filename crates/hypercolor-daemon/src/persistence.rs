//! Durable cross-platform file replacement for daemon state stores.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use tempfile::NamedTempFile;

/// Write a complete file beside its destination and atomically replace it.
///
/// The temporary file lives in the destination directory so persistence never
/// crosses filesystems. `tempfile` maps replacement to `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING` on Windows and `rename` on Unix.
pub fn write_atomic(path: &Path, payload: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create persistence directory {}",
            parent.display()
        )
    })?;

    let mut temporary = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary persistence file beside {}",
            path.display()
        )
    })?;
    temporary.write_all(payload).with_context(|| {
        format!(
            "failed to write temporary persistence file for {}",
            path.display()
        )
    })?;
    temporary.as_file().sync_all().with_context(|| {
        format!(
            "failed to flush temporary persistence file for {}",
            path.display()
        )
    })?;

    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace persistence file {}", path.display()))?;

    #[cfg(unix)]
    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> anyhow::Result<()> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to flush persistence directory {}", parent.display()))
}
