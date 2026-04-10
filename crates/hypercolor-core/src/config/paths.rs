//! Cross-platform path resolution for Hypercolor directories.
//!
//! Uses the `dirs` crate for platform-aware XDG (Linux) and `AppData` (Windows)
//! path resolution. All paths append `"hypercolor"` as the final component.

use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

/// Application directory name, appended to all platform base paths.
const APP_DIR: &str = "hypercolor";

static DATA_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static CONFIG_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

/// Returns the platform-appropriate configuration directory.
///
/// - **Linux:** `$XDG_CONFIG_HOME/hypercolor/` (default `~/.config/hypercolor/`)
/// - **Windows:** `%APPDATA%\hypercolor\`
pub fn config_dir() -> PathBuf {
    if let Some(override_path) = CONFIG_DIR_OVERRIDE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return override_path;
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .map_or_else(
                |_| dirs::home_dir().expect("HOME must be set").join(".config"),
                PathBuf::from,
            )
            .join(APP_DIR)
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::config_dir()
            .expect("config directory must be resolvable")
            .join(APP_DIR)
    }
}

/// Override the resolved config directory.
///
/// This exists primarily to keep integration tests hermetic without mutating
/// process environment variables.
#[doc(hidden)]
pub fn set_config_dir_override(path: Option<PathBuf>) {
    let mut override_path = CONFIG_DIR_OVERRIDE
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *override_path = path;
}

/// Returns the platform-appropriate data directory.
///
/// - **Linux:** `$XDG_DATA_HOME/hypercolor/` (default `~/.local/share/hypercolor/`)
/// - **Windows:** `%LOCALAPPDATA%\hypercolor\`
pub fn data_dir() -> PathBuf {
    if let Some(override_path) = DATA_DIR_OVERRIDE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return override_path;
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_DATA_HOME")
            .map_or_else(
                |_| {
                    dirs::home_dir()
                        .expect("HOME must be set")
                        .join(".local/share")
                },
                PathBuf::from,
            )
            .join(APP_DIR)
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::data_local_dir()
            .expect("data directory must be resolvable")
            .join(APP_DIR)
    }
}

/// Override the resolved data directory.
///
/// This exists primarily to keep integration tests hermetic without mutating
/// process environment variables.
#[doc(hidden)]
pub fn set_data_dir_override(path: Option<PathBuf>) {
    let mut override_path = DATA_DIR_OVERRIDE.write().unwrap_or_else(|e| e.into_inner());
    *override_path = path;
}

/// Returns the platform-appropriate cache directory.
///
/// - **Linux:** `$XDG_CACHE_HOME/hypercolor/` (default `~/.cache/hypercolor/`)
/// - **Windows:** `%LOCALAPPDATA%\hypercolor\cache\`
pub fn cache_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CACHE_HOME")
            .map_or_else(
                |_| dirs::home_dir().expect("HOME must be set").join(".cache"),
                PathBuf::from,
            )
            .join(APP_DIR)
    }

    #[cfg(not(target_os = "linux"))]
    {
        dirs::cache_dir()
            .expect("cache directory must be resolvable")
            .join(APP_DIR)
            .join("cache")
    }
}
