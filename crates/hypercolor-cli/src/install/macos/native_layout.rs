use std::collections::BTreeSet;
use std::path::Path;

use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};

use super::super::{InstallPlatformError, UnitRecord};
use super::model::{
    MAX_LEGACY_DEPTH, MAX_LEGACY_MEMBERS, MacosCandidateLayout, compare_directory_paths, error,
};

const BINARIES: [&str; 5] = [
    "hypercolor",
    "hypercolor-daemon",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
];

pub(super) fn candidate_layout(
    unit: &UnitRecord,
    home: &Path,
    install_prefix: &Path,
    install_dir: &Path,
    active_root: &Path,
    direct_plist: &Path,
    log_directory: &Path,
) -> Result<MacosCandidateLayout, InstallPlatformError> {
    let mut builder = LayoutBuilder::new(home, active_root);
    for binary in BINARIES {
        builder.add_file(
            unit.directory(),
            &format!("bin/{binary}"),
            &install_dir.join(binary),
        )?;
    }
    builder.add_file(
        unit.directory(),
        "share/applications/hypercolor.desktop",
        &install_prefix.join("share/applications/hypercolor.desktop"),
    )?;
    builder.add_file(
        unit.directory(),
        "share/bash-completion/completions/hypercolor",
        &install_prefix.join("share/bash-completion/completions/hypercolor"),
    )?;
    builder.add_file(
        unit.directory(),
        "share/zsh/site-functions/_hypercolor",
        &install_prefix.join("share/zsh/site-functions/_hypercolor"),
    )?;
    builder.add_file(
        unit.directory(),
        "share/fish/vendor_completions.d/hypercolor.fish",
        &home.join(".config/fish/completions/hypercolor.fish"),
    )?;
    builder.add_tree(
        unit.directory(),
        "share/hypercolor/ui",
        &install_prefix.join("share/hypercolor/ui"),
    )?;
    builder.add_tree(
        unit.directory(),
        "share/icons",
        &install_prefix.join("share/icons"),
    )?;
    builder.add_directory(
        direct_plist
            .parent()
            .ok_or_else(|| error("macOS launchd plist has no parent"))?,
    )?;
    builder.add_directory(log_directory)?;
    Ok(builder.finish())
}

struct LayoutBuilder<'a> {
    home: &'a Path,
    active_root: &'a Path,
    directories: BTreeSet<String>,
    entries: BTreeSet<(String, String)>,
    members: usize,
}

impl<'a> LayoutBuilder<'a> {
    fn new(home: &'a Path, active_root: &'a Path) -> Self {
        Self {
            home,
            active_root,
            directories: BTreeSet::new(),
            entries: BTreeSet::new(),
            members: 0,
        }
    }

    fn add_file(
        &mut self,
        unit: &ReadOnlyDirectoryAuthority,
        source: &str,
        public: &Path,
    ) -> Result<(), InstallPlatformError> {
        let (parent, name) = open_parent(unit, source)?;
        let metadata = parent
            .entry_metadata(Path::new(name))
            .map_err(io_error)?
            .ok_or_else(|| error(format!("macOS release unit is missing {source}")))?;
        if metadata.kind() != DirectoryEntryKind::RegularFile || metadata.link_count() != 1 {
            return Err(error(format!(
                "macOS release member {source} is not regular"
            )));
        }
        self.add_public_parents(public)?;
        let public = exact_path(public)?;
        let target = exact_path(&self.active_root.join(source))?;
        if !self.entries.insert((public, target)) {
            return Err(error(
                "macOS candidate projection contains a duplicate entry",
            ));
        }
        self.bump_member()
    }

    fn add_tree(
        &mut self,
        unit: &ReadOnlyDirectoryAuthority,
        source: &str,
        public: &Path,
    ) -> Result<(), InstallPlatformError> {
        let directory = open_directory(unit, source)?;
        self.walk_tree(&directory, source, public, 0)
    }

    fn walk_tree(
        &mut self,
        directory: &ReadOnlyDirectoryAuthority,
        source: &str,
        public: &Path,
        depth: usize,
    ) -> Result<(), InstallPlatformError> {
        if depth > MAX_LEGACY_DEPTH {
            return Err(error("macOS candidate projection exceeds its depth bound"));
        }
        self.add_directory(public)?;
        for name in directory.child_names().map_err(io_error)? {
            self.bump_member()?;
            let name_text = name
                .to_str()
                .ok_or_else(|| error("macOS release member name is not exact UTF-8"))?;
            let metadata = directory
                .entry_metadata(Path::new(&name))
                .map_err(io_error)?
                .ok_or_else(|| error("macOS release member disappeared during projection"))?;
            let child_source = format!("{source}/{name_text}");
            let child_public = public.join(name_text);
            match metadata.kind() {
                DirectoryEntryKind::Directory => {
                    let child = directory
                        .open_child_directory(Path::new(&name))
                        .map_err(io_error)?;
                    self.walk_tree(&child, &child_source, &child_public, depth + 1)?;
                }
                DirectoryEntryKind::RegularFile if metadata.link_count() == 1 => {
                    self.add_public_parents(&child_public)?;
                    let path = exact_path(&child_public)?;
                    let target = exact_path(&self.active_root.join(&child_source))?;
                    if !self.entries.insert((path, target)) {
                        return Err(error(
                            "macOS candidate projection contains a duplicate entry",
                        ));
                    }
                }
                _ => {
                    return Err(error(
                        "macOS candidate projection contains an unsafe member",
                    ));
                }
            }
        }
        Ok(())
    }

    fn add_public_parents(&mut self, path: &Path) -> Result<(), InstallPlatformError> {
        let parent = path
            .parent()
            .ok_or_else(|| error("macOS public projection entry has no parent"))?;
        self.add_directory(parent)
    }

    fn add_directory(&mut self, path: &Path) -> Result<(), InstallPlatformError> {
        let relative = path
            .strip_prefix(self.home)
            .map_err(|_| error("macOS public projection is outside retained HOME"))?;
        let mut current = self.home.to_path_buf();
        for component in relative.components() {
            current.push(component.as_os_str());
            self.directories.insert(exact_path(&current)?);
        }
        Ok(())
    }

    fn bump_member(&mut self) -> Result<(), InstallPlatformError> {
        self.members = self
            .members
            .checked_add(1)
            .ok_or_else(|| error("macOS candidate member count overflowed"))?;
        if self.members > MAX_LEGACY_MEMBERS {
            return Err(error("macOS candidate projection exceeds its member bound"));
        }
        Ok(())
    }

    fn finish(self) -> MacosCandidateLayout {
        let mut directories = self.directories.into_iter().collect::<Vec<_>>();
        directories.sort_by(|left, right| compare_directory_paths(left, right));
        let entries = self.entries.into_iter().collect();
        MacosCandidateLayout {
            directories,
            entries,
        }
    }
}

fn open_directory(
    root: &ReadOnlyDirectoryAuthority,
    path: &str,
) -> Result<ReadOnlyDirectoryAuthority, InstallPlatformError> {
    let mut components = path.split('/');
    let first = components
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("macOS retained release member path is not canonical"))?;
    let mut current = root
        .open_child_directory(Path::new(first))
        .map_err(io_error)?;
    for component in components {
        current = current
            .open_child_directory(Path::new(component))
            .map_err(io_error)?;
    }
    Ok(current)
}

fn open_parent<'a>(
    root: &ReadOnlyDirectoryAuthority,
    path: &'a str,
) -> Result<(ReadOnlyDirectoryAuthority, &'a str), InstallPlatformError> {
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or_else(|| error("macOS retained release member has no parent"))?;
    Ok((open_directory(root, parent)?, name))
}

fn exact_path(path: &Path) -> Result<String, InstallPlatformError> {
    let path = path
        .to_str()
        .ok_or_else(|| error("macOS public projection path is not exact UTF-8"))?
        .to_owned();
    super::model::validate_public_path(&path)?;
    Ok(path)
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
