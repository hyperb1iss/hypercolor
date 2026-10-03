//! Shared attachment catalog path helpers.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Return the bundled built-in attachment template root inside this crate.
#[must_use]
pub fn bundled_attachments_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("attachments")
}

/// Resolve a bundled attachment-relative path to an existing file on disk.
///
/// Accepts absolute paths, paths relative to the bundled root, or paths
/// relative to the current working directory.
pub fn resolve_attachment_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!(
            "absolute attachment template path does not exist: {}",
            path.display()
        );
    }

    let mut candidates = vec![bundled_attachments_root().join(path)];
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join(path));
    }
    candidates.push(path.to_path_buf());

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not resolve attachment template path '{}'; expected relative to bundled attachment root or current directory",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{bundled_attachments_root, resolve_attachment_path};

    #[test]
    fn bundled_attachments_root_is_the_crate_attachment_folder() {
        let root = bundled_attachments_root();
        assert_eq!(
            root.file_name().and_then(|value| value.to_str()),
            Some("attachments")
        );
        assert!(root.is_dir(), "{} should exist", root.display());
    }

    #[test]
    fn resolve_attachment_path_accepts_existing_absolute() {
        let dir = tempdir().expect("tempdir should create");
        let attachment_path = dir.path().join("attachment.toml");
        std::fs::write(&attachment_path, "schema_version = 1").expect("write should work");

        let resolved =
            resolve_attachment_path(&attachment_path).expect("absolute path should resolve");
        assert_eq!(resolved, attachment_path);
    }

    #[test]
    fn resolve_attachment_path_rejects_missing_file() {
        let missing = Path::new("this/path/does/not/exist.toml");
        let error = resolve_attachment_path(missing).expect_err("missing path should fail");
        assert!(
            error
                .to_string()
                .contains("could not resolve attachment template path")
        );
    }
}
