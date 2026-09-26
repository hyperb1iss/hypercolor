//! Recorded per-user installation topology, independent of ambient XDG changes.

use std::path::{Component, Path, PathBuf};

use hypercolor_platform_fs::PublicDirectoryAuthority;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::ownership::{DirectoryRefusal, DirectoryRole, OwnershipPolicy};
use super::super::{InstallLock, InstallStoreError};

const LOCATION_SCHEMA: u32 = 2;
const LAUNCHER_CONTRACT: u32 = 1;
const MAX_LOCATION_BYTES: usize = 32 * 1024;
/// HOME bound. The historical root, launcher and layout all live beneath it.
const MAX_HOME_BYTES: usize = 256;
/// Recorded root bound. Every transaction record embeds each root several
/// times, so these bounds keep the longest record inside its journal budget.
const MAX_ROOT_BYTES: usize = 512;

/// Installer-recorded roots and owner identity for a managed Linux installation.
///
/// Parsing verifies the topology contract. Filesystem ownership and retained
/// directory relationships must also be validated before using these paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinuxInstallLocation {
    schema_version: u32,
    kind: LocationKind,
    installation_id: Uuid,
    uid: u32,
    data_root: PathBuf,
    state_root: PathBuf,
    release_root: PathBuf,
    config_root: PathBuf,
    service_name: String,
    launcher_contract: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LocationKind {
    ManagedLocation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocation {
    schema_version: u32,
    kind: LocationKind,
    installation_id: Uuid,
    uid: u32,
    data_root: PathBuf,
    state_root: PathBuf,
    release_root: PathBuf,
    config_root: PathBuf,
    service_name: String,
    launcher_contract: u32,
}

/// Invalid recorded installation identity or root topology.
#[derive(Debug, thiserror::Error)]
pub enum InstallLocationError {
    #[error("installation location exceeds its byte bound")]
    TooLarge,
    #[error("invalid installation location: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("unsupported managed installation contract")]
    UnsupportedContract,
    #[error("installation root must be a bounded normalized absolute path: {}", .0.display())]
    InvalidPath(PathBuf),
    #[error("installation roots overlap protected or legacy installation state")]
    OverlappingRoots,
    #[error(
        "installation path {path} is longer than {limit} bytes, the most a transaction record can carry",
        path = .path.display()
    )]
    PathTooLong { path: PathBuf, limit: usize },
    #[error("installation directory authority could not be retained: {0}")]
    Authority(#[from] std::io::Error),
    #[error("installation lock cannot authorize the recorded roots: {0}")]
    Store(#[from] InstallStoreError),
    #[error(
        "installation directory {path} is not writable only by its recorded owner: {refusal}",
        path = .0.display(),
        refusal = .1
    )]
    InvalidOwner(PathBuf, DirectoryRefusal),
}

impl LinuxInstallLocation {
    /// Retain existing roots through the caller's exclusive installation lock.
    ///
    /// Ownership, original ancestry and physical root relationships are checked
    /// before returning. This method creates no directories or locator files.
    ///
    /// # Errors
    /// Returns an error for missing, replaced, aliased or foreign-owned roots.
    pub fn retain_existing(
        &self,
        home: &Path,
        gate: &InstallLock,
    ) -> Result<RetainedLinuxInstallLocation, InstallLocationError> {
        self.validate(home)?;
        let legacy = home.join(".local/lib/hypercolor");
        let retained = RetainedLinuxInstallLocation {
            uid: self.uid,
            ownership: gate.ownership_policy().clone(),
            data: gate.open_public_directory(&self.data_root)?,
            state: gate.open_public_directory(&self.state_root)?,
            releases: gate.open_public_directory(&self.release_root)?,
            config: gate.open_public_directory(&self.config_root)?,
            legacy: gate.open_public_directory(&legacy)?,
            paths: [
                self.data_root.clone(),
                self.state_root.clone(),
                self.release_root.clone(),
                self.config_root.clone(),
                legacy,
            ],
        };
        retained.validate()?;
        Ok(retained)
    }
    /// Construct a fresh installation identity from explicit XDG base paths.
    ///
    /// Callers resolve absent environment values to their normal defaults before
    /// calling this method; recorded paths are never recomputed during recovery.
    ///
    /// # Errors
    /// Returns an error for unsafe paths or overlapping installation roots.
    pub fn new(
        home: &Path,
        data_base: &Path,
        state_base: &Path,
        config_base: &Path,
        uid: u32,
    ) -> Result<Self, InstallLocationError> {
        let data_root = data_base.join("hypercolor");
        let location = Self {
            schema_version: LOCATION_SCHEMA,
            kind: LocationKind::ManagedLocation,
            installation_id: Uuid::new_v4(),
            uid,
            release_root: data_root.join("releases"),
            data_root,
            state_root: state_base.join("hypercolor/update"),
            config_root: config_base.join("hypercolor"),
            service_name: "hypercolor.service".to_owned(),
            launcher_contract: LAUNCHER_CONTRACT,
        };
        location.validate(home)?;
        Ok(location)
    }

    /// Decode the fixed legacy-path locator without consulting the environment.
    ///
    /// # Errors
    /// Returns an error for unknown fields/contracts, oversized input or unsafe
    /// topology. A caller must never reinterpret a failed V2 parse as legacy V1.
    pub fn parse(bytes: &[u8], home: &Path) -> Result<Self, InstallLocationError> {
        if bytes.len() > MAX_LOCATION_BYTES {
            return Err(InstallLocationError::TooLarge);
        }
        let raw: RawLocation = serde_json::from_slice(bytes)?;
        let location = Self {
            schema_version: raw.schema_version,
            kind: raw.kind,
            installation_id: raw.installation_id,
            uid: raw.uid,
            data_root: raw.data_root,
            state_root: raw.state_root,
            release_root: raw.release_root,
            config_root: raw.config_root,
            service_name: raw.service_name,
            launcher_contract: raw.launcher_contract,
        };
        location.validate(home)?;
        Ok(location)
    }

    fn validate(&self, home: &Path) -> Result<(), InstallLocationError> {
        if self.schema_version != LOCATION_SCHEMA
            || self.launcher_contract != LAUNCHER_CONTRACT
            || self.service_name != "hypercolor.service"
            || self.installation_id.is_nil()
        {
            return Err(InstallLocationError::UnsupportedContract);
        }
        for root in [
            home,
            &self.data_root,
            &self.state_root,
            &self.release_root,
            &self.config_root,
        ] {
            validate_path(root)?;
        }
        for (path, limit) in [
            (home, MAX_HOME_BYTES),
            (self.data_root.as_path(), MAX_ROOT_BYTES),
            (self.state_root.as_path(), MAX_ROOT_BYTES),
            (self.release_root.as_path(), MAX_ROOT_BYTES),
            (self.config_root.as_path(), MAX_ROOT_BYTES),
        ] {
            if path.as_os_str().len() > limit {
                return Err(InstallLocationError::PathTooLong {
                    path: path.to_path_buf(),
                    limit,
                });
            }
        }
        let legacy = home.join(".local/lib/hypercolor");
        if self.release_root != self.data_root.join("releases")
            || overlaps(&self.state_root, &self.release_root)
            || overlaps(&self.state_root, &legacy)
            || overlaps(&self.release_root, &legacy)
            || self.config_root.starts_with(&self.release_root)
            || self.config_root.starts_with(&self.state_root)
            || self.data_root.starts_with(&self.state_root)
        {
            return Err(InstallLocationError::OverlappingRoots);
        }
        Ok(())
    }

    /// Stable installation identity, independent of a selected release.
    #[must_use]
    pub fn installation_id(&self) -> Uuid {
        self.installation_id
    }

    /// Expected filesystem owner, checked against retained directory metadata.
    #[must_use]
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// Container for mutable application data and the reserved releases subtree.
    #[must_use]
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// Root containing the sole managed transaction journal, lock and staging.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Root containing immutable units and the active pointer.
    #[must_use]
    pub fn release_root(&self) -> &Path {
        &self.release_root
    }

    /// Protected user configuration directory.
    #[must_use]
    pub fn config_root(&self) -> &Path {
        &self.config_root
    }
}

/// Retained roots whose filesystem identities govern a managed installation.
///
/// The caller keeps this authority alive while preparing the location handoff
/// and revalidates immediately before publishing the locator.
#[derive(Debug)]
pub struct RetainedLinuxInstallLocation {
    uid: u32,
    ownership: OwnershipPolicy,
    paths: [PathBuf; 5],
    data: PublicDirectoryAuthority,
    state: PublicDirectoryAuthority,
    releases: PublicDirectoryAuthority,
    config: PublicDirectoryAuthority,
    legacy: PublicDirectoryAuthority,
}

impl RetainedLinuxInstallLocation {
    /// Revalidate ownership, original paths and physical protected-root bounds.
    ///
    /// # Errors
    /// Returns an error when any retained relationship can no longer be proven.
    pub fn validate(&self) -> Result<(), InstallLocationError> {
        // The daemon creates and shares the data and configuration roots, so
        // they follow the ancestor rule; the installer owns every other root.
        for (root, path, role) in [
            (&self.data, &self.paths[0], DirectoryRole::Ancestor),
            (&self.state, &self.paths[1], DirectoryRole::InstallerOwned),
            (
                &self.releases,
                &self.paths[2],
                DirectoryRole::InstallerOwned,
            ),
            (&self.config, &self.paths[3], DirectoryRole::Ancestor),
            (&self.legacy, &self.paths[4], DirectoryRole::InstallerOwned),
        ] {
            let metadata = root.metadata()?;
            if metadata.owner_uid() != self.uid {
                return Err(InstallLocationError::InvalidOwner(
                    path.clone(),
                    DirectoryRefusal::RecordedOwnerMismatch {
                        recorded: self.uid,
                        actual: metadata.owner_uid(),
                    },
                ));
            }
            self.ownership
                .require_owner_only(root, metadata, role)
                .map_err(|refusal| InstallLocationError::InvalidOwner(path.clone(), refusal))?;
        }
        for (left, right) in [
            (&self.state, &self.releases),
            (&self.releases, &self.state),
            (&self.state, &self.legacy),
            (&self.legacy, &self.state),
            (&self.releases, &self.legacy),
            (&self.legacy, &self.releases),
            (&self.config, &self.releases),
            (&self.config, &self.state),
            (&self.data, &self.state),
        ] {
            if left.is_within(right)? {
                return Err(InstallLocationError::OverlappingRoots);
            }
        }
        if !self.releases.is_within(&self.data)? {
            return Err(InstallLocationError::OverlappingRoots);
        }
        Ok(())
    }
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_path(path: &Path) -> Result<(), InstallLocationError> {
    let Some(text) = path.to_str() else {
        return Err(InstallLocationError::InvalidPath(path.to_path_buf()));
    };
    if !path.is_absolute()
        || text.len() > 4096
        || text.contains("//")
        || (text.len() > 1 && text.ends_with('/'))
        || text.split('/').any(|part| matches!(part, "." | ".."))
        || text.bytes().any(|byte| byte < b' ' || byte == 127)
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(InstallLocationError::InvalidPath(path.to_path_buf()));
    }
    Ok(())
}
