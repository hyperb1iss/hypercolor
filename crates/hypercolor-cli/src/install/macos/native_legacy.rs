use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use hypercolor_platform_fs::{DirectoryAuthority, DirectoryEntryKind};
use serde::{Deserialize, Serialize};

use super::super::{InstallPlatformError, UnitRecord};
use super::model::{
    MAX_LAUNCHER_BYTES, MAX_LEGACY_DEPTH, MAX_LEGACY_FILE_BYTES, MAX_LEGACY_MEMBERS,
    MAX_PUBLIC_PATH_BYTES, MacosExactEntry, MacosLegacyExecutable, MacosLegacySnapshot, error,
    hex_digest, is_sha256, public_path_depth, validate_public_path,
};
use super::native_legacy_identity::{
    MacosLegacyExecutableRecord, executable_record, has_valid_cdhash, legacy_executable,
    stable_executable_matches,
};
use super::native_legacy_tree::{collect_tree, parent_paths, read_file};

const INDEX_FILE: &str = "legacy-snapshot.json";
const MANIFEST_FILE: &str = "manifest.json";
const DAEMON_FILE: &str = "bin/hypercolor-daemon";
const LAUNCHER_FILE: &str = "launchd/prior.plist";
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyIndex {
    schema_version: u32,
    unit: super::super::UnitId,
    version: String,
    executable: MacosLegacyExecutableRecord,
    launcher: Option<StoredFile>,
    entries: BTreeMap<String, StoredEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFile {
    relative_path: String,
    mode: u32,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum StoredEntry {
    Absent,
    Symlink { target: String },
    RegularFile { file: StoredFile },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyManifest {
    name: String,
    version: String,
    unit: super::super::UnitId,
    index_sha256: String,
}

pub(super) fn snapshot_legacy_unit(
    units: &DirectoryAuthority,
    units_root_hint: &Path,
    snapshot: &MacosLegacySnapshot,
    daemon_bytes: &[u8],
) -> Result<UnitRecord, InstallPlatformError> {
    validate_snapshot_source(snapshot, daemon_bytes)?;
    match units.open_child_directory(Path::new(snapshot.unit.as_str())) {
        Ok(existing) => {
            let record = retained(existing, units_root_hint, snapshot.unit.clone())?;
            validate_legacy_snapshot(
                &record,
                &snapshot.executable,
                &launcher_entry(snapshot),
                snapshot
                    .launcher
                    .as_ref()
                    .map_or(&[][..], |launcher| launcher.contents.as_slice()),
                &snapshot.entries.iter().cloned().collect(),
            )?;
            return Ok(record);
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error(source)),
    }
    let index = build_index(snapshot)?;
    let index_bytes = serde_json::to_vec(&index).map_err(json_error)?;
    if index_bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(error("macOS legacy snapshot index exceeds its byte bound"));
    }
    let manifest = LegacyManifest {
        name: "hypercolor-macos-legacy-snapshot".to_owned(),
        version: snapshot.version.clone(),
        unit: snapshot.unit.clone(),
        index_sha256: hex_digest(&index_bytes),
    };
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(json_error)?;
    let stage_name = format!(".hypercolor-stage-macos-{}", snapshot.unit.as_str());
    let stage = units
        .create_private_staging_directory(Path::new(&stage_name))
        .map_err(io_error)?;
    let result = (|| {
        write_file(stage.directory(), MANIFEST_FILE, 0o644, &manifest_bytes)?;
        write_file(stage.directory(), INDEX_FILE, 0o644, &index_bytes)?;
        write_file(
            stage.directory(),
            DAEMON_FILE,
            snapshot.executable.mode,
            daemon_bytes,
        )?;
        if let Some(launcher) = &snapshot.launcher {
            write_file(
                stage.directory(),
                LAUNCHER_FILE,
                launcher.mode,
                &launcher.contents,
            )?;
        }
        for file in &snapshot.regular_files {
            let stored = index
                .entries
                .get(&file.path)
                .and_then(|entry| match entry {
                    StoredEntry::RegularFile { file } => Some(file),
                    StoredEntry::Absent | StoredEntry::Symlink { .. } => None,
                })
                .ok_or_else(|| error("macOS legacy file is absent from its strict index"))?;
            write_file(
                stage.directory(),
                &stored.relative_path,
                file.mode,
                &file.contents,
            )?;
        }
        validate_tree(
            &stage.directory().read_only().map_err(io_error)?,
            &index,
            &manifest_bytes,
            &index_bytes,
        )?;
        Ok(())
    })();
    if let Err(source) = result {
        return match stage.remove() {
            Ok(()) => Err(source),
            Err(cleanup) => Err(error(format!(
                "{source}; cleanup of private macOS legacy stage failed: {cleanup}"
            ))),
        };
    }
    let published = stage
        .publish_or_remove(Path::new(snapshot.unit.as_str()))
        .map_err(io_error)?;
    let record = retained(published, units_root_hint, snapshot.unit.clone())?;
    validate_legacy_snapshot(
        &record,
        &snapshot.executable,
        &launcher_entry(snapshot),
        snapshot
            .launcher
            .as_ref()
            .map_or(&[][..], |launcher| launcher.contents.as_slice()),
        &snapshot.entries.iter().cloned().collect(),
    )?;
    Ok(record)
}

pub(super) fn validate_legacy_snapshot(
    unit: &UnitRecord,
    executable: &MacosLegacyExecutable,
    launcher: &MacosExactEntry,
    launcher_bytes: &[u8],
    entries: &BTreeMap<String, MacosExactEntry>,
) -> Result<(), InstallPlatformError> {
    if !unit.id().as_str().starts_with("legacy-") {
        return Err(error("macOS synthetic snapshot has a non-legacy unit ID"));
    }
    let manifest_bytes = read_file(unit.directory(), MANIFEST_FILE, MAX_INDEX_BYTES)?;
    let index_bytes = read_file(unit.directory(), INDEX_FILE, MAX_INDEX_BYTES)?;
    let manifest: LegacyManifest = serde_json::from_slice(&manifest_bytes).map_err(json_error)?;
    let index: LegacyIndex = serde_json::from_slice(&index_bytes).map_err(json_error)?;
    validate_index_storage(unit.directory(), &index)?;
    if manifest.name != "hypercolor-macos-legacy-snapshot"
        || manifest.unit != *unit.id()
        || manifest.version != executable.version
        || manifest.index_sha256 != hex_digest(&index_bytes)
        || index.schema_version != 2
        || index.unit != *unit.id()
        || index.version != executable.version
        || !stable_executable_matches(&index.executable, executable)
        || stored_launcher_matches(&index, launcher, launcher_bytes).is_err()
        || !stored_entries_match(&index.entries, entries)
    {
        return Err(error("macOS synthetic snapshot binding is inconsistent"));
    }
    let mut regular_bytes = BTreeMap::new();
    for (path, entry) in &index.entries {
        if let StoredEntry::RegularFile { file } = entry {
            let bytes = read_file(unit.directory(), &file.relative_path, file.size)?;
            if bytes.len() as u64 != file.size || hex_digest(&bytes) != file.sha256 {
                return Err(error("macOS synthetic snapshot regular bytes changed"));
            }
            regular_bytes.insert(path.clone(), bytes);
        }
    }
    let identity = super::legacy::legacy_identity_parts(
        launcher,
        launcher_bytes,
        entries,
        &regular_bytes,
        Some(executable),
    )?;
    if unit.id().as_str() != format!("legacy-{identity}") {
        return Err(error("macOS synthetic snapshot does not bind its unit ID"));
    }
    validate_tree(unit.directory(), &index, &manifest_bytes, &index_bytes)
}

pub(super) fn validate_legacy_snapshot_binding(
    root: &hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
    expected_unit: &super::super::UnitId,
) -> Result<(), InstallPlatformError> {
    if !expected_unit.as_str().starts_with("legacy-") {
        return Err(error("macOS synthetic snapshot has a non-legacy unit ID"));
    }
    let manifest_bytes = read_file(root, MANIFEST_FILE, MAX_INDEX_BYTES)?;
    let index_bytes = read_file(root, INDEX_FILE, MAX_INDEX_BYTES)?;
    let manifest: LegacyManifest = serde_json::from_slice(&manifest_bytes).map_err(json_error)?;
    let index: LegacyIndex = serde_json::from_slice(&index_bytes).map_err(json_error)?;
    validate_index_storage(root, &index)?;
    if manifest.name != "hypercolor-macos-legacy-snapshot"
        || manifest.unit != *expected_unit
        || index.unit != *expected_unit
        || index.schema_version != 2
        || manifest.version != index.version
        || manifest.index_sha256 != hex_digest(&index_bytes)
    {
        return Err(error("macOS synthetic snapshot manifest is not self-bound"));
    }
    let executable = legacy_executable(&index.executable);
    let mut entries = BTreeMap::new();
    let mut regular_bytes = BTreeMap::new();
    for (path, stored) in &index.entries {
        let entry = match stored {
            StoredEntry::Absent => MacosExactEntry::Absent,
            StoredEntry::Symlink { target } => MacosExactEntry::Symlink {
                target: target.clone(),
            },
            StoredEntry::RegularFile { file } => {
                let bytes = read_file(root, &file.relative_path, file.size)?;
                if bytes.len() as u64 != file.size || hex_digest(&bytes) != file.sha256 {
                    return Err(error("macOS synthetic snapshot regular bytes changed"));
                }
                regular_bytes.insert(path.clone(), bytes);
                MacosExactEntry::RegularFile {
                    mode: file.mode,
                    sha256: file.sha256.clone(),
                    snapshot_unit: None,
                    snapshot_path: None,
                }
            }
        };
        entries.insert(path.clone(), entry);
    }
    let (launcher, launcher_bytes) = match &index.launcher {
        None => (MacosExactEntry::Absent, Vec::new()),
        Some(file) => {
            let bytes = read_file(root, &file.relative_path, file.size)?;
            if bytes.len() as u64 != file.size || hex_digest(&bytes) != file.sha256 {
                return Err(error("macOS synthetic launcher snapshot changed"));
            }
            (
                MacosExactEntry::RegularFile {
                    mode: file.mode,
                    sha256: file.sha256.clone(),
                    snapshot_unit: None,
                    snapshot_path: None,
                },
                bytes,
            )
        }
    };
    let identity = super::legacy::legacy_identity_parts(
        &launcher,
        &launcher_bytes,
        &entries,
        &regular_bytes,
        Some(&executable),
    )?;
    if expected_unit.as_str() != format!("legacy-{identity}") {
        return Err(error("macOS synthetic snapshot does not bind its unit ID"));
    }
    validate_tree(root, &index, &manifest_bytes, &index_bytes)
}

pub(super) fn read_snapshot_file(
    unit: &UnitRecord,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let index_bytes = read_file(unit.directory(), INDEX_FILE, MAX_INDEX_BYTES)?;
    let index: LegacyIndex = serde_json::from_slice(&index_bytes).map_err(json_error)?;
    let file = index
        .entries
        .get(path)
        .and_then(|entry| match entry {
            StoredEntry::RegularFile { file } => Some(file),
            StoredEntry::Absent | StoredEntry::Symlink { .. } => None,
        })
        .ok_or_else(|| error("macOS synthetic snapshot lacks the requested regular file"))?;
    let bytes = read_file(
        unit.directory(),
        &file.relative_path,
        max_bytes.min(file.size),
    )?;
    if bytes.len() as u64 != file.size || hex_digest(&bytes) != file.sha256 {
        return Err(error("macOS synthetic snapshot file changed"));
    }
    Ok(bytes)
}

fn build_index(snapshot: &MacosLegacySnapshot) -> Result<LegacyIndex, InstallPlatformError> {
    let regular = snapshot
        .regular_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut entries = BTreeMap::new();
    for (position, (path, entry)) in snapshot.entries.iter().enumerate() {
        let stored = match entry {
            MacosExactEntry::Absent => StoredEntry::Absent,
            MacosExactEntry::Symlink { target } => StoredEntry::Symlink {
                target: target.clone(),
            },
            MacosExactEntry::RegularFile { mode, sha256, .. } => {
                let file = regular
                    .get(path.as_str())
                    .ok_or_else(|| error("macOS legacy regular entry lacks exact bytes"))?;
                if file.mode != *mode || hex_digest(&file.contents) != *sha256 {
                    return Err(error("macOS legacy regular entry bytes are inconsistent"));
                }
                StoredEntry::RegularFile {
                    file: StoredFile {
                        relative_path: format!("public/{position:05}.bin"),
                        mode: *mode,
                        sha256: sha256.clone(),
                        size: file.contents.len() as u64,
                    },
                }
            }
        };
        if entries.insert(path.clone(), stored).is_some() {
            return Err(error("macOS legacy snapshot contains duplicate paths"));
        }
    }
    if regular.len()
        != entries
            .values()
            .filter(|entry| matches!(entry, StoredEntry::RegularFile { .. }))
            .count()
    {
        return Err(error(
            "macOS legacy snapshot contains unbound regular bytes",
        ));
    }
    Ok(LegacyIndex {
        schema_version: 2,
        unit: snapshot.unit.clone(),
        version: snapshot.version.clone(),
        executable: executable_record(&snapshot.executable),
        launcher: snapshot.launcher.as_ref().map(|launcher| StoredFile {
            relative_path: LAUNCHER_FILE.to_owned(),
            mode: launcher.mode,
            sha256: hex_digest(&launcher.contents),
            size: launcher.contents.len() as u64,
        }),
        entries,
    })
}

fn validate_snapshot_source(
    snapshot: &MacosLegacySnapshot,
    daemon_bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    if daemon_bytes.len() as u64 != snapshot.executable.size
        || hex_digest(daemon_bytes) != snapshot.executable.sha256
        || snapshot.version.is_empty()
        || snapshot.version.len() > 128
        || snapshot.entries.len() > MAX_LEGACY_MEMBERS
    {
        return Err(error(
            "macOS legacy executable or inventory is inconsistent",
        ));
    }
    build_index(snapshot)?;
    let regular_bytes = snapshot
        .regular_files
        .iter()
        .map(|file| (file.path.clone(), file.contents.clone()))
        .collect::<BTreeMap<_, _>>();
    let identity = super::legacy::legacy_identity_parts(
        &launcher_entry(snapshot),
        snapshot
            .launcher
            .as_ref()
            .map_or(&[][..], |launcher| launcher.contents.as_slice()),
        &snapshot.entries.iter().cloned().collect(),
        &regular_bytes,
        Some(&snapshot.executable),
    )?;
    if snapshot.unit.as_str() != format!("legacy-{identity}") {
        return Err(error("macOS legacy snapshot unit identity changed"));
    }
    Ok(())
}

fn validate_tree(
    root: &hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
    index: &LegacyIndex,
    manifest_bytes: &[u8],
    index_bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    let mut expected = BTreeMap::from([
        (
            MANIFEST_FILE.to_owned(),
            (0o644, hex_digest(manifest_bytes)),
        ),
        (INDEX_FILE.to_owned(), (0o644, hex_digest(index_bytes))),
        (
            DAEMON_FILE.to_owned(),
            (index.executable.mode, index.executable.sha256.clone()),
        ),
    ]);
    if let Some(launcher) = &index.launcher {
        expected.insert(
            launcher.relative_path.clone(),
            (launcher.mode, launcher.sha256.clone()),
        );
    }
    for entry in index.entries.values() {
        if let StoredEntry::RegularFile { file } = entry {
            expected.insert(file.relative_path.clone(), (file.mode, file.sha256.clone()));
        }
    }
    let (actual_files, actual_directories) = collect_tree(root)?;
    let expected_directories = expected
        .keys()
        .flat_map(|path| parent_paths(path))
        .chain(std::iter::once(String::new()))
        .map(|path| (path, 0o700))
        .collect::<BTreeMap<_, _>>();
    if actual_files != expected || actual_directories != expected_directories {
        return Err(error(
            "macOS synthetic snapshot is incomplete, stale, or has extras",
        ));
    }
    Ok(())
}

fn validate_index_storage(
    root: &hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
    index: &LegacyIndex,
) -> Result<(), InstallPlatformError> {
    let executable = &index.executable;
    validate_public_path(&executable.path)?;
    if index.version != executable.version
        || executable.version.is_empty()
        || executable.version.len() > 128
        || executable.size == 0
        || executable.size > MAX_LEGACY_FILE_BYTES
        || executable.mode > 0o777
        || executable.mode & 0o100 == 0
        || executable.mode & 0o022 != 0
        || executable.device == 0
        || executable.inode == 0
        || !is_sha256(&executable.sha256)
        || executable.designated_requirement.is_empty()
        || executable.designated_requirement.len() > 8 * 1024
        || !is_sha256(&executable.designated_requirement_sha256)
        || !has_valid_cdhash(executable)
        || hex_digest(executable.designated_requirement.as_bytes())
            != executable.designated_requirement_sha256
    {
        return Err(error("macOS synthetic executable index is malformed"));
    }
    let daemon = read_file(root, DAEMON_FILE, executable.size)?;
    if daemon.len() as u64 != executable.size || hex_digest(&daemon) != executable.sha256 {
        return Err(error("macOS synthetic daemon bytes changed"));
    }
    if let Some(launcher) = &index.launcher
        && (launcher.relative_path != LAUNCHER_FILE
            || launcher.mode > 0o777
            || launcher.mode & 0o111 != 0
            || launcher.size == 0
            || launcher.size > MAX_LAUNCHER_BYTES as u64
            || !is_sha256(&launcher.sha256))
    {
        return Err(error("macOS synthetic launcher index is malformed"));
    }
    if index.entries.len() > MAX_LEGACY_MEMBERS {
        return Err(error(
            "macOS synthetic entry index exceeds its member bound",
        ));
    }
    for (position, (path, entry)) in index.entries.iter().enumerate() {
        validate_public_path(path)?;
        if public_path_depth(path) > MAX_LEGACY_DEPTH {
            return Err(error("macOS synthetic entry path exceeds its depth bound"));
        }
        match entry {
            StoredEntry::Absent => {}
            StoredEntry::Symlink { target } => {
                if target.is_empty()
                    || target.len() > MAX_PUBLIC_PATH_BYTES
                    || target.as_bytes().contains(&0)
                {
                    return Err(error("macOS synthetic symlink target is malformed"));
                }
            }
            StoredEntry::RegularFile { file } => {
                if file.relative_path != format!("public/{position:05}.bin")
                    || file.mode > 0o777
                    || file.size > MAX_LEGACY_FILE_BYTES
                    || !is_sha256(&file.sha256)
                {
                    return Err(error("macOS synthetic regular index is not canonical"));
                }
            }
        }
    }
    Ok(())
}

fn write_file(
    root: &DirectoryAuthority,
    relative: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    let parts = relative.split('/').collect::<Vec<_>>();
    let (name, parents) = parts
        .split_last()
        .ok_or_else(|| error("macOS synthetic snapshot path is empty"))?;
    let mut directory = None;
    for parent_name in parents {
        let parent = directory.as_ref().unwrap_or(root);
        let child = match parent
            .entry_metadata(Path::new(parent_name))
            .map_err(io_error)?
        {
            None => parent
                .create_child_directory(Path::new(parent_name))
                .map_err(io_error)?,
            Some(metadata) if metadata.kind() == DirectoryEntryKind::Directory => parent
                .open_child_directory(Path::new(parent_name))
                .map_err(io_error)?,
            Some(_) => return Err(error("macOS synthetic snapshot parent is not a directory")),
        };
        directory = Some(child);
    }
    directory
        .as_ref()
        .unwrap_or(root)
        .create_regular_file(
            Path::new(name),
            mode,
            bytes.len() as u64,
            &mut Cursor::new(bytes),
        )
        .map(|_| ())
        .map_err(io_error)
}

fn stored_launcher_matches(
    index: &LegacyIndex,
    launcher: &MacosExactEntry,
    bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    match (&index.launcher, launcher) {
        (None, MacosExactEntry::Absent) if bytes.is_empty() => Ok(()),
        (Some(file), MacosExactEntry::RegularFile { mode, sha256, .. })
            if file.mode == *mode
                && file.sha256 == *sha256
                && file.size == bytes.len() as u64
                && file.sha256 == hex_digest(bytes) =>
        {
            Ok(())
        }
        _ => Err(error("macOS synthetic launcher snapshot is inconsistent")),
    }
}

fn stored_entries_match(
    stored: &BTreeMap<String, StoredEntry>,
    entries: &BTreeMap<String, MacosExactEntry>,
) -> bool {
    stored.len() == entries.len()
        && stored.iter().all(|(path, stored)| {
            entries
                .get(path)
                .is_some_and(|entry| match (stored, entry) {
                    (StoredEntry::Absent, MacosExactEntry::Absent) => true,
                    (
                        StoredEntry::Symlink { target },
                        MacosExactEntry::Symlink { target: expected },
                    ) => target == expected,
                    (
                        StoredEntry::RegularFile { file },
                        MacosExactEntry::RegularFile { mode, sha256, .. },
                    ) => file.mode == *mode && file.sha256 == *sha256,
                    _ => false,
                })
        })
}

fn launcher_entry(snapshot: &MacosLegacySnapshot) -> MacosExactEntry {
    snapshot
        .launcher
        .as_ref()
        .map_or(MacosExactEntry::Absent, |file| {
            MacosExactEntry::RegularFile {
                mode: file.mode,
                sha256: hex_digest(&file.contents),
                snapshot_unit: None,
                snapshot_path: None,
            }
        })
}

fn retained(
    directory: DirectoryAuthority,
    root_hint: &Path,
    unit: super::super::UnitId,
) -> Result<UnitRecord, InstallPlatformError> {
    let read_only = directory.read_only().map_err(io_error)?;
    UnitRecord::new(unit.clone(), root_hint.join(unit.as_str()), read_only).map_err(io_error)
}

fn json_error(source: serde_json::Error) -> InstallPlatformError {
    error(source.to_string())
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
