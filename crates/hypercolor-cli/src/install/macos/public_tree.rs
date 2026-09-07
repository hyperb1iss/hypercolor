use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _};
use std::path::{Component, Path};

use hypercolor_platform_fs::{
    DirectoryEntryMetadata, EntryReplacement, ExactEntry, OpenedRegularFile,
    PublicDirectoryAuthority,
};

use super::super::InstallPlatformError;
use super::model::{
    MAX_LEGACY_MEMBERS, MAX_LEGACY_TOTAL_BYTES, MacosDirectoryState, MacosEntryPublication,
    MacosExactEntry, MacosFilePublication, MacosPublicSnapshot, error, hex_digest,
};

const PUBLIC_DIRECTORY_MODE: u32 = 0o755;

#[derive(Debug)]
pub struct MacosPublicTree {
    home_path: String,
    home: PublicDirectoryAuthority,
    directories: BTreeSet<String>,
    entries: BTreeSet<String>,
}

impl MacosPublicTree {
    pub fn new(
        home_path: impl Into<String>,
        home: PublicDirectoryAuthority,
    ) -> Result<Self, InstallPlatformError> {
        let home_path = home_path.into();
        super::model::validate_public_path(&home_path)?;
        home.metadata().map_err(io_error)?;
        Ok(Self {
            home_path,
            home,
            directories: BTreeSet::new(),
            entries: BTreeSet::new(),
        })
    }

    pub fn bind_paths(
        &mut self,
        directories: impl IntoIterator<Item = String>,
        entries: impl IntoIterator<Item = String>,
    ) -> Result<(), InstallPlatformError> {
        for directory in directories {
            self.relative(&directory)?;
            self.directories.insert(directory);
        }
        for entry in entries {
            self.relative(&entry)?;
            if entry == self.home_path {
                return Err(error("macOS public entry cannot replace HOME"));
            }
            self.entries.insert(entry);
        }
        Ok(())
    }

    pub(super) fn discover_tree(
        &self,
        root: &Path,
        include_all_files: bool,
        max_depth: usize,
        max_members: usize,
    ) -> Result<(Vec<String>, Vec<String>, usize), InstallPlatformError> {
        let root_text = root
            .to_str()
            .ok_or_else(|| error("macOS legacy inventory root is not exact UTF-8"))?;
        let Some(authority) = self.open_optional_directory(root_text)? else {
            return Ok((Vec::new(), Vec::new(), 0));
        };
        let mut discovery = TreeDiscovery {
            directories: BTreeSet::new(),
            entries: BTreeSet::new(),
            members: 0,
            max_depth,
            max_members,
            include_all_files,
        };
        discovery.walk(authority, root_text, 0)?;
        Ok((
            discovery.directories.into_iter().collect(),
            discovery.entries.into_iter().collect(),
            discovery.members,
        ))
    }

    pub fn snapshot(&self, max_bytes: u64) -> Result<MacosPublicSnapshot, InstallPlatformError> {
        if self.directories.len() + self.entries.len() > MAX_LEGACY_MEMBERS {
            return Err(error("macOS public snapshot exceeds its member bound"));
        }
        let mut directories = BTreeMap::new();
        for path in &self.directories {
            directories.insert(path.clone(), self.directory_state(path)?);
        }
        let mut entries = BTreeMap::new();
        let mut regular_bytes = BTreeMap::new();
        let mut total_bytes = 0u64;
        for path in &self.entries {
            let (entry, bytes) = self.entry(path, max_bytes)?;
            entries.insert(path.clone(), entry);
            if let Some(bytes) = bytes {
                total_bytes = total_bytes
                    .checked_add(bytes.len() as u64)
                    .filter(|total| *total <= MAX_LEGACY_TOTAL_BYTES)
                    .ok_or_else(|| error("macOS public snapshot exceeds aggregate bytes"))?;
                regular_bytes.insert(path.clone(), bytes);
            }
        }
        Ok(MacosPublicSnapshot {
            directories,
            entries,
            regular_bytes,
        })
    }

    pub fn entry(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> Result<(MacosExactEntry, Option<Vec<u8>>), InstallPlatformError> {
        let (parent, name) = self.parent(path)?;
        if self.directory_state(parent)? == MacosDirectoryState::Absent {
            return Ok((MacosExactEntry::Absent, None));
        }
        let observed = self.with_directory(parent, |authority| authority.observe_entry(name))?;
        match observed {
            ExactEntry::Absent => Ok((MacosExactEntry::Absent, None)),
            ExactEntry::Symlink { target, .. } => Ok((
                MacosExactEntry::Symlink {
                    target: target
                        .to_str()
                        .ok_or_else(|| error("macOS public symlink target is not exact UTF-8"))?
                        .to_owned(),
                },
                None,
            )),
            ExactEntry::RegularFile {
                mode, size, sha256, ..
            } => {
                if size > max_bytes {
                    return Err(error("macOS public regular entry exceeds its byte bound"));
                }
                let mut opened =
                    self.with_directory(parent, |authority| authority.open_regular_file(name))?;
                if opened.metadata().size() != size || opened.metadata().mode() != mode {
                    return Err(error(
                        "macOS public regular entry changed before exact read",
                    ));
                }
                let limit = size
                    .checked_add(1)
                    .ok_or_else(|| error("macOS public read bound overflowed"))?;
                let mut bytes =
                    Vec::with_capacity(usize::try_from(size).map_err(|_| {
                        error("macOS public regular entry size exceeds this process")
                    })?);
                opened
                    .file_mut()
                    .take(limit)
                    .read_to_end(&mut bytes)
                    .map_err(io_error)?;
                if bytes.len() as u64 != size || hex_digest(&bytes) != hex_bytes(&sha256) {
                    return Err(error(
                        "macOS public regular entry changed during exact read",
                    ));
                }
                let after =
                    self.with_directory(parent, |authority| authority.observe_entry(name))?;
                if after
                    != (ExactEntry::RegularFile {
                        mode,
                        size,
                        sha256,
                        device: opened.metadata().device(),
                        inode: opened.metadata().inode(),
                    })
                {
                    return Err(error("macOS public regular entry changed after exact read"));
                }
                Ok((
                    MacosExactEntry::RegularFile {
                        mode,
                        sha256: hex_bytes(&sha256),
                        snapshot_unit: None,
                        snapshot_path: None,
                    },
                    Some(bytes),
                ))
            }
        }
    }

    pub(super) fn regular_file(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> Result<(DirectoryEntryMetadata, Vec<u8>), InstallPlatformError> {
        let (opened, bytes) = self.retained_regular_file(path, max_bytes)?;
        Ok((opened.metadata(), bytes))
    }

    pub(super) fn retained_regular_file(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> Result<(OpenedRegularFile, Vec<u8>), InstallPlatformError> {
        let (parent, name) = self.parent(path)?;
        let before = self.with_directory(parent, |authority| authority.observe_entry(name))?;
        let ExactEntry::RegularFile {
            mode,
            size,
            sha256,
            device,
            inode,
        } = before
        else {
            return Err(error("macOS public executable is not a regular file"));
        };
        if size > max_bytes {
            return Err(error("macOS public executable exceeds its byte bound"));
        }
        let mut opened =
            self.with_directory(parent, |authority| authority.open_regular_file(name))?;
        let metadata = opened.metadata();
        if metadata.mode() != mode
            || metadata.size() != size
            || metadata.device() != device
            || metadata.inode() != inode
        {
            return Err(error("macOS public executable changed before exact read"));
        }
        let mut bytes = Vec::with_capacity(
            usize::try_from(size)
                .map_err(|_| error("macOS public executable size exceeds this process"))?,
        );
        opened
            .file_mut()
            .take(size.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        let after = self.with_directory(parent, |authority| authority.observe_entry(name))?;
        if bytes.len() as u64 != size
            || hex_digest(&bytes) != hex_bytes(&sha256)
            || after
                != (ExactEntry::RegularFile {
                    mode,
                    size,
                    sha256,
                    device,
                    inode,
                })
        {
            return Err(error("macOS public executable changed during exact read"));
        }
        Ok((opened, bytes))
    }

    pub fn replace_entry(
        &self,
        path: &str,
        expected: &MacosExactEntry,
        replacement: Option<&MacosEntryPublication>,
    ) -> Result<(), InstallPlatformError> {
        let (parent, name) = self.parent(path)?;
        let current = self.with_directory(parent, |authority| authority.observe_entry(name))?;
        require_model_match(&current, expected)?;
        match replacement {
            None if matches!(current, ExactEntry::Absent) => Ok(()),
            None => self.with_directory(parent, |authority| {
                authority.durable_remove_entry(name, &current)
            }),
            Some(MacosEntryPublication::RegularFile(file)) => self
                .with_directory(parent, |authority| {
                    authority.durable_replace_entry(
                        name,
                        &current,
                        EntryReplacement::RegularFile {
                            mode: file.mode,
                            contents: &file.contents,
                        },
                    )
                })
                .map(|_| ()),
            Some(MacosEntryPublication::Symlink(target)) => self
                .with_directory(parent, |authority| {
                    authority.durable_replace_entry(
                        name,
                        &current,
                        EntryReplacement::Symlink {
                            target: Path::new(target),
                        },
                    )
                })
                .map(|_| ()),
        }
    }

    pub fn replace_file(
        &self,
        path: &str,
        expected: &MacosExactEntry,
        replacement: Option<&MacosFilePublication>,
    ) -> Result<(), InstallPlatformError> {
        let replacement = replacement.map(|file| MacosEntryPublication::RegularFile(file.clone()));
        self.replace_entry(path, expected, replacement.as_ref())
    }

    pub fn ensure_directory(
        &self,
        path: &str,
        expected: MacosDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError> {
        let actual = self.directory_state(path)?;
        if actual != expected {
            return Err(error("macOS public directory drifted before exact effect"));
        }
        if !create {
            return Ok(());
        }
        let (parent, name) = self.parent(path)?;
        self.with_directory(parent, |authority| {
            authority.durable_ensure_child_directory(name, PUBLIC_DIRECTORY_MODE)
        })
        .map(|_| ())
    }

    fn directory_state(&self, path: &str) -> Result<MacosDirectoryState, InstallPlatformError> {
        let relative = self.relative(path)?;
        if relative.as_os_str().is_empty() {
            return Ok(MacosDirectoryState::Present);
        }
        let mut current: Option<PublicDirectoryAuthority> = None;
        for component in relative.components() {
            let opened = match &current {
                Some(parent) => parent.open_child_directory(Path::new(component.as_os_str())),
                None => self
                    .home
                    .open_child_directory(Path::new(component.as_os_str())),
            };
            match opened {
                Ok(child) => current = Some(child),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(MacosDirectoryState::Absent);
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(MacosDirectoryState::Present)
    }

    fn with_directory<T>(
        &self,
        path: &str,
        operation: impl FnOnce(&PublicDirectoryAuthority) -> io::Result<T>,
    ) -> Result<T, InstallPlatformError> {
        let relative = self.relative(path)?;
        if relative.as_os_str().is_empty() {
            return operation(&self.home).map_err(io_error);
        }
        let mut components = relative.components();
        let first = components.next().expect("nonempty relative path");
        let mut current = self
            .home
            .open_child_directory(Path::new(first.as_os_str()))
            .map_err(io_error)?;
        for component in components {
            current = current
                .open_child_directory(Path::new(component.as_os_str()))
                .map_err(io_error)?;
        }
        operation(&current).map_err(io_error)
    }

    fn open_optional_directory(
        &self,
        path: &str,
    ) -> Result<Option<PublicDirectoryAuthority>, InstallPlatformError> {
        let relative = self.relative(path)?;
        if relative.as_os_str().is_empty() {
            return Err(error("macOS legacy inventory cannot scan HOME itself"));
        }
        let mut current: Option<PublicDirectoryAuthority> = None;
        for component in relative.components() {
            let opened = match &current {
                Some(parent) => parent.open_child_directory(Path::new(component.as_os_str())),
                None => self
                    .home
                    .open_child_directory(Path::new(component.as_os_str())),
            };
            match opened {
                Ok(child) => current = Some(child),
                Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(source) => return Err(io_error(source)),
            }
        }
        Ok(current)
    }

    fn parent<'a>(&self, path: &'a str) -> Result<(&'a str, &'a Path), InstallPlatformError> {
        self.relative(path)?;
        let path = Path::new(path);
        let parent = path
            .parent()
            .and_then(Path::to_str)
            .ok_or_else(|| error("macOS public entry has no exact parent"))?;
        let name = path
            .file_name()
            .map(Path::new)
            .ok_or_else(|| error("macOS public entry has no exact name"))?;
        Ok((parent, name))
    }

    fn relative<'a>(&self, path: &'a str) -> Result<&'a Path, InstallPlatformError> {
        super::model::validate_public_path(path)?;
        Path::new(path)
            .strip_prefix(&self.home_path)
            .map_err(|_| error("macOS public path is outside retained HOME"))
            .and_then(|relative| {
                if relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
                    || relative.as_os_str().is_empty()
                {
                    Ok(relative)
                } else {
                    Err(error("macOS public path is not below retained HOME"))
                }
            })
    }
}

struct TreeDiscovery {
    directories: BTreeSet<String>,
    entries: BTreeSet<String>,
    members: usize,
    max_depth: usize,
    max_members: usize,
    include_all_files: bool,
}

impl TreeDiscovery {
    fn walk(
        &mut self,
        directory: PublicDirectoryAuthority,
        prefix: &str,
        depth: usize,
    ) -> Result<bool, InstallPlatformError> {
        if depth > self.max_depth {
            return Err(error(
                "macOS legacy public inventory exceeds its depth bound",
            ));
        }
        let mut owns_descendant = false;
        for name in directory.child_names().map_err(io_error)? {
            self.members = self
                .members
                .checked_add(1)
                .ok_or_else(|| error("macOS legacy public member count overflowed"))?;
            if self.members > self.max_members {
                return Err(error(
                    "macOS legacy public inventory exceeds its member bound",
                ));
            }
            let name_text = name
                .to_str()
                .ok_or_else(|| error("macOS legacy public entry name is not exact UTF-8"))?;
            let path = format!("{prefix}/{name_text}");
            if let Ok(child) = directory.open_child_directory(Path::new(&name)) {
                if self.walk(child, &path, depth + 1)? {
                    self.directories.insert(path);
                    owns_descendant = true;
                }
            } else {
                let entry = directory
                    .observe_entry(Path::new(&name))
                    .map_err(io_error)?;
                let owned = self.include_all_files || owned_icon_name(name_text);
                if owned {
                    if matches!(entry, ExactEntry::Absent) {
                        return Err(error("macOS legacy public entry disappeared"));
                    }
                    self.entries.insert(path);
                    owns_descendant = true;
                }
            }
        }
        Ok(owns_descendant)
    }
}

fn owned_icon_name(name: &str) -> bool {
    name == "hypercolor" || name.starts_with("hypercolor.") || name.starts_with("hypercolor-")
}

fn require_model_match(
    actual: &ExactEntry,
    expected: &MacosExactEntry,
) -> Result<(), InstallPlatformError> {
    let matches = match (actual, expected) {
        (ExactEntry::Absent, MacosExactEntry::Absent) => true,
        (
            ExactEntry::RegularFile { mode, sha256, .. },
            MacosExactEntry::RegularFile {
                mode: expected_mode,
                sha256: expected_sha,
                ..
            },
        ) => mode == expected_mode && hex_bytes(sha256) == *expected_sha,
        (
            ExactEntry::Symlink { target, .. },
            MacosExactEntry::Symlink {
                target: expected_target,
            },
        ) => target.to_str() == Some(expected_target),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(error("macOS public entry drifted before exact effect"))
    }
}

fn hex_bytes(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
            hex
        })
}

fn io_error(source: io::Error) -> InstallPlatformError {
    error(source.to_string())
}
