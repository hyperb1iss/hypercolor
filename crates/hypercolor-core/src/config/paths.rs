//! Cross-platform path resolution for Hypercolor directories.
//!
//! Platform base directories (XDG on Linux, `AppData` on Windows, `Library`
//! on macOS) resolve through `hypercolor_platform_fs::user_dirs`; this module
//! appends `"hypercolor"` as the final component and layers the test
//! overrides on top.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError, RwLock};

use hypercolor_platform_fs::user_dirs;

/// Application directory name, appended to all platform base paths.
const APP_DIR: &str = "hypercolor";

static DATA_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static CONFIG_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static STATE_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static CACHE_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));
static RUNTIME_DIR_OVERRIDE: LazyLock<RwLock<Option<PathBuf>>> =
    LazyLock::new(|| RwLock::new(None));
/// Serializes fixtures that claim the process-wide directory overrides.
static USER_DIRS_CLAIM: Mutex<()> = Mutex::new(());

fn read_override(slot: &RwLock<Option<PathBuf>>) -> Option<PathBuf> {
    slot.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn write_override(slot: &RwLock<Option<PathBuf>>, path: Option<PathBuf>) {
    *slot.write().unwrap_or_else(PoisonError::into_inner) = path;
}

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
///
/// A process that relocated its data directory without relocating its state
/// directory gets `<data>/state`. Only fixtures relocate the data directory,
/// and the state directory holds the live daemon's runtime session, audit
/// trail, and history, so it must never fall through to the real user's.
pub fn state_dir() -> PathBuf {
    if let Some(override_path) = STATE_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return override_path;
    }

    if let Some(data_override) = DATA_DIR_OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return data_override.join("state");
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
    if let Some(override_path) = read_override(&CACHE_DIR_OVERRIDE) {
        return override_path;
    }

    user_dirs::app_cache_dir(APP_DIR).expect("cache directory must be resolvable")
}

/// Returns the per-user runtime directory that host IPC sockets live under.
///
/// Linux sessions publish it as `$XDG_RUNTIME_DIR`; services such as
/// blocksd put their sockets beneath it. Returns `None` when the session
/// has no runtime directory.
pub fn runtime_dir() -> Option<PathBuf> {
    read_override(&RUNTIME_DIR_OVERRIDE)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// Where a fixture's user directories live.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirPaths {
    /// Data directory.
    pub data: PathBuf,
    /// Config directory.
    pub config: PathBuf,
    /// Machine-local state directory.
    pub state: PathBuf,
    /// Cache directory.
    pub cache: PathBuf,
    /// Session runtime directory.
    pub runtime: PathBuf,
}

impl UserDirPaths {
    /// One subdirectory of `root` per user directory.
    #[must_use]
    pub fn under(root: &Path) -> Self {
        Self {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("run"),
        }
    }
}

/// A fixture's claim on every Hypercolor user directory.
///
/// Points data, config, state, cache, and runtime resolution at fixture
/// paths for as long as the claim lives, and clears the overrides when it
/// drops. The overrides are process-wide, so claims serialize: a second
/// claim in the same process waits for the first.
#[doc(hidden)]
pub struct IsolatedUserDirs {
    paths: UserDirPaths,
    _claim: MutexGuard<'static, ()>,
}

impl IsolatedUserDirs {
    /// Claim the user directories for a fixture rooted at `root`.
    ///
    /// The directories themselves are created lazily by whatever writes
    /// them; `root` only needs to outlive the claim.
    #[must_use]
    pub fn claim(root: &Path) -> Self {
        Self::claim_paths(UserDirPaths::under(root))
    }

    /// Claim the user directories at exactly `paths`.
    #[must_use]
    pub fn claim_paths(paths: UserDirPaths) -> Self {
        let claim = USER_DIRS_CLAIM
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        write_override(&DATA_DIR_OVERRIDE, Some(paths.data.clone()));
        write_override(&CONFIG_DIR_OVERRIDE, Some(paths.config.clone()));
        write_override(&STATE_DIR_OVERRIDE, Some(paths.state.clone()));
        write_override(&CACHE_DIR_OVERRIDE, Some(paths.cache.clone()));
        write_override(&RUNTIME_DIR_OVERRIDE, Some(paths.runtime.clone()));
        Self {
            paths,
            _claim: claim,
        }
    }

    /// The claimed directories.
    #[must_use]
    pub const fn paths(&self) -> &UserDirPaths {
        &self.paths
    }
}

impl Drop for IsolatedUserDirs {
    fn drop(&mut self) {
        for slot in [
            &*DATA_DIR_OVERRIDE,
            &*CONFIG_DIR_OVERRIDE,
            &*STATE_DIR_OVERRIDE,
            &*CACHE_DIR_OVERRIDE,
            &*RUNTIME_DIR_OVERRIDE,
        ] {
            write_override(slot, None);
        }
    }
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
    if let Some(cache_override) = read_override(&CACHE_DIR_OVERRIDE) {
        return cache_override.join("servo-runtime");
    }
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
