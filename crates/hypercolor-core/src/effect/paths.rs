//! Shared effect source path helpers.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::config::paths::data_dir;

/// Return the installed bundled effects directory.
///
/// Resolution order:
/// 1. `$XDG_DATA_HOME/hypercolor/effects/bundled/` (installed location)
/// 2. Repository `effects/` directory (development fallback via `CARGO_MANIFEST_DIR`)
#[must_use]
pub fn bundled_effects_root() -> PathBuf {
    let installed = data_dir().join("effects").join("bundled");
    if installed.is_dir() {
        return installed;
    }

    // Development fallback — resolves to repo root effects/ at compile time
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../effects")
}

/// Return the user effects directory.
///
/// Defaults to `$XDG_DATA_HOME/hypercolor/effects/user/`
/// (typically `~/.local/share/hypercolor/effects/user/`).
#[must_use]
pub fn user_effects_dir() -> PathBuf {
    data_dir().join("effects").join("user")
}

/// Resolve an HTML source path to an existing path on disk.
///
/// Accepts either:
/// - absolute file paths
/// - paths relative to bundled `effects/`
/// - paths relative to user effects directory
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

    let mut candidates = vec![
        bundled_effects_root().join(path),
        user_effects_dir().join(path),
    ];
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
        "could not resolve HTML effect source '{}'; searched bundled, user, and current directories",
        path.display()
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{bundled_effects_root, resolve_html_source_path, user_effects_dir};

    #[test]
    fn bundled_effects_root_returns_valid_path() {
        let root = bundled_effects_root();
        // In dev, falls back to repo effects/; in prod, uses XDG data dir
        let name = root.file_name().and_then(|v| v.to_str());
        assert!(
            name == Some("effects") || name == Some("bundled"),
            "expected 'effects' or 'bundled', got {name:?}"
        );
    }

    #[test]
    fn user_effects_dir_ends_with_user() {
        let dir = user_effects_dir();
        assert_eq!(dir.file_name().and_then(|v| v.to_str()), Some("user"));
        assert!(dir.to_string_lossy().contains("hypercolor"));
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
