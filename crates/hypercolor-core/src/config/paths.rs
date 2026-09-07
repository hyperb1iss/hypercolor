//! Cross-platform path resolution for Hypercolor directories.
//!
//! Platform base directories (XDG on Linux, `AppData` on Windows, `Library`
//! on macOS) resolve through `hypercolor_platform_fs::user_dirs`; this module
//! appends `"hypercolor"` as the final component and layers the test
//! overrides on top.

use std::path::PathBuf;
use std::sync::{LazyLock, PoisonError, RwLock};

use hypercolor_platform_fs::user_dirs;

/// Application directory name, appended to all platform base paths.
const APP_DIR: &str = "hypercolor";

static DATA_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static CONFIG_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static STATE_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

/// Returns the platform-appropriate configuration directory.
///
/// - **Linux:** `$XDG_CONFIG_HOME/hypercolor/` (default `~/.config/hypercolor/`)
/// - **Windows:** `%APPDATA%\hypercolor\`
pub fn config_dir() -> PathBuf {
    if let Some(override_path) = CONFIG_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return override_path;
    }

    user_dirs::config_base_dir()
        .expect("config directory must be resolvable")
        .join(APP_DIR)
}

/// Override the resolved config directory.
///
/// This exists primarily to keep integration tests hermetic without mutating
/// process environment variables.
#[doc(hidden)]
pub fn set_config_dir_override(path: Option<PathBuf>) {
    let mut override_path = CONFIG_DIR_OVERRIDE
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    *override_path = path;
}

/// Returns the platform-appropriate data directory.
///
/// - **Linux:** `$XDG_DATA_HOME/hypercolor/` (default `~/.local/share/hypercolor/`)
/// - **Windows:** `%LOCALAPPDATA%\hypercolor\`
pub fn data_dir() -> PathBuf {
    if let Some(override_path) = DATA_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return override_path;
    }

    user_dirs::data_base_dir()
        .expect("data directory must be resolvable")
        .join(APP_DIR)
}

/// Override the resolved data directory.
///
/// This exists primarily to keep integration tests hermetic without mutating
/// process environment variables.
#[doc(hidden)]
pub fn set_data_dir_override(path: Option<PathBuf>) {
    let mut override_path = DATA_DIR_OVERRIDE
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    *override_path = path;
}

/// Returns the platform-appropriate machine-local state directory.
///
/// Linux uses `$XDG_STATE_HOME/hypercolor/` (default
/// `~/.local/state/hypercolor/`). Platforms without a distinct state home use
/// the local application-data directory.
pub fn state_dir() -> PathBuf {
    if let Some(override_path) = STATE_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return override_path;
    }

    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .expect("state directory must be resolvable")
        .join(APP_DIR)
}

/// Override the resolved machine-local state directory.
#[doc(hidden)]
pub fn set_state_dir_override(path: Option<PathBuf>) {
    let mut override_path = STATE_DIR_OVERRIDE
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    *override_path = path;
}

/// Returns the platform-appropriate cache directory.
///
/// - **Linux:** `$XDG_CACHE_HOME/hypercolor/` (default `~/.cache/hypercolor/`)
/// - **Windows:** `%LOCALAPPDATA%\hypercolor\cache\`
pub fn cache_dir() -> PathBuf {
    user_dirs::app_cache_dir(APP_DIR).expect("cache directory must be resolvable")
}

/// Returns the current user's home directory when the platform can resolve it.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Returns the directory for Servo runtime HTML shims.
///
/// Resolves to `<platform cache>/hypercolor/servo-runtime`, falling back to
/// the system temp directory when the platform cache root is unknown. The
/// Servo worker relied on that fallback before this accessor existed, so
/// the layout (including the macOS `~/Library/Caches` root rather than the
/// data-local `cache/` used by [`cache_dir`]) is preserved to keep existing
/// caches reachable.
pub fn servo_runtime_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
        .join("servo-runtime")
}

/// Returns the directory a user-facing export should land in.
///
/// Prefers the Desktop when it exists and falls back to the home directory.
pub fn user_output_dir() -> Option<PathBuf> {
    dirs::desktop_dir()
        .filter(|desktop| desktop.is_dir())
        .or_else(dirs::home_dir)
}

/// Returns the per-user launchd agent directory (`~/Library/LaunchAgents`).
///
/// The layout is macOS-specific, but the resolution is plain home-directory
/// arithmetic, so the accessor compiles everywhere; callers decide whether
/// the path is meaningful on the running host.
pub fn macos_launch_agents_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library").join("LaunchAgents"))
}

/// Returns the per-user launchd log directory (`~/Library/Logs/hypercolor`).
///
/// This is the directory the installed launchd plist points its
/// `StandardOutPath` and `StandardErrorPath` at, so service log readers
/// must resolve through here rather than rebuilding the path.
pub fn macos_user_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library").join("Logs").join(APP_DIR))
}

/// Returns the per-user application bundle directory (`~/Applications`).
pub fn macos_user_applications_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Applications"))
}
