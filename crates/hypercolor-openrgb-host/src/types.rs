//! Neutral, serializable vocabulary shared by every consumer of this crate.
//!
//! Every type here derives `Serialize` and `Deserialize` so the desktop app,
//! the CLI, and the daemon can pass values across Tauri commands and REST
//! responses without hand-mirrored copies.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How an OpenRGB binary is packaged on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryKind {
    /// A regular executable (distro package, MSI, app bundle, self-built).
    Native,
    /// The Flathub build, launched through `flatpak run org.openrgb.OpenRGB`.
    Flatpak,
    /// A portable AppImage file.
    AppImage,
}

/// An OpenRGB installation discovered on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRgbBinary {
    /// The executable that launches OpenRGB.
    ///
    /// For [`BinaryKind::Flatpak`] this is the `flatpak` launcher itself; the
    /// application id travels in the process arguments instead.
    pub path: PathBuf,
    /// Packaging of the discovered binary.
    pub kind: BinaryKind,
    /// The version OpenRGB reported, when it could be read within the timeout.
    #[serde(default)]
    pub version: Option<String>,
}

/// Result of probing an OpenRGB SDK server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProbe {
    /// Whether the SDK handshake completed.
    pub reachable: bool,
    /// The negotiated SDK protocol version when reachable.
    #[serde(default)]
    pub protocol_version: Option<u32>,
    /// The controller count the server reported when reachable.
    #[serde(default)]
    pub controller_count: Option<u32>,
    /// The failure description when the probe did not complete.
    #[serde(default)]
    pub error: Option<String>,
}

/// Host operating system an install hint targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Any Linux distribution.
    Linux,
    /// Windows 10 or newer.
    Windows,
    /// macOS on Intel or Apple Silicon.
    Macos,
}

impl Platform {
    /// The platform this binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Self::Linux
        }
    }
}

/// Package managers and launchers this crate can recommend or detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethod {
    /// Arch Linux `pacman` (`extra/openrgb`).
    Pacman,
    /// Debian and Ubuntu `apt`.
    Apt,
    /// Fedora `dnf`.
    Dnf,
    /// openSUSE `zypper`.
    Zypper,
    /// Flathub through `flatpak`.
    Flatpak,
    /// Windows Package Manager.
    Winget,
    /// Manual download from the OpenRGB releases page.
    DirectDownload,
}

/// One way to install OpenRGB on a platform, with the exact command to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallHint {
    /// Platform the hint applies to.
    pub platform: Platform,
    /// Package manager or download channel.
    pub method: InstallMethod,
    /// A command to paste into a shell, or a URL for direct downloads.
    pub command: String,
    /// Follow-up guidance: permissions, drivers, and platform limits.
    #[serde(default)]
    pub note: String,
}

/// One host permission or driver prerequisite for OpenRGB device access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionCheck {
    /// Stable machine-readable identifier (`udev_rules`, `i2c_dev_module`, ...).
    pub id: String,
    /// Whether the prerequisite is satisfied.
    pub ok: bool,
    /// What was inspected and what was found.
    pub detail: String,
    /// The exact command that fixes a failing check, when one exists.
    #[serde(default)]
    pub remedy: Option<String>,
}

/// A fully resolved process launch: program, arguments, environment, cwd.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSpec {
    /// The program to execute.
    pub program: PathBuf,
    /// Arguments in order, without the program name.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables layered over the parent environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory, or the parent's when `None`.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// The OpenRGB configuration directory Hypercolor owns for its headless server.
///
/// OpenRGB reads `OpenRGB.json` from the directory passed with `--config`
/// and writes `sizes.ors`, profiles, logs, and plugins beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedConfigDir {
    /// The directory passed to OpenRGB's `--config` flag.
    pub root: PathBuf,
}

impl ManagedConfigDir {
    /// OpenRGB's main configuration file name inside the config directory.
    pub const CONFIG_FILE_NAME: &'static str = "OpenRGB.json";

    /// The `OpenRGB.json` path inside this directory.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(Self::CONFIG_FILE_NAME)
    }
}

impl AsRef<std::path::Path> for ManagedConfigDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.root
    }
}
