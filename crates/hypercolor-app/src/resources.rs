//! Runtime installation of resources staged inside the native app bundle.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;

/// Summary for one staged resource installation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceInstallReport {
    /// Source directory inside the Tauri resource root.
    pub source: PathBuf,
    /// Destination directory in Hypercolor's data root.
    pub destination: PathBuf,
    /// Number of files copied or overwritten.
    pub copied_files: usize,
}

/// Install bundled resources from an optional Tauri resource directory.
///
/// # Errors
///
/// Returns an error if the bundled resource tree exists but cannot be copied
/// into Hypercolor's data directory.
pub fn install_bundled_runtime_assets(
    resource_dir: Option<&Path>,
) -> Result<Option<ResourceInstallReport>> {
    let Some(resource_dir) = resource_dir else {
        return Ok(None);
    };

    let source = bundled_effects_resource_dir(resource_dir);
    if !source.is_dir() {
        return Ok(None);
    }

    let destination = bundled_effects_install_dir();
    install_bundled_effects(&source, &destination).map(Some)
}

/// Resolve the bundled effects source directory inside a Tauri resource root.
#[must_use]
pub fn bundled_effects_resource_dir(resource_dir: &Path) -> PathBuf {
    resource_dir.join("effects").join("bundled")
}

/// Resolve the daemon-visible bundled effects installation directory.
#[must_use]
pub fn bundled_effects_install_dir() -> PathBuf {
    data_dir().join("effects").join("bundled")
}

/// Copy staged bundled effects into the daemon-visible installation directory.
///
/// # Errors
///
/// Returns an error if directories cannot be created, traversed, or copied.
pub fn install_bundled_effects(source: &Path, destination: &Path) -> Result<ResourceInstallReport> {
    let copied_files = copy_tree(source, destination)?;
    Ok(ResourceInstallReport {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        copied_files,
    })
}

fn copy_tree(source: &Path, destination: &Path) -> Result<usize> {
    fs::create_dir_all(destination).with_context(|| {
        format!(
            "failed to create bundled resource directory {}",
            destination.display()
        )
    })?;

    let mut copied_files = 0;
    for entry in fs::read_dir(source).with_context(|| {
        format!(
            "failed to read bundled resource directory {}",
            source.display()
        )
    })? {
        let entry =
            entry.with_context(|| format!("failed to read entry under {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().with_context(|| {
            format!(
                "failed to inspect bundled resource {}",
                source_path.display()
            )
        })?;

        if file_type.is_dir() {
            copied_files += copy_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create bundled resource directory {}",
                        parent.display()
                    )
                })?;
            }
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy bundled resource {} to {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
            copied_files += 1;
        }
    }

    Ok(copied_files)
}
