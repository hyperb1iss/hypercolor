use std::io::{self, Read as _};
use std::path::{Component, Path};

use hypercolor_platform_fs::PublicDirectoryAuthority;

use super::super::{InstallLock, InstallStore, MAX_INSTALL_JOURNAL_BYTES};
use super::{LinuxInstallLocation, LinuxLocatorError};

pub(super) fn prepare_roots(
    home: &Path,
    old_lock: &InstallLock,
    proposed: LinuxInstallLocation,
) -> Result<(LinuxInstallLocation, InstallStore, InstallLock), LinuxLocatorError> {
    let owner = old_lock.open_public_directory(home)?.metadata()?;
    if owner.owner_uid() != proposed.uid() || !owner.is_owned_by_current_user() {
        return Err(LinuxLocatorError::Unprepared);
    }
    let paths = [
        proposed.data_root(),
        proposed.state_root(),
        proposed.release_root(),
        proposed.config_root(),
    ];
    let mut original = Vec::new();
    for path in paths {
        original.push(bootstrap_root(old_lock, path)?);
    }
    proposed.retain_existing(home, old_lock)?.validate()?;
    let store = InstallStore::with_roots(
        proposed.release_root(),
        proposed.state_root(),
        MAX_INSTALL_JOURNAL_BYTES,
    )?;
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

fn bootstrap_root(
    lock: &InstallLock,
    path: &Path,
) -> Result<PublicDirectoryAuthority, LinuxLocatorError> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(LinuxLocatorError::Unprepared);
    }
    let Some(Component::Normal(first)) = components.next() else {
        return Err(LinuxLocatorError::Unprepared);
    };
    let mut authority = lock.open_public_directory(&Path::new("/").join(first))?;
    for component in components {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(LinuxLocatorError::Unprepared),
        };
        authority = match authority.open_child_directory(Path::new(name)) {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let owner = authority.metadata()?;
                if !owner.is_owned_by_current_user() || owner.mode() & 0o022 != 0 {
                    return Err(LinuxLocatorError::Unprepared);
                }
                authority.durable_ensure_child_directory(Path::new(name), 0o755)?
            }
            Err(error) => return Err(error.into()),
        };
    }
    let metadata = authority.metadata()?;
    if !metadata.is_owned_by_current_user() || metadata.mode() & 0o022 != 0 {
        return Err(LinuxLocatorError::Unprepared);
    }
    Ok(authority)
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
