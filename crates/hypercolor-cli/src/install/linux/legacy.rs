use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use hypercolor_platform_fs::{ExactEntry, PublicDirectoryAuthority};
use sha2::{Digest as _, Sha256};

use super::super::InstallPlatformError;
use super::directory::LinuxPublicTree;
use super::executor::{public_name, public_parent};
use super::model::{
    LinuxDirectoryItem, LinuxExactEntry, LinuxLayoutItem, LinuxLegacyFile, LinuxLegacySnapshot,
    MAX_LEGACY_DEPTH, MAX_LEGACY_FILE_BYTES, MAX_LEGACY_MEMBERS, MAX_LEGACY_PATH_BYTES,
    MAX_LEGACY_TOTAL_BYTES, error,
};

pub(super) type LegacyFile = LinuxLegacyFile;
type LegacyDescriptors = BTreeMap<String, (u32, String)>;

pub(super) struct LegacyBudget {
    members: usize,
    bytes: u64,
    limits: LegacyLimits,
}

#[derive(Clone, Copy)]
pub(super) struct LegacyLimits {
    pub(super) depth: usize,
    pub(super) members: usize,
    pub(super) file_bytes: u64,
    pub(super) total_bytes: u64,
}

impl LegacyBudget {
    pub(super) fn new() -> Self {
        Self {
            members: 0,
            bytes: 0,
            limits: LegacyLimits {
                depth: MAX_LEGACY_DEPTH,
                members: MAX_LEGACY_MEMBERS,
                file_bytes: MAX_LEGACY_FILE_BYTES,
                total_bytes: MAX_LEGACY_TOTAL_BYTES,
            },
        }
    }

    #[cfg(test)]
    pub(super) fn with_limits(limits: LegacyLimits) -> Self {
        Self {
            members: 0,
            bytes: 0,
            limits,
        }
    }

    pub(super) fn member(&mut self, depth: usize, path: &str) -> Result<(), InstallPlatformError> {
        if depth > self.limits.depth || path.len() > MAX_LEGACY_PATH_BYTES {
            return Err(error("legacy inventory exceeds its depth or path bound"));
        }
        self.members = self
            .members
            .checked_add(1)
            .ok_or_else(|| error("legacy inventory member count overflowed"))?;
        if self.members > self.limits.members {
            return Err(error("legacy inventory exceeds its member bound"));
        }
        Ok(())
    }

    pub(super) fn file(&mut self, size: u64) -> Result<usize, InstallPlatformError> {
        if size > self.limits.file_bytes {
            return Err(error("legacy inventory file exceeds its byte bound"));
        }
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or_else(|| error("legacy inventory byte count overflowed"))?;
        if self.bytes > self.limits.total_bytes {
            return Err(error("legacy inventory exceeds its aggregate byte bound"));
        }
        usize::try_from(size).map_err(|_| error("legacy inventory file does not fit in memory"))
    }
}

pub(super) fn collect_public_legacy_inventory(
    public_tree: &LinuxPublicTree,
) -> Result<Vec<LinuxLegacyFile>, InstallPlatformError> {
    let mut budget = LegacyBudget::new();
    collect_public_legacy_inventory_with(public_tree, &mut budget)
}

pub(super) fn legacy_identity_digest<'a>(
    launcher: &LinuxExactEntry,
    launcher_bytes: &[u8],
    layout: impl IntoIterator<Item = (&'a LinuxLayoutItem, &'a LinuxExactEntry)>,
    inventory: &[LinuxLegacyFile],
) -> Result<String, InstallPlatformError> {
    let mut descriptors = LegacyDescriptors::new();
    match launcher {
        LinuxExactEntry::Absent => {}
        LinuxExactEntry::RegularFile { mode, sha256, .. } => {
            if *sha256 != format!("{:x}", Sha256::digest(launcher_bytes)) {
                return Err(error("legacy launcher identity changed"));
            }
            descriptors.insert(
                "launcher/hypercolor.service".to_owned(),
                (*mode, sha256.clone()),
            );
        }
        LinuxExactEntry::Symlink { .. } => {
            return Err(error("legacy launcher identity is not a regular file"));
        }
    }
    for (item, entry) in layout {
        match entry {
            LinuxExactEntry::Absent => {}
            LinuxExactEntry::RegularFile { mode, sha256, .. } => {
                insert_descriptor(
                    &mut descriptors,
                    item.unit_path().to_owned(),
                    *mode,
                    sha256.clone(),
                )?;
            }
            LinuxExactEntry::Symlink { .. } => {
                return Err(error("legacy layout identity contains a symbolic link"));
            }
        }
    }
    for file in inventory {
        insert_descriptor(
            &mut descriptors,
            file.path.clone(),
            file.mode,
            format!("{:x}", Sha256::digest(&file.contents)),
        )?;
    }
    serde_json::to_vec(&descriptors)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|source| error(source.to_string()))
}

fn insert_descriptor(
    descriptors: &mut LegacyDescriptors,
    path: String,
    mode: u32,
    digest: String,
) -> Result<(), InstallPlatformError> {
    if path.is_empty()
        || path.len() > MAX_LEGACY_PATH_BYTES
        || path.starts_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || descriptors.insert(path, (mode, digest)).is_some()
    {
        return Err(error(
            "legacy identity contains a noncanonical or duplicate path",
        ));
    }
    Ok(())
}

pub(super) fn collect_public_legacy_inventory_with(
    public_tree: &LinuxPublicTree,
    budget: &mut LegacyBudget,
) -> Result<Vec<LinuxLegacyFile>, InstallPlatformError> {
    let mut files = Vec::new();
    for (parent, name, path) in [
        (LinuxDirectoryItem::LocalBin, "hyper", "bin/hyper"),
        (
            LinuxDirectoryItem::LocalBin,
            "hypercolor-tray",
            "bin/hypercolor-tray",
        ),
        (
            LinuxDirectoryItem::BashCompletions,
            "hyper",
            "share/bash-completion/completions/hyper",
        ),
        (
            LinuxDirectoryItem::ZshSiteFunctions,
            "_hyper",
            "share/zsh/site-functions/_hyper",
        ),
    ] {
        collect_named_layout_file(public_tree, parent, name, path, budget, &mut files)?;
    }
    if public_tree.state(LinuxDirectoryItem::LocalShare)?
        == super::model::LinuxDirectoryState::Present
        && let Some(ui) = public_tree.open_optional_relative_directory(
            LinuxDirectoryItem::LocalShare,
            &["hypercolor", "ui"],
        )?
    {
        collect_public_tree(ui, "share/hypercolor/ui", 0, true, budget, &mut files)?;
    }
    if public_tree.state(LinuxDirectoryItem::Icons)? == super::model::LinuxDirectoryState::Present {
        collect_public_tree(
            public_tree.open_directory(LinuxDirectoryItem::Icons)?,
            "share/icons",
            0,
            false,
            budget,
            &mut files,
        )?;
    }
    if public_tree.state(LinuxDirectoryItem::Config)? == super::model::LinuxDirectoryState::Present
        && let Some(fish) = public_tree.open_optional_relative_directory(
            LinuxDirectoryItem::Config,
            &["fish", "completions"],
        )?
    {
        collect_named_public_file(
            &fish,
            Path::new("hypercolor.fish"),
            "home/.config/fish/completions/hypercolor.fish",
            1,
            budget,
            &mut files,
        )?;
        collect_named_public_file(
            &fish,
            Path::new("hyper.fish"),
            "home/.config/fish/completions/hyper.fish",
            5,
            budget,
            &mut files,
        )?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_named_layout_file(
    public_tree: &LinuxPublicTree,
    parent: LinuxDirectoryItem,
    name: &str,
    path: &str,
    budget: &mut LegacyBudget,
    files: &mut Vec<LegacyFile>,
) -> Result<(), InstallPlatformError> {
    if public_tree.state(parent)? == super::model::LinuxDirectoryState::Present {
        collect_named_public_file(
            &public_tree.open_directory(parent)?,
            Path::new(name),
            path,
            path.split('/').count(),
            budget,
            files,
        )?;
    }
    Ok(())
}

fn collect_public_tree(
    root: PublicDirectoryAuthority,
    root_path: &str,
    root_depth: usize,
    include_all_files: bool,
    budget: &mut LegacyBudget,
    files: &mut Vec<LegacyFile>,
) -> Result<(), InstallPlatformError> {
    let mut pending = vec![(root, root_path.to_owned(), root_depth)];
    while let Some((directory, prefix, depth)) = pending.pop() {
        for name in directory.child_names().map_err(io_error)? {
            let name = name
                .to_str()
                .ok_or_else(|| error("legacy public entry name is not UTF-8"))?;
            let path = format!("{prefix}/{name}");
            budget.member(depth + 1, &path)?;
            match directory.open_child_directory(Path::new(name)) {
                Ok(child) => pending.push((child, path, depth + 1)),
                Err(_) => match directory.observe_entry(Path::new(name)).map_err(io_error)? {
                    ExactEntry::RegularFile { .. }
                        if (include_all_files || is_owned_icon_name(name))
                            && !is_fixed_layout_path(&path) =>
                    {
                        collect_opened_public_file(
                            &directory,
                            Path::new(name),
                            path,
                            budget,
                            files,
                        )?;
                    }
                    ExactEntry::RegularFile { .. } | ExactEntry::Symlink { .. }
                        if is_fixed_layout_path(&path) => {}
                    ExactEntry::RegularFile { .. } | ExactEntry::Symlink { .. }
                        if !include_all_files && !is_owned_icon_name(name) => {}
                    ExactEntry::Absent => {
                        return Err(error("legacy public entry disappeared during traversal"));
                    }
                    _ => return Err(error("legacy owned public entry has an unsupported kind")),
                },
            }
        }
    }
    Ok(())
}

fn collect_named_public_file(
    directory: &PublicDirectoryAuthority,
    name: &Path,
    path: &str,
    depth: usize,
    budget: &mut LegacyBudget,
    files: &mut Vec<LegacyFile>,
) -> Result<(), InstallPlatformError> {
    match directory.observe_entry(name).map_err(io_error)? {
        ExactEntry::Absent => Ok(()),
        ExactEntry::RegularFile { .. } => {
            budget.member(depth, path)?;
            collect_opened_public_file(directory, name, path.to_owned(), budget, files)
        }
        ExactEntry::Symlink { .. } => {
            Err(error("legacy owned public entry has an unsupported kind"))
        }
    }
}

fn collect_opened_public_file(
    directory: &PublicDirectoryAuthority,
    name: &Path,
    path: String,
    budget: &mut LegacyBudget,
    files: &mut Vec<LegacyFile>,
) -> Result<(), InstallPlatformError> {
    let mut opened = directory.open_regular_file(name).map_err(io_error)?;
    let metadata = opened.metadata();
    let capacity = budget.file(metadata.size())?;
    let mut contents = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(metadata.size() + 1)
        .read_to_end(&mut contents)
        .map_err(io_error)?;
    if contents.len() != capacity {
        return Err(error("legacy public file changed size while reading"));
    }
    files.push(LegacyFile {
        path,
        mode: metadata.mode() & 0o7777,
        contents,
    });
    Ok(())
}

fn is_owned_icon_name(name: &str) -> bool {
    name == "hypercolor" || name.starts_with("hypercolor.") || name.starts_with("hypercolor-")
}

fn is_fixed_layout_path(path: &str) -> bool {
    super::model::LINUX_LAYOUT_ITEMS
        .into_iter()
        .any(|item| item.unit_path() == path)
}

pub(super) fn prepare_legacy_files(
    snapshot: &LinuxLegacySnapshot,
    public_tree: &LinuxPublicTree,
) -> Result<Vec<LegacyFile>, InstallPlatformError> {
    let mut budget = LegacyBudget::new();
    let mut files = collect_public_legacy_inventory_with(public_tree, &mut budget)?;
    if files != snapshot.inventory {
        return Err(error("legacy public inventory drifted before snapshot"));
    }
    if let Some(launcher) = &snapshot.launcher {
        budget.member(2, "launcher/hypercolor.service")?;
        budget.file(launcher.contents.len() as u64)?;
        files.push(LegacyFile {
            path: "launcher/hypercolor.service".to_owned(),
            mode: launcher.mode,
            contents: launcher.contents.clone(),
        });
    }
    for (item, expected) in &snapshot.layout {
        let LinuxExactEntry::RegularFile { mode, .. } = expected else {
            continue;
        };
        let directory = public_tree.open_directory(public_parent(*item))?;
        let path = item.unit_path();
        budget.member(path.split('/').count(), path)?;
        let mut opened = directory
            .open_regular_file(Path::new(public_name(*item)))
            .map_err(io_error)?;
        let metadata = opened.metadata();
        let capacity = budget.file(metadata.size())?;
        let mut contents = Vec::with_capacity(capacity);
        opened
            .file_mut()
            .take(metadata.size() + 1)
            .read_to_end(&mut contents)
            .map_err(io_error)?;
        if contents.len() != capacity {
            return Err(error("legacy public entry changed size during snapshot"));
        }
        let observed = LinuxExactEntry::RegularFile {
            mode: metadata.mode() & 0o7777,
            sha256: format!("{:x}", Sha256::digest(&contents)),
            snapshot_unit: None,
            snapshot_path: None,
        };
        if &observed != expected {
            return Err(error("legacy public entry drifted during snapshot"));
        }
        files.push(LegacyFile {
            path: path.to_owned(),
            mode: *mode,
            contents,
        });
    }
    if !snapshot.layout.iter().any(|(item, entry)| {
        *item == LinuxLayoutItem::HypercolorDaemon
            && matches!(entry, LinuxExactEntry::RegularFile { .. })
    }) {
        return Err(error(
            "legacy snapshot requires an exact regular daemon entry",
        ));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(error("legacy snapshot contains duplicate owned paths"));
    }
    let descriptors = files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                (file.mode, format!("{:x}", Sha256::digest(&file.contents))),
            )
        })
        .collect::<LegacyDescriptors>();
    let identity = format!(
        "legacy-{:x}",
        Sha256::digest(
            serde_json::to_vec(&descriptors).map_err(|source| error(source.to_string()))?
        )
    );
    if snapshot.unit.as_str() != identity {
        return Err(error(
            "legacy snapshot unit ID does not bind the complete inventory",
        ));
    }
    let manifest = serde_json::to_vec(&serde_json::json!({
        "name": "hypercolor-legacy-snapshot",
        "version": snapshot.version,
        "files": descriptors,
    }))
    .map_err(|source| error(source.to_string()))?;
    if manifest.len() > super::model::MAX_MANIFEST_BYTES {
        return Err(error("legacy snapshot manifest exceeds its byte bound"));
    }
    budget.member(1, "manifest.json")?;
    budget.file(manifest.len() as u64)?;
    files.push(LegacyFile {
        path: "manifest.json".to_owned(),
        mode: 0o644,
        contents: manifest,
    });
    Ok(files)
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
