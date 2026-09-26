use std::io::{self, Read as _};
use std::path::{Component, Path};

use hypercolor_platform_fs::PublicDirectoryAuthority;

use super::super::ownership::{DirectoryRole, OwnershipPolicy};
use super::super::{InstallLock, InstallStore, MAX_INSTALL_JOURNAL_BYTES};
use super::{LinuxInstallLocation, LinuxLocatorError};

/// Exact modes for directories adoption creates, applied after creation so the
/// process umask never decides them. The daemon reads releases like the
/// historical store; journal state and configuration stay private.
const DATA_ROOT_MODE: u32 = 0o755;
const RELEASE_ROOT_MODE: u32 = 0o755;
const STATE_ROOT_MODE: u32 = 0o700;
const CONFIG_ROOT_MODE: u32 = 0o700;

pub(super) fn prepare_roots(
    home: &Path,
    old_lock: &InstallLock,
    proposed: LinuxInstallLocation,
) -> Result<(LinuxInstallLocation, InstallStore, InstallLock), LinuxLocatorError> {
    let owner = old_lock.open_public_directory(home)?.metadata()?;
    if owner.owner_uid() != proposed.uid() || !owner.is_owned_by_current_user() {
        return Err(LinuxLocatorError::Unprepared);
    }
    let ownership = old_lock.ownership_policy().clone();
    let state_container = proposed
        .state_root()
        .parent()
        .ok_or(LinuxLocatorError::Unprepared)?;
    let data = bootstrap_root(
        old_lock,
        proposed.data_root(),
        DATA_ROOT_MODE,
        DirectoryRole::Ancestor,
    )?;
    let releases = bootstrap_root(
        old_lock,
        proposed.release_root(),
        RELEASE_ROOT_MODE,
        DirectoryRole::InstallerOwned,
    )?;
    bootstrap_root(
        old_lock,
        state_container,
        STATE_ROOT_MODE,
        DirectoryRole::Ancestor,
    )?;
    let state = bootstrap_root(
        old_lock,
        proposed.state_root(),
        STATE_ROOT_MODE,
        DirectoryRole::InstallerOwned,
    )?;
    let config = bootstrap_root(
        old_lock,
        proposed.config_root(),
        CONFIG_ROOT_MODE,
        DirectoryRole::Ancestor,
    )?;
    let paths = [
        proposed.data_root(),
        proposed.state_root(),
        proposed.release_root(),
        proposed.config_root(),
    ];
    let original = [data, state, releases, config];
    proposed.retain_existing(home, old_lock)?.validate()?;
    let store = InstallStore::with_roots(
        proposed.release_root(),
        proposed.state_root(),
        MAX_INSTALL_JOURNAL_BYTES,
    )?
    .with_ownership_policy(ownership);
    let lock = store.acquire_lock()?;
    for (path, retained) in paths.into_iter().zip(&original) {
        let before = retained.metadata()?;
        let after = lock.open_public_directory(path)?.metadata()?;
        if (before.device(), before.inode()) != (after.device(), after.inode()) {
            return Err(LinuxLocatorError::Unprepared);
        }
    }
    proposed.retain_existing(home, &lock)?.validate()?;
    let location = retain_identity(home, &lock, proposed)?;
    for root in &original {
        root.validate_ancestry()?;
    }
    Ok((location, store, lock))
}

/// Open `path`, creating each missing component with the exact `mode`.
///
/// A missing component is created only beneath a parent that passes the
/// ancestor writer rule. The final directory must pass `role`. Existing
/// components are never chmodded; their authority is proven or refused.
fn bootstrap_root(
    lock: &InstallLock,
    path: &Path,
    mode: u32,
    role: DirectoryRole,
) -> Result<PublicDirectoryAuthority, LinuxLocatorError> {
    let ownership = lock.ownership_policy();
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(LinuxLocatorError::Unprepared);
    }
    let Some(Component::Normal(first)) = components.next() else {
        return Err(LinuxLocatorError::Unprepared);
    };
    let mut current = Path::new("/").join(first);
    let mut authority = lock.open_public_directory(&current)?;
    for component in components {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(LinuxLocatorError::Unprepared),
        };
        authority = match authority.open_child_directory(Path::new(name)) {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                require_directory(ownership, &authority, &current, DirectoryRole::Ancestor)?;
                authority.durable_ensure_child_directory(Path::new(name), mode)?
            }
            Err(error) => return Err(error.into()),
        };
        current.push(name);
    }
    require_directory(ownership, &authority, &current, role)?;
    Ok(authority)
}

fn require_directory(
    ownership: &OwnershipPolicy,
    authority: &PublicDirectoryAuthority,
    path: &Path,
    role: DirectoryRole,
) -> Result<(), LinuxLocatorError> {
    ownership
        .require_owner_only(authority, authority.metadata()?, role)
        .map_err(|refusal| LinuxLocatorError::UnsafeDirectory(path.to_path_buf(), refusal))
}

fn retain_identity(
    home: &Path,
    lock: &InstallLock,
    proposed: LinuxInstallLocation,
) -> Result<LinuxInstallLocation, LinuxLocatorError> {
    let state = lock.open_public_directory(proposed.state_root())?;
    let name = Path::new("installation.json");
    let location = match state.open_regular_file(name) {
        Ok(mut file) => {
            require_owner(file.metadata(), proposed.uid())?;
            let mut bytes = Vec::new();
            file.file_mut()
                .take(MAX_INSTALL_JOURNAL_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            let recorded = LinuxInstallLocation::parse(&bytes, home)?;
            if recorded.uid() != proposed.uid()
                || recorded.data_root() != proposed.data_root()
                || recorded.state_root() != proposed.state_root()
                || recorded.release_root() != proposed.release_root()
                || recorded.config_root() != proposed.config_root()
            {
                return Err(LinuxLocatorError::Unprepared);
            }
            recorded
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let bytes = serde_json::to_vec(&proposed)?;
            lock.open_public_directory(proposed.state_root())?
                .into_directory_authority()?
                .create_regular_file(name, 0o600, bytes.len() as u64, &mut bytes.as_slice())?;
            proposed
        }
        Err(error) => return Err(error.into()),
    };
    let identity = state.open_regular_file(name)?;
    require_owner(identity.metadata(), location.uid())?;
    identity.file().sync_all()?;
    lock.open_public_directory(location.state_root())?
        .into_directory_authority()?
        .sync()?;
    state.validate_ancestry()?;
    Ok(location)
}

fn require_owner(
    metadata: hypercolor_platform_fs::DirectoryEntryMetadata,
    uid: u32,
) -> Result<(), LinuxLocatorError> {
    if metadata.owner_uid() != uid
        || !metadata.is_owned_by_current_user()
        || metadata.mode() & 0o022 != 0
    {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(())
}
