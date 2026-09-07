//! Host-side OpenRGB integration for Hypercolor.
//!
//! This crate answers the questions the desktop app and the CLI ask before
//! they lean on the OpenRGB fallback bridge: is OpenRGB installed, is its
//! SDK server reachable, how do you install it here, which permissions are
//! missing, and how do we launch a headless server that leaves natively
//! supported hardware alone.
//!
//! It follows the platform-crate pattern: every type is neutral and
//! serializable on every target, and only the functions that touch the OS
//! are gated behind `cfg(target_os)` inside this crate.
//!
//! Anything that spawns a process or opens a socket is `async` on tokio,
//! matching the SDK client. Filesystem inspection and the pure builders
//! (hints, partition, launch spec) are synchronous.

mod detect;
mod error;
mod hints;
mod permissions;
mod probe;
mod types;
pub use detect::{
    FLATPAK_APP_ID, SUBPROCESS_TIMEOUT, appimage_search_dirs, classify_binary, detect_binary,
    executable_names, find_appimage_in, find_in_path, find_native_binary, flatpak_app_version,
    is_executable_file, known_locations, parse_flatpak_info_version, parse_version_output,
    read_version,
};
pub use error::{HostError, Result};
pub use hints::{RELEASES_URL, detect_package_managers, install_hints, install_hints_for};
pub use permissions::{
    CHECK_HIDRAW_NODES, CHECK_I2C_DEV_MODULE, CHECK_I2C_NODES, CHECK_UDEV_RULES, UDEV_RULES_PATHS,
    linux_permission_checks_at, permission_checks, udev_rules_remedy,
};
pub use probe::{DEFAULT_SERVER_PORT, PROBE_CLIENT_NAME, probe_server};
pub use types::{
    BinaryKind, InstallHint, InstallMethod, ManagedConfigDir, OpenRgbBinary, PermissionCheck,
    Platform, ProcessSpec, ServerProbe,
};
