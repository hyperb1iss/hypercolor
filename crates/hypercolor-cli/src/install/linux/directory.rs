use std::collections::BTreeMap;
use std::io::{self, Read as _};
use std::path::Path;

use hypercolor_platform_fs::{
    ExactDirectoryEntry, ExactEntry, OpenedRegularFile, PublicDirectoryAuthority,
};

use super::super::{InstallLock, InstallPlatformError};
use super::model::{
    LINUX_DIRECTORY_ITEMS, LinuxDirectoryItem, LinuxDirectoryState, PUBLIC_DIRECTORY_MODE, error,
};

#[derive(Debug)]
enum DirectoryObservation {
    Absent,
    Present(PublicDirectoryAuthority),
}

impl DirectoryObservation {
    fn state(&self) -> LinuxDirectoryState {
        match self {
            Self::Absent => LinuxDirectoryState::Absent,
            Self::Present(_) => LinuxDirectoryState::Present,
        }
    }
}

#[derive(Debug)]
pub struct LinuxPublicTree {
    home: PublicDirectoryAuthority,
    direct_fragment_path: String,
    directories: BTreeMap<LinuxDirectoryItem, DirectoryObservation>,
}

impl LinuxPublicTree {
    pub fn new(lock: &InstallLock, home: &Path) -> Result<Self, InstallPlatformError> {
        let direct_fragment_path = home
            .join(".config/systemd/user/hypercolor.service")
            .to_str()
            .ok_or_else(|| error("Linux HOME must be exact UTF-8"))?
            .to_owned();
        let home = lock
            .open_public_directory(home)
            .map_err(|source| error(source.to_string()))?;
        let mut tree = Self {
            home,
            direct_fragment_path,
            directories: BTreeMap::new(),
        };
        for item in LINUX_DIRECTORY_ITEMS {
            let state = match tree.parent_state(item) {
                LinuxDirectoryState::Present => tree.observe_child(item)?,
                LinuxDirectoryState::Absent => DirectoryObservation::Absent,
            };
            tree.directories.insert(item, state);
        }
        Ok(tree)
    }

    pub(super) fn direct_fragment_path(&self) -> &str {
        &self.direct_fragment_path
    }

    /// Observe one fixed public scaffold directory through retained authorities.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing authority no longer has its retained
    /// ancestry or an unretained entry appears where absence was recorded.
    pub fn state(
        &self,
        item: LinuxDirectoryItem,
    ) -> Result<LinuxDirectoryState, InstallPlatformError> {
        let recorded = self
            .directories
            .get(&item)
            .map(DirectoryObservation::state)
            .ok_or_else(|| error("unknown Linux public directory"))?;
        if recorded == LinuxDirectoryState::Present {
            self.open_directory(item)?;
            return Ok(recorded);
        }
        let parent_state = item
            .parent()
            .map(|parent| self.state(parent))
            .transpose()?
            .unwrap_or(LinuxDirectoryState::Present);
        if parent_state == LinuxDirectoryState::Present {
            self.require_child_absent(item)?;
        }
        Ok(recorded)
    }

    /// Durably create one fixed scaffold directory from an exact absent state.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory was not absent, its parent has not
    /// been retained yet, or durable handle-relative creation cannot be proven.
    pub fn ensure_directory(
        &mut self,
        item: LinuxDirectoryItem,
        expected: LinuxDirectoryState,
    ) -> Result<(), InstallPlatformError> {
        if self.state(item)? != expected {
            return Err(error("public directory drifted before exact mutation"));
        }
        if expected != LinuxDirectoryState::Absent {
            return Err(error("public directory creation expected absence"));
        }
        if self.parent_state(item) != LinuxDirectoryState::Present {
            return Err(error("public directory parent authority is absent"));
        }
        let created = match item.parent() {
            Some(parent) => self
                .open_directory(parent)?
                .durable_ensure_child_directory(Path::new(item.name()), PUBLIC_DIRECTORY_MODE),
            None => self
                .home
                .durable_ensure_child_directory(Path::new(item.name()), PUBLIC_DIRECTORY_MODE),
        }
        .map_err(io_error)?;
        created.validate_ancestry().map_err(io_error)?;
        self.directories
            .insert(item, DirectoryObservation::Present(created));
        Ok(())
    }

    pub(super) fn replace(
        &mut self,
        item: LinuxDirectoryItem,
        expected: LinuxDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError> {
        if !create {
            return Err(error(
                "version-neutral public scaffolding cannot be removed by rollback",
            ));
        }
        self.ensure_directory(item, expected)
    }

    pub(super) fn open_directory(
        &self,
        item: LinuxDirectoryItem,
    ) -> Result<PublicDirectoryAuthority, InstallPlatformError> {
        let retained = match self.directories.get(&item) {
            Some(DirectoryObservation::Present(authority)) => authority,
            Some(DirectoryObservation::Absent) => {
                return Err(error("Linux public directory authority is absent"));
            }
            None => return Err(error("unknown Linux public directory")),
        };
        retained.validate_ancestry().map_err(io_error)?;
        let mut authority = self
            .home
            .open_child_directory(Path::new(first_name(item)))
            .map_err(io_error)?;
        for component in descendants(item) {
            authority = authority
                .open_child_directory(Path::new(component))
                .map_err(io_error)?;
        }
        retained.validate_ancestry().map_err(io_error)?;
        Ok(authority)
    }

    pub(super) fn open_optional_relative_directory(
        &self,
        root: LinuxDirectoryItem,
        components: &[&str],
    ) -> Result<Option<PublicDirectoryAuthority>, InstallPlatformError> {
        let mut authority = self.open_directory(root)?;
        for component in components {
            match authority.open_child_directory(Path::new(component)) {
                Ok(child) => authority = child,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    if authority
                        .observe_entry(Path::new(component))
                        .map_err(io_error)?
                        == ExactEntry::Absent
                    {
                        return Ok(None);
                    }
                    return Err(error("legacy public directory changed during traversal"));
                }
                Err(source) => return Err(io_error(source)),
            }
        }
        Ok(Some(authority))
    }

    fn parent_state(&self, item: LinuxDirectoryItem) -> LinuxDirectoryState {
        item.parent()
            .map_or(LinuxDirectoryState::Present, |parent| {
                self.directories
                    .get(&parent)
                    .map(DirectoryObservation::state)
                    .unwrap_or(LinuxDirectoryState::Absent)
            })
    }

    fn observe_child(
        &self,
        item: LinuxDirectoryItem,
    ) -> Result<DirectoryObservation, InstallPlatformError> {
        let opened = match item.parent() {
            Some(parent) => self
                .open_directory(parent)?
                .open_child_directory(Path::new(item.name())),
            None => self.home.open_child_directory(Path::new(item.name())),
        };
        match opened {
            Ok(authority) => Ok(DirectoryObservation::Present(authority)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                self.require_child_absent(item)?;
                Ok(DirectoryObservation::Absent)
            }
            Err(source) => Err(io_error(source)),
        }
    }

    fn require_child_absent(&self, item: LinuxDirectoryItem) -> Result<(), InstallPlatformError> {
        match item.parent() {
            Some(parent) => require_absent(&self.open_directory(parent)?, item),
            None => require_absent(&self.home, item),
        }
    }
}

fn first_name(item: LinuxDirectoryItem) -> &'static str {
    let mut current = item;
    while let Some(parent) = current.parent() {
        current = parent;
    }
    current.name()
}

fn descendants(item: LinuxDirectoryItem) -> Vec<&'static str> {
    let mut names = Vec::new();
    let mut current = item;
    while let Some(parent) = current.parent() {
        names.push(current.name());
        current = parent;
    }
    names.reverse();
    names
}

fn require_absent(
    parent: &PublicDirectoryAuthority,
    item: LinuxDirectoryItem,
) -> Result<(), InstallPlatformError> {
    if parent
        .observe_empty_child_directory(Path::new(item.name()))
        .map_err(io_error)?
        != ExactDirectoryEntry::Absent
    {
        return Err(error("unretained public directory appeared"));
    }
    Ok(())
}

pub(super) fn read_opened_public_bytes(
    opened: &mut OpenedRegularFile,
    initial_size: u64,
    max_bytes: usize,
) -> Result<Vec<u8>, InstallPlatformError> {
    if initial_size > max_bytes as u64 {
        return Err(error("public regular entry exceeds its byte bound"));
    }
    let capacity = usize::try_from(initial_size)
        .map_err(|_| error("public regular entry does not fit in memory"))?;
    let mut bytes = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(initial_size + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() != capacity {
        return Err(error("public regular entry changed size while reading"));
    }
    Ok(bytes)
}

fn io_error(source: io::Error) -> InstallPlatformError {
    error(source.to_string())
}
