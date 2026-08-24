use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::Path;

use hypercolor_platform_fs::{DirectoryAuthority, DirectoryEntryKind};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, UnitId};
use super::legacy::{LegacyBudget, LegacyFile};
use super::model::{MAX_LEGACY_PATH_BYTES, MAX_MANIFEST_BYTES, error};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyManifest {
    name: String,
    version: String,
    files: BTreeMap<String, (u32, String)>,
}

pub(super) fn populate_legacy_stage(
    root: &DirectoryAuthority,
    files: &[LegacyFile],
) -> Result<(), InstallPlatformError> {
    for file in files {
        write_tree_file(root, &file.path, file.mode, &file.contents)?;
    }
    Ok(())
}

pub(super) fn validate_legacy_unit(
    root: &DirectoryAuthority,
    expected: &[LegacyFile],
) -> Result<(), InstallPlatformError> {
    validate_legacy_unit_with_budget(root, expected, LegacyBudget::new())
}

pub(super) fn validate_legacy_snapshot_binding(
    root: &DirectoryAuthority,
    expected_unit: &UnitId,
) -> Result<(), InstallPlatformError> {
    if !expected_unit.as_str().starts_with("legacy-") {
        return Err(error("synthetic snapshot requires a legacy unit ID"));
    }
    let manifest_bytes = read_file(root, "manifest.json", MAX_MANIFEST_BYTES as u64)?;
    let manifest: LegacyManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|source| error(format!("invalid legacy snapshot manifest: {source}")))?;
    if manifest.name != "hypercolor-legacy-snapshot"
        || manifest.version.is_empty()
        || manifest.version.len() > 128
        || manifest.files.is_empty()
        || manifest.files.contains_key("manifest.json")
    {
        return Err(error("legacy snapshot manifest identity is invalid"));
    }
    for (path, (mode, digest)) in &manifest.files {
        if !canonical_relative_path(path)
            || *mode > 0o777
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(error("legacy snapshot manifest descriptor is invalid"));
        }
    }
    let identity = format!(
        "legacy-{:x}",
        Sha256::digest(
            serde_json::to_vec(&manifest.files).map_err(|source| error(source.to_string()))?
        )
    );
    if expected_unit.as_str() != identity {
        return Err(error("legacy snapshot manifest does not bind its unit ID"));
    }
    let mut actual_files = BTreeMap::new();
    let mut actual_directories = BTreeSet::new();
    collect_tree(
        root,
        &mut LegacyBudget::new(),
        &mut actual_directories,
        &mut actual_files,
    )?;
    let mut expected_files = manifest.files;
    expected_files.insert(
        "manifest.json".to_owned(),
        (0o644, format!("{:x}", Sha256::digest(&manifest_bytes))),
    );
    let expected_directories = expected_files
        .keys()
        .flat_map(|path| parent_paths(path))
        .collect::<BTreeSet<_>>();
    if actual_files != expected_files || actual_directories != expected_directories {
        return Err(error("legacy snapshot tree does not match its manifest"));
    }
    Ok(())
}

pub(super) fn validate_legacy_unit_with_budget(
    root: &DirectoryAuthority,
    expected: &[LegacyFile],
    mut budget: LegacyBudget,
) -> Result<(), InstallPlatformError> {
    let mut actual_files = BTreeMap::new();
    let mut actual_directories = BTreeSet::new();
    collect_tree(
        root,
        &mut budget,
        &mut actual_directories,
        &mut actual_files,
    )?;
    let expected_files = expected
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                (file.mode, format!("{:x}", Sha256::digest(&file.contents))),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected_directories = expected
        .iter()
        .flat_map(|file| parent_paths(&file.path))
        .collect::<BTreeSet<_>>();
    if actual_files != expected_files || actual_directories != expected_directories {
        return Err(error(
            "existing legacy snapshot is incomplete, stale, or contains extras",
        ));
    }
    Ok(())
}

fn collect_tree(
    root: &DirectoryAuthority,
    budget: &mut LegacyBudget,
    directories: &mut BTreeSet<String>,
    files: &mut BTreeMap<String, (u32, String)>,
) -> Result<(), InstallPlatformError> {
    let mut pending = Vec::new();
    scan_directory(root, "", 0, budget, directories, files, &mut pending)?;
    while let Some((directory, prefix, depth)) = pending.pop() {
        scan_directory(
            &directory,
            &prefix,
            depth,
            budget,
            directories,
            files,
            &mut pending,
        )?;
    }
    Ok(())
}

fn scan_directory(
    root: &DirectoryAuthority,
    prefix: &str,
    depth: usize,
    budget: &mut LegacyBudget,
    directories: &mut BTreeSet<String>,
    files: &mut BTreeMap<String, (u32, String)>,
    pending: &mut Vec<(DirectoryAuthority, String, usize)>,
) -> Result<(), InstallPlatformError> {
    for name in root.child_names().map_err(io_error)? {
        let name = name
            .to_str()
            .ok_or_else(|| error("legacy snapshot entry name is not UTF-8"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        budget.member(depth + 1, &path)?;
        let metadata = root
            .entry_metadata(Path::new(name))
            .map_err(io_error)?
            .ok_or_else(|| error("legacy snapshot entry disappeared"))?;
        match metadata.kind() {
            DirectoryEntryKind::Directory => {
                directories.insert(path.clone());
                let child = root
                    .open_child_directory(Path::new(name))
                    .map_err(io_error)?;
                pending.push((child, path, depth + 1));
            }
            DirectoryEntryKind::RegularFile => {
                let mut file = root.open_regular_file(Path::new(name)).map_err(io_error)?;
                let capacity = budget.file(metadata.size())?;
                let mut contents = Vec::with_capacity(capacity);
                file.file_mut()
                    .take(metadata.size() + 1)
                    .read_to_end(&mut contents)
                    .map_err(io_error)?;
                if contents.len() != capacity {
                    return Err(error("legacy snapshot file changed size while reading"));
                }
                if files
                    .insert(
                        path,
                        (
                            metadata.mode() & 0o7777,
                            format!("{:x}", Sha256::digest(&contents)),
                        ),
                    )
                    .is_some()
                {
                    return Err(error("legacy snapshot contains duplicate file paths"));
                }
            }
            _ => return Err(error("legacy snapshot contains an unsupported entry type")),
        }
    }
    Ok(())
}

fn parent_paths(path: &str) -> Vec<String> {
    let components = path.split('/').collect::<Vec<_>>();
    (1..components.len())
        .map(|length| components[..length].join("/"))
        .collect()
}

fn canonical_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_LEGACY_PATH_BYTES
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn read_file(
    root: &DirectoryAuthority,
    name: &str,
    max: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let mut opened = root.open_regular_file(Path::new(name)).map_err(io_error)?;
    let size = opened.metadata().size();
    if size > max {
        return Err(error("legacy snapshot file exceeds its byte bound"));
    }
    let capacity =
        usize::try_from(size).map_err(|_| error("legacy snapshot file does not fit in memory"))?;
    let mut bytes = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() != capacity {
        return Err(error("legacy snapshot file changed size while reading"));
    }
    Ok(bytes)
}

fn write_tree_file(
    root: &DirectoryAuthority,
    relative: &str,
    mode: u32,
    contents: &[u8],
) -> Result<(), InstallPlatformError> {
    let components = relative.split('/').collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .ok_or_else(|| error("legacy snapshot file path is empty"))?;
    let mut directory = None;
    for component in parents {
        let parent = directory.as_ref().unwrap_or(root);
        let child = match parent
            .entry_metadata(Path::new(component))
            .map_err(io_error)?
        {
            None => parent
                .create_child_directory(Path::new(component))
                .map_err(io_error)?,
            Some(metadata) if metadata.kind() == DirectoryEntryKind::Directory => parent
                .open_child_directory(Path::new(component))
                .map_err(io_error)?,
            Some(_) => return Err(error("legacy snapshot parent is not a directory")),
        };
        directory = Some(child);
    }
    let parent = directory.as_ref().unwrap_or(root);
    let mut source = contents;
    parent
        .create_regular_file(Path::new(name), mode, contents.len() as u64, &mut source)
        .map_err(io_error)?;
    Ok(())
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
