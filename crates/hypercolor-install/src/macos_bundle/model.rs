use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::super::{InstallPlatformError, UnitId};

/// The name of the installed bundle and of every staged copy of it.
pub const MACOS_BUNDLE_NAME: &str = "Hypercolor.app";
/// The staging root beside the installed bundle, on its filesystem.
pub const MACOS_BUNDLE_STAGING_ROOT_NAME: &str = ".hypercolor-update";
/// Free space a materialization keeps beyond the candidate's installed bytes.
pub const MACOS_BUNDLE_STAGING_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
/// Suffix of a materialization that has not been verified and published yet.
pub const MACOS_BUNDLE_PARTIAL_SUFFIX: &str = ".partial";
/// Suffix of a staged bundle that is being removed.
pub const MACOS_BUNDLE_DISCARD_SUFFIX: &str = ".discard";

const DAEMON_IN_BUNDLE: &str = "Contents/MacOS/hypercolor-daemon";
const MAX_CONFIG_PATH_BYTES: usize = 1024;

/// Where an app bundle installation lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleConfig {
    /// The bundle players launch, normally `/Applications/Hypercolor.app`.
    pub installed_bundle: PathBuf,
    /// The release root, holding the `active` pointer and `units/`.
    pub release_root: PathBuf,
}

impl MacosBundleConfig {
    /// Check that both roots are canonical absolute UTF-8 paths and that the
    /// installed bundle is named [`MACOS_BUNDLE_NAME`].
    ///
    /// # Errors
    /// Returns an error naming the first path that is not exact.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        for path in [&self.installed_bundle, &self.release_root] {
            let text = path
                .to_str()
                .ok_or_else(|| error("macOS bundle paths must be exact UTF-8"))?;
            if text.len() > MAX_CONFIG_PATH_BYTES
                || !path.is_absolute()
                || path.components().count() < 2
                || path.components().any(|component| {
                    !matches!(component, Component::RootDir | Component::Normal(_))
                })
                || text.ends_with('/')
            {
                return Err(error(format!(
                    "macOS bundle path {text} is not a bounded canonical absolute path"
                )));
            }
        }
        if self.installed_bundle.file_name() != Some(std::ffi::OsStr::new(MACOS_BUNDLE_NAME)) {
            return Err(error(format!(
                "the installed macOS bundle must be named {MACOS_BUNDLE_NAME}"
            )));
        }
        if self.release_root.starts_with(&self.installed_bundle)
            || self.release_root.starts_with(self.staging_root())
        {
            return Err(error(
                "the macOS release root cannot live inside the installed bundle or its staging root",
            ));
        }
        Ok(())
    }

    /// The directory holding the installed bundle.
    #[must_use]
    pub fn applications(&self) -> &Path {
        self.installed_bundle.parent().unwrap_or(Path::new("/"))
    }

    /// `<applications>/.hypercolor-update`.
    #[must_use]
    pub fn staging_root(&self) -> PathBuf {
        self.applications().join(MACOS_BUNDLE_STAGING_ROOT_NAME)
    }

    /// The staged copy one transaction swaps with the installed bundle.
    #[must_use]
    pub fn staged_bundle(&self, name: &str) -> PathBuf {
        self.staging_root().join(name).join(MACOS_BUNDLE_NAME)
    }

    /// The daemon the service runs from the installed bundle.
    #[must_use]
    pub fn installed_daemon(&self) -> PathBuf {
        self.installed_bundle.join(DAEMON_IN_BUNDLE)
    }

    /// The daemon inside one transaction's staged copy.
    #[must_use]
    pub fn staged_daemon(&self, name: &str) -> PathBuf {
        self.staged_bundle(name).join(DAEMON_IN_BUNDLE)
    }
}

/// What identifies one materialized bundle: the cdhash of each executable
/// its signing policy names.
///
/// The app's cdhash seals the bundle's resources and nested code through
/// its `CodeResources`, and the daemon's identifies the running service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosBundleIdentity {
    pub app: String,
    pub daemon: String,
    pub cli: String,
}

impl MacosBundleIdentity {
    /// # Errors
    /// Returns an error when any cdhash is not 40 lowercase hex digits.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        if [&self.app, &self.daemon, &self.cli]
            .into_iter()
            .all(|cdhash| is_cdhash(cdhash))
        {
            Ok(())
        } else {
            Err(error("macOS bundle identity holds a malformed cdhash"))
        }
    }
}

/// What one bundle location holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosBundleSlot {
    Absent,
    Bundle(MacosBundleIdentity),
    /// Something is there, but it is not a readable bundle.
    Unrecognized(String),
}

impl MacosBundleSlot {
    #[must_use]
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Whether this slot holds exactly `expected`, or nothing when `None`.
    #[must_use]
    pub fn holds(&self, expected: Option<&MacosBundleIdentity>) -> bool {
        match (self, expected) {
            (Self::Absent, None) => true,
            (Self::Bundle(actual), Some(expected)) => actual == expected,
            _ => false,
        }
    }
}

/// One observation of the installed bundle and the staging root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleObservation {
    pub installed: MacosBundleSlot,
    /// Every digest-named directory in the staging root, with what its
    /// `Hypercolor.app` holds.
    pub staged: BTreeMap<String, MacosBundleSlot>,
    /// Digest-named leftovers of an interrupted materialization
    /// ([`MACOS_BUNDLE_PARTIAL_SUFFIX`]) or removal
    /// ([`MACOS_BUNDLE_DISCARD_SUFFIX`]). Their contents never count.
    pub interrupted: BTreeSet<String>,
}

impl MacosBundleObservation {
    /// What the staged copy named `name` holds.
    #[must_use]
    pub fn staged_slot(&self, name: &str) -> &MacosBundleSlot {
        self.staged.get(name).unwrap_or(&MacosBundleSlot::Absent)
    }
}

/// One member of a bundle unit's `Hypercolor.app`, relative to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleMember {
    pub path: String,
    pub kind: MacosBundleMemberKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosBundleMemberKind {
    Directory {
        mode: u32,
    },
    File {
        mode: u32,
        size: u64,
        sha256: String,
    },
}

/// A retained app bundle unit, as its own manifest and executables state it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleUnit {
    id: UnitId,
    version: String,
    installed_bytes: u64,
    identity: MacosBundleIdentity,
    daemon_sha256: String,
    bundle_mode: u32,
    members: Vec<MacosBundleMember>,
}

impl MacosBundleUnit {
    pub(crate) fn new(
        id: UnitId,
        version: String,
        installed_bytes: u64,
        identity: MacosBundleIdentity,
        daemon_sha256: String,
        bundle_mode: u32,
        members: Vec<MacosBundleMember>,
    ) -> Self {
        Self {
            id,
            version,
            installed_bytes,
            identity,
            daemon_sha256,
            bundle_mode,
            members,
        }
    }

    #[must_use]
    pub fn id(&self) -> &UnitId {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// `manifest.json` plus every regular file of the unit.
    #[must_use]
    pub fn installed_bytes(&self) -> u64 {
        self.installed_bytes
    }

    #[must_use]
    pub fn identity(&self) -> &MacosBundleIdentity {
        &self.identity
    }

    /// SHA-256 of the daemon, as the unit manifest records it.
    #[must_use]
    pub fn daemon_sha256(&self) -> &str {
        &self.daemon_sha256
    }

    /// The mode of `Hypercolor.app` itself.
    #[must_use]
    pub fn bundle_mode(&self) -> u32 {
        self.bundle_mode
    }

    /// Every member below `Hypercolor.app`, parents before children.
    #[must_use]
    pub fn members(&self) -> &[MacosBundleMember] {
        &self.members
    }
}

/// How a materialization produced its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MacosBundleMaterialization {
    pub cloned_files: u64,
    pub copied_files: u64,
}

/// What the daemon's service manager reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleServiceObservation {
    /// The daemon agent's definition is installed.
    pub registered: bool,
    /// The agent may start on its own (it is not disabled).
    pub enabled: bool,
    pub running: Option<MacosBundleRunningDaemon>,
}

/// A running daemon, as the operating system identifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosBundleRunningDaemon {
    pub pid: u32,
    /// The process's executable path (`proc_pidpath`).
    pub executable: PathBuf,
    /// The running code's cdhash.
    pub cdhash: String,
}

impl MacosBundleRunningDaemon {
    /// # Errors
    /// Returns an error for a zero process ID or a malformed cdhash.
    pub fn validate(&self) -> Result<(), InstallPlatformError> {
        if self.pid == 0 || !is_cdhash(&self.cdhash) {
            return Err(error("macOS running daemon identity is malformed"));
        }
        Ok(())
    }
}

pub(super) fn is_cdhash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn error(detail: impl Into<String>) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}
