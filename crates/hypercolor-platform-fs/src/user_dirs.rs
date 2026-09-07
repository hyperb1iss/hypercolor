//! Per-user base directory resolution.
//!
//! Linux honours the XDG base-directory variables verbatim (an explicit
//! `$XDG_*_HOME` wins, the conventional dotfolder under `$HOME` is the
//! fallback). Other platforms resolve through the operating system's own
//! user-directory conventions. Callers append their application segment;
//! the platform split stays here so neutral crates never branch on the
//! operating system.

use std::path::PathBuf;

/// Base directory for per-user configuration.
///
/// - **Linux:** `$XDG_CONFIG_HOME` (default `~/.config`)
/// - **Windows:** `%APPDATA%`
/// - **macOS:** `~/Library/Application Support`
///
/// Returns `None` when the platform cannot resolve a home or config root.
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        xdg_or_home("XDG_CONFIG_HOME", ".config")
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::config_dir()
    }
}

/// Base directory for per-user data.
///
/// - **Linux:** `$XDG_DATA_HOME` (default `~/.local/share`)
/// - **Windows:** `%LOCALAPPDATA%`
/// - **macOS:** `~/Library/Application Support`
///
/// Returns `None` when the platform cannot resolve a home or data root.
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        xdg_or_home("XDG_DATA_HOME", ".local/share")
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::data_local_dir()
    }
}

/// Application cache directory for `app`.
///
/// - **Linux:** `$XDG_CACHE_HOME/<app>` (default `~/.cache/<app>`)
/// - **Windows:** `%LOCALAPPDATA%\<app>\cache`
/// - **macOS:** `~/Library/Caches/<app>/cache`
///
/// The non-Linux layout nests a `cache` segment under the application
/// directory; that layout predates this module and is preserved so existing
/// caches stay reachable. Returns `None` when the platform cannot resolve a
/// cache root.
#[must_use]
pub fn app_cache_dir(app: &str) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        xdg_or_home("XDG_CACHE_HOME", ".cache").map(|base| base.join(app))
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::cache_dir().map(|base| base.join(app).join("cache"))
    }
}

#[cfg(target_os = "linux")]
fn xdg_or_home(variable: &str, home_relative: &str) -> Option<PathBuf> {
    match std::env::var(variable) {
        Ok(value) => Some(PathBuf::from(value)),
        Err(_) => dirs::home_dir().map(|home| home.join(home_relative)),
    }
}
