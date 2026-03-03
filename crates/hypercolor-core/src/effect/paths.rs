//! Shared effect source path helpers.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Return the bundled `effects/` root in the repository.
#[must_use]
pub fn bundled_effects_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../effects")
}

/// Resolve an HTML source path to an existing path on disk.
///
/// Accepts either:
/// - absolute file paths
/// - paths relative to bundled `effects/`
/// - paths relative to the current working directory
///
/// # Errors
///
/// Returns an error when no existing file can be resolved.
pub fn resolve_html_source_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!(
            "absolute HTML effect path does not exist: {}",
            path.display()
        );
    }

    let mut candidates = vec![bundled_effects_root().join(path)];
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
        "could not resolve HTML effect source '{}'; expected relative to bundled effects root or current directory",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{bundled_effects_root, resolve_html_source_path};

    #[test]
    fn bundled_effects_root_ends_with_effects() {
        assert_eq!(
            bundled_effects_root()
                .file_name()
                .and_then(|value| value.to_str()),
            Some("effects")
        );
    }

    #[test]
    fn resolve_html_source_path_accepts_existing_absolute() {
        let dir = tempdir().expect("tempdir should create");
        let html_path = dir.path().join("effect.html");
        std::fs::write(&html_path, "<html><body>ok</body></html>").expect("write should work");

        let resolved = resolve_html_source_path(&html_path).expect("absolute path should resolve");
        assert_eq!(resolved, html_path);
    }

    #[test]
    fn resolve_html_source_path_rejects_missing_file() {
        let missing = Path::new("this/path/does/not/exist.html");
        let error = resolve_html_source_path(missing).expect_err("missing path should fail");
        assert!(
            error
                .to_string()
                .contains("could not resolve HTML effect source")
        );
    }
}
