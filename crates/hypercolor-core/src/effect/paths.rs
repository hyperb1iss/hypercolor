//! Shared effect source path helpers.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, PoisonError, RwLock};

use anyhow::{Result, bail};

use crate::config::paths::data_dir;

/// Environment variable naming the bundled effects directory outright.
pub const EFFECTS_DIR_ENV: &str = "HYPERCOLOR_EFFECTS_DIR";

static EFFECTS_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> =
    LazyLock::new(|| RwLock::new(None));

/// Point bundled effect resolution at an explicit directory for this process.
///
/// The app supervisor calls this with the directory it staged inside the
/// application bundle, which is why the daemon never needs a copy of the
/// catalog in the user's data directory.
pub fn set_bundled_effects_root(path: Option<PathBuf>) {
    *EFFECTS_DIR_OVERRIDE
        .write()
        .unwrap_or_else(PoisonError::into_inner) = path;
}

/// Return the bundled effects directory.
///
/// Bundled effects are read-only program assets that ship with the
/// application, so they are read where they are installed rather than unpacked
/// into the user's data directory. Only [`user_effects_dir`] is mutable.
///
/// Resolution order:
/// 1. an explicit override set by [`set_bundled_effects_root`] (the `--effects-dir` flag)
/// 2. `$HYPERCOLOR_EFFECTS_DIR`
/// 3. the install layouts next to the running executable
///
/// The final candidate is returned even when it does not exist, so failures
/// name the directory the daemon actually expects.
#[must_use]
pub fn bundled_effects_root() -> PathBuf {
    if let Some(override_path) = EFFECTS_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return override_path;
    }

    if let Some(from_env) = std::env::var_os(EFFECTS_DIR_ENV) {
        return PathBuf::from(from_env);
    }

    let candidates = installed_effects_candidates();
    candidates
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .or_else(|| candidates.into_iter().next())
        .unwrap_or_else(|| PathBuf::from("effects"))
}

/// Bundled effect directories for the layouts our packaging produces.
fn installed_effects_candidates() -> Vec<PathBuf> {
    let Ok(current_exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(exe_dir) = current_exe.parent() else {
        return Vec::new();
    };

    let mut candidates = vec![exe_dir.join("effects").join("bundled")];

    // Tarball and distro layout: <prefix>/bin/<exe>, <prefix>/share/hypercolor/effects/bundled
    if let Some(prefix) = exe_dir.parent() {
        candidates.push(
            prefix
                .join("share")
                .join("hypercolor")
                .join("effects")
                .join("bundled"),
        );
    }

    // macOS bundle layout: <app>/Contents/MacOS/<exe>, <app>/Contents/Resources/effects/bundled
    if exe_dir.file_name().and_then(|name| name.to_str()) == Some("MacOS")
        && let Some(contents) = exe_dir.parent()
    {
        candidates.push(contents.join("Resources").join("effects").join("bundled"));
    }

    candidates
}

/// Return the curated screenshot override directory.
///
/// Unlike the bundled catalog these are user-authored overrides that win over
/// the cover an effect ships inline, so they belong in the data directory
/// alongside the user's own effects.
///
/// Layout is `<root>/<slug>/<variant>.webp` — e.g.
/// `color-wave/default.webp`, `color-wave/silk-sweep.webp`.
#[must_use]
pub fn bundled_screenshots_root() -> PathBuf {
    data_dir()
        .join("effects")
        .join("screenshots")
        .join("curated")
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
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::{
        EFFECTS_DIR_ENV, bundled_effects_root, bundled_screenshots_root, resolve_html_source_path,
        user_effects_dir,
    };

    #[test]
    fn bundled_effects_root_uses_configured_or_installed_path() {
        let root = bundled_effects_root();
        if let Some(configured) = std::env::var_os(EFFECTS_DIR_ENV) {
            assert_eq!(root, PathBuf::from(configured));
            return;
        }

        let name = root.file_name().and_then(|v| v.to_str());
        assert!(
            name == Some("effects") || name == Some("bundled"),
            "expected 'effects' or 'bundled', got {name:?}"
        );
    }

    #[test]
    fn bundled_screenshots_root_ends_with_curated() {
        let root = bundled_screenshots_root();
        let name = root.file_name().and_then(|v| v.to_str());
        assert_eq!(name, Some("curated"));
        assert!(root.to_string_lossy().contains("screenshots"));
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
