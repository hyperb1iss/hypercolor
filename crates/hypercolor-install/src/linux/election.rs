//! Select the permanent locator before taking an installation authority lock.

use std::io::{self, Read as _};
use std::path::Path;

use hypercolor_platform_fs::{ExactEntry, PublicDirectoryAuthority, ReadOnlyDirectoryAuthority};

use super::{
    InstallLock, InstallStore, LOCATOR_NAME, LinuxInstallAuthority, LinuxInstallLocation,
    LinuxInstallLocator, LinuxLocatorError, MAX_INSTALL_JOURNAL_BYTES, MAX_LOCATOR_BYTES,
    MAX_MANAGED_INSTALL_JOURNAL_BYTES,
};
use crate::OwnershipPolicy;
use crate::linux::RetainedLinuxInstallLocation;

/// The exclusive authority selected after rereading the permanent locator.
#[derive(Debug)]
pub enum LinuxInstallElection {
    Legacy {
        store: InstallStore,
        lock: InstallLock,
        locator: LinuxInstallLocator,
    },
    Managed {
        store: InstallStore,
        lock: InstallLock,
        authority: LinuxManagedAuthority,
    },
}

/// Managed authority retains only the elected state lock, never the old lock.
#[derive(Debug)]
pub struct LinuxManagedAuthority {
    locator: LinuxInstallLocator,
    location: LinuxInstallLocation,
    roots: RetainedLinuxInstallLocation,
    state: PublicDirectoryAuthority,
    identity: ExactEntry,
}

impl LinuxManagedAuthority {
    #[must_use]
    pub fn location(&self) -> &LinuxInstallLocation {
        &self.location
    }

    /// Reconfirm the permanent locator barrier and retained topology.
    ///
    /// # Errors
    /// Refuses changed ancestry, identity, roots or an unsuccessful barrier.
    pub fn confirm_durable(&self) -> Result<(), LinuxLocatorError> {
        self.roots.validate()?;
        let journal = self.state.open_regular_file(Path::new(LOCATOR_NAME))?;
        super::preparation::require_file_owner(journal.metadata(), self.location.uid())?;
        if self.state.observe_entry(Path::new("installation.json"))? != self.identity {
            return Err(LinuxLocatorError::Unprepared);
        }
        self.locator.confirm_durable(&self.location)?;
        self.roots.validate()?;
        if self.state.observe_entry(Path::new("installation.json"))? != self.identity {
            return Err(LinuxLocatorError::Unprepared);
        }
        Ok(())
    }
}

/// Elect legacy or managed authority without acquiring locks in reverse order.
///
/// A managed hint only selects which existing state lock to attempt. The exact
/// locator, identity file, journal and roots are checked again under that lock.
///
/// # Errors
/// Refuses malformed locators, contended locks, missing managed preparation,
/// changed authority or failed directory durability. Never falls back from V2.
pub fn elect_linux_installation(home: &Path) -> Result<LinuxInstallElection, LinuxLocatorError> {
    elect_linux_installation_with(home, &OwnershipPolicy::system())
}

/// Elect authority with an explicit directory writer policy.
///
/// Every store and lock produced by the election carries `ownership`.
///
/// # Errors
/// Returns the same errors as [`elect_linux_installation`].
pub fn elect_linux_installation_with(
    home: &Path,
    ownership: &OwnershipPolicy,
) -> Result<LinuxInstallElection, LinuxLocatorError> {
    elect_with(home, ownership, || {})
}

pub(super) fn elect_with(
    home: &Path,
    ownership: &OwnershipPolicy,
    after_hint: impl FnOnce(),
) -> Result<LinuxInstallElection, LinuxLocatorError> {
    let hint = read_hint(home)?;
    after_hint();
    if let LinuxInstallAuthority::Managed(location) = hint {
        return elect_managed(home, location, ownership);
    }
    let root = home.join(".local/lib/hypercolor");
    let store =
        InstallStore::new(root, MAX_INSTALL_JOURNAL_BYTES).with_ownership_policy(ownership.clone());
    let lock = store.acquire_anchored_lock(home)?;
    let locator = LinuxInstallLocator::retain(home, &lock)?;
    match locator.read()? {
        LinuxInstallAuthority::Legacy(_) => Ok(LinuxInstallElection::Legacy {
            store,
            lock,
            locator,
        }),
        LinuxInstallAuthority::Managed(location) => {
            // A prior adopter may publish between the hint and old-lock grant.
            // Old then state is the only permitted two-lock acquisition order.
            let elected = elect_managed(home, location, ownership)?;
            drop(locator);
            drop(lock);
            Ok(elected)
        }
    }
}

fn elect_managed(
    home: &Path,
    location: LinuxInstallLocation,
    ownership: &OwnershipPolicy,
) -> Result<LinuxInstallElection, LinuxLocatorError> {
    let store = InstallStore::with_roots(
        location.release_root(),
        location.state_root(),
        MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    )?
    .with_ownership_policy(ownership.clone());
    // Split-root acquire_lock opens existing roots; it does not bootstrap them.
    let lock = store.acquire_lock()?;
    let authority = LinuxManagedAuthority::retain(home, &store, &lock, location)?;
    Ok(LinuxInstallElection::Managed {
        store,
        lock,
        authority,
    })
}

impl LinuxManagedAuthority {
    pub(crate) fn retain(
        home: &Path,
        store: &InstallStore,
        lock: &InstallLock,
        location: LinuxInstallLocation,
    ) -> Result<Self, LinuxLocatorError> {
        if store.root() != location.release_root()
            || store.state_root() != location.state_root()
            || !lock.guards_roots(location.release_root(), location.state_root())
        {
            return Err(LinuxLocatorError::WrongLock);
        }
        let root = home.join(".local/lib/hypercolor");
        let locator = LinuxInstallLocator {
            home: home.to_owned(),
            public: lock.open_public_directory(&root)?,
            directory: lock
                .open_public_directory(&root)?
                .into_directory_authority()?,
        };
        let roots = location.retain_existing(home, lock)?;
        locator.confirm_durable(&location)?;
        let state = lock.open_public_directory(location.state_root())?;
        let identity_observation = state.observe_entry(Path::new("installation.json"))?;
        let mut identity = state.open_regular_file(Path::new("installation.json"))?;
        super::preparation::require_file_owner(identity.metadata(), location.uid())?;
        let mut bytes = Vec::new();
        identity
            .file_mut()
            .take(MAX_LOCATOR_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LOCATOR_BYTES
            || LinuxInstallLocation::parse(&bytes, home)? != location
            || store.load_journal(lock)?.is_none()
        {
            return Err(LinuxLocatorError::Unprepared);
        }
        roots.validate()?;
        let authority = LinuxManagedAuthority {
            locator,
            location,
            roots,
            state,
            identity: identity_observation,
        };
        authority.confirm_durable()?;
        Ok(authority)
    }
}

pub(in crate::linux) fn read_hint(home: &Path) -> Result<LinuxInstallAuthority, LinuxLocatorError> {
    let root = match ReadOnlyDirectoryAuthority::open(&home.join(".local/lib/hypercolor")) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LinuxInstallAuthority::Legacy(None));
        }
        Err(error) => return Err(error.into()),
    };
    let mut file = match root.open_regular_file(Path::new(LOCATOR_NAME)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LinuxInstallAuthority::Legacy(None));
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.file_mut()
        .take(MAX_LOCATOR_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LOCATOR_BYTES {
        return Err(LinuxLocatorError::InvalidLocator);
    }
    LinuxInstallLocator::decode_at(home, Some(bytes))
}
