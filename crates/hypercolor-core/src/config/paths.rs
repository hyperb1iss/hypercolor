//! Cross-platform path resolution for Hypercolor directories.
//!
//! Uses the `dirs` crate for platform-aware XDG (Linux) and `AppData` (Windows)
//! path resolution. All paths append `"hypercolor"` as the final component.

use std::path::PathBuf;

/// Application directory name, appended to all platform base paths.
const APP_DIR: &str = "hypercolor";

/// Returns the platform-appropriate configuration directory.
///
/// - **Linux:** `$XDG_CONFIG_HOME/hypercolor/` (default `~/.config/hypercolor/`)
/// - **Windows:** `%APPDATA%\hypercolor\`
pub fn config_dir() -> PathBuf {
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

/// Returns the platform-appropriate data directory.
///
/// - **Linux:** `$XDG_DATA_HOME/hypercolor/` (default `~/.local/share/hypercolor/`)
/// - **Windows:** `%LOCALAPPDATA%\hypercolor\`
pub fn data_dir() -> PathBuf {
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
