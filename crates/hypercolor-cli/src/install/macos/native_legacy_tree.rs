use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};

use super::super::InstallPlatformError;
use super::model::{
    MAX_LEGACY_DEPTH, MAX_LEGACY_FILE_BYTES, MAX_LEGACY_MEMBERS, MAX_LEGACY_TOTAL_BYTES, error,
    hex_digest,
};

type TreeFiles = BTreeMap<String, (u32, String)>;
type TreeDirectories = BTreeMap<String, u32>;

pub(super) fn collect_tree(
    root: &ReadOnlyDirectoryAuthority,
) -> Result<(TreeFiles, TreeDirectories), InstallPlatformError> {
    let mut files = BTreeMap::new();
    let mut directories =
        BTreeMap::from([(String::new(), root.metadata().map_err(io_error)?.mode())]);
    let mut budget = ScanBudget::default();
    scan_tree(root, "", 0, &mut budget, &mut files, &mut directories)?;
    Ok((files, directories))
}

#[derive(Default)]
struct ScanBudget {
    members: usize,
    bytes: u64,
}

fn scan_tree(
    directory: &ReadOnlyDirectoryAuthority,
    prefix: &str,
    depth: usize,
    budget: &mut ScanBudget,
    files: &mut BTreeMap<String, (u32, String)>,
    directories: &mut BTreeMap<String, u32>,
) -> Result<(), InstallPlatformError> {
    if depth > MAX_LEGACY_DEPTH {
        return Err(error("macOS synthetic snapshot exceeds its depth bound"));
    }
    for name in directory.child_names().map_err(io_error)? {
        budget.members = budget
            .members
            .checked_add(1)
            .ok_or_else(|| error("macOS synthetic snapshot member count overflowed"))?;
        if budget.members > MAX_LEGACY_MEMBERS {
            return Err(error("macOS synthetic snapshot exceeds its member bound"));
        }
        let name = name
            .to_str()
            .ok_or_else(|| error("macOS synthetic snapshot name is not exact UTF-8"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = directory
            .entry_metadata(Path::new(name))
            .map_err(io_error)?
            .ok_or_else(|| error("macOS synthetic snapshot member disappeared"))?;
        match metadata.kind() {
            DirectoryEntryKind::Directory => {
                directories.insert(path.clone(), metadata.mode());
                let child = directory
                    .open_child_directory(Path::new(name))
                    .map_err(io_error)?;
                scan_tree(&child, &path, depth + 1, budget, files, directories)?;
            }
            DirectoryEntryKind::RegularFile if metadata.link_count() == 1 => {
                if metadata.size() > MAX_LEGACY_FILE_BYTES {
                    return Err(error("macOS synthetic snapshot file exceeds its bound"));
                }
                budget.bytes = budget
                    .bytes
                    .checked_add(metadata.size())
                    .filter(|value| *value <= MAX_LEGACY_TOTAL_BYTES)
                    .ok_or_else(|| error("macOS synthetic snapshot exceeds aggregate bytes"))?;
                let bytes = read_named(directory, name, metadata.size())?;
                files.insert(path, (metadata.mode(), hex_digest(&bytes)));
            }
            _ => return Err(error("macOS synthetic snapshot contains an unsafe member")),
        }
    }
    Ok(())
}

pub(super) fn read_file(
    root: &ReadOnlyDirectoryAuthority,
    relative: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let parts = relative.split('/').collect::<Vec<_>>();
    let (name, parents) = parts
        .split_last()
        .ok_or_else(|| error("macOS synthetic snapshot path is empty"))?;
    let mut directory = None;
    for parent in parents {
        let child = directory
            .as_ref()
            .unwrap_or(root)
            .open_child_directory(Path::new(parent))
            .map_err(io_error)?;
        directory = Some(child);
    }
    let directory = directory.as_ref().unwrap_or(root);
    let opened = directory
        .open_regular_file(Path::new(name))
        .map_err(io_error)?;
    if opened.metadata().size() > max_bytes {
        return Err(error(
            "macOS synthetic snapshot read exceeds its byte bound",
        ));
    }
    read_named(directory, name, opened.metadata().size())
}

fn read_named(
    directory: &ReadOnlyDirectoryAuthority,
    name: &str,
    size: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let mut opened = directory
        .open_regular_file(Path::new(name))
        .map_err(io_error)?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(size).map_err(|_| error("macOS snapshot file does not fit in memory"))?,
    );
    opened
        .file_mut()
        .take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 != size {
        return Err(error("macOS snapshot file changed during bounded read"));
    }
    Ok(bytes)
}

pub(super) fn parent_paths(path: &str) -> Vec<String> {
    let parts = path.split('/').collect::<Vec<_>>();
    (1..parts.len())
        .map(|length| parts[..length].join("/"))
        .collect()
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
