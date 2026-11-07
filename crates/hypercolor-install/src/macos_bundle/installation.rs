//! Where a managed app bundle installation lives, and the record that says
//! one exists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The record's file in the state root, the last thing adoption writes.
pub const MACOS_BUNDLE_INSTALLATION_FILE: &str = "installation.json";
/// The stable launcher in the release root, outside every unit.
pub const MACOS_BUNDLE_LAUNCHER_NAME: &str = "launcher";

const INSTALLATION_SCHEMA_VERSION: u32 = 1;
const LAUNCHER_CONTRACT: u32 = 1;
const RELEASES_DIRECTORY: &str = "releases";
const UPDATE_DIRECTORY: &str = "update";

/// The two roots of one account's installation, siblings below its product
/// directory in Application Support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleRoots {
    /// Immutable units, the `active` pointer and the launcher.
    pub release_root: PathBuf,
    /// The journal, the lock and the update records.
    pub state_root: PathBuf,
}

impl MacosBundleRoots {
    /// The roots of the installation whose update state root is `state_root`.
    #[must_use]
    pub fn of_state_root(state_root: &Path) -> Option<Self> {
        (state_root.file_name()? == UPDATE_DIRECTORY).then(|| Self {
            release_root: state_root
                .parent()
                .unwrap_or(state_root)
                .join(RELEASES_DIRECTORY),
            state_root: state_root.to_path_buf(),
        })
    }

    /// The roots of the installation whose launcher is `executable`.
    #[must_use]
    pub fn of_launcher(executable: &Path) -> Option<Self> {
        if executable.file_name()? != MACOS_BUNDLE_LAUNCHER_NAME {
            return None;
        }
        let release_root = executable.parent()?;
        if release_root.file_name()? != RELEASES_DIRECTORY {
            return None;
        }
        Some(Self {
            state_root: release_root.parent()?.join(UPDATE_DIRECTORY),
            release_root: release_root.to_path_buf(),
        })
    }

    /// The stable launcher.
    #[must_use]
    pub fn launcher(&self) -> PathBuf {
        self.release_root.join(MACOS_BUNDLE_LAUNCHER_NAME)
    }
}

/// What the installer recorded when it finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosBundleInstallation {
    schema_version: u32,
    /// The identity every update plan binds to.
    pub installation_id: Uuid,
    /// The bundle players launch.
    pub installed_bundle: PathBuf,
    pub release_root: PathBuf,
    pub state_root: PathBuf,
    /// The daemon's data directory.
    pub data_root: PathBuf,
    launcher_contract: u32,
}

impl MacosBundleInstallation {
    pub(super) fn new(
        installation_id: Uuid,
        installed_bundle: PathBuf,
        roots: &MacosBundleRoots,
        data_root: PathBuf,
    ) -> Self {
        Self {
            schema_version: INSTALLATION_SCHEMA_VERSION,
            installation_id,
            installed_bundle,
            release_root: roots.release_root.clone(),
            state_root: roots.state_root.clone(),
            data_root,
            launcher_contract: LAUNCHER_CONTRACT,
        }
    }

    /// Decode the record.
    ///
    /// # Errors
    /// Returns the reason the record is not one this build reads.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let record: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if record.schema_version != INSTALLATION_SCHEMA_VERSION
            || record.launcher_contract != LAUNCHER_CONTRACT
            || record.installation_id.is_nil()
        {
            return Err(format!(
                "unsupported installation schema {} with launcher contract {}",
                record.schema_version, record.launcher_contract
            ));
        }
        Ok(record)
    }

    pub(super) fn encode(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }
}
