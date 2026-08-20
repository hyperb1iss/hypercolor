use std::collections::BTreeSet;

use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, UnitRecord};
use super::MacosInspection;
use super::model::{
    MAX_LEGACY_FILE_BYTES, MAX_LEGACY_MEMBERS, MAX_LEGACY_TOTAL_BYTES, MacosExactEntry,
    MacosFilePublication, MacosLegacyFile, MacosLegacySnapshot, error,
};

pub(super) fn legacy_identity(
    inspection: &MacosInspection,
) -> Result<String, InstallPlatformError> {
    let mut hasher = Sha256::new();
    encode_entry(
        &mut hasher,
        "launcher",
        &inspection.launcher,
        Some(&inspection.launcher_bytes),
    )?;
    for (path, entry) in &inspection.public.entries {
        encode_entry(
            &mut hasher,
            path,
            entry,
            inspection.public.regular_bytes.get(path).map(Vec::as_slice),
        )?;
    }
    if let Some(executable) = &inspection.legacy_executable {
        hasher.update(b"executable\0");
        hasher.update(executable.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(executable.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(executable.designated_requirement_sha256.as_bytes());
    }
    Ok(super::model::hex_digest(&hasher.finalize()))
}

pub(super) fn build_legacy_snapshot(
    inspection: &MacosInspection,
    unit: super::super::UnitId,
) -> Result<MacosLegacySnapshot, InstallPlatformError> {
    let executable = inspection
        .legacy_executable
        .clone()
        .ok_or_else(|| error("legacy macOS install lacks an exact daemon identity"))?;
    let launcher = match &inspection.launcher {
        MacosExactEntry::RegularFile { mode, .. } => Some(MacosFilePublication {
            mode: *mode,
            contents: inspection.launcher_bytes.clone(),
        }),
        MacosExactEntry::Absent => None,
        MacosExactEntry::Symlink { .. } => {
            return Err(error("legacy macOS launchd plist is not a regular file"));
        }
    };
    let mut total = u64::try_from(inspection.launcher_bytes.len())
        .map_err(|_| error("legacy launcher size does not fit u64"))?;
    let mut regular_files = Vec::new();
    let mut paths = BTreeSet::new();
    for (path, entry) in &inspection.public.entries {
        if super::model::public_path_depth(path) > super::model::MAX_LEGACY_DEPTH {
            return Err(error("legacy macOS inventory exceeds its depth bound"));
        }
        if !paths.insert(path) || paths.len() > MAX_LEGACY_MEMBERS {
            return Err(error("legacy macOS inventory is duplicate or oversized"));
        }
        if let MacosExactEntry::RegularFile { mode, .. } = entry {
            let contents = inspection
                .public
                .regular_bytes
                .get(path)
                .ok_or_else(|| error("legacy macOS regular file lacks exact bytes"))?;
            let size = u64::try_from(contents.len())
                .map_err(|_| error("legacy macOS file size does not fit u64"))?;
            if size > MAX_LEGACY_FILE_BYTES {
                return Err(error("legacy macOS file exceeds its byte bound"));
            }
            total = total
                .checked_add(size)
                .filter(|total| *total <= MAX_LEGACY_TOTAL_BYTES)
                .ok_or_else(|| error("legacy macOS snapshot exceeds its aggregate byte bound"))?;
            regular_files.push(MacosLegacyFile {
                path: path.clone(),
                mode: *mode,
                contents: contents.clone(),
            });
        }
    }
    Ok(MacosLegacySnapshot {
        unit,
        version: executable.version.clone(),
        launcher,
        entries: inspection
            .public
            .entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.clone()))
            .collect(),
        regular_files,
        executable,
    })
}

pub(super) fn validate_legacy_unit_id(unit: &UnitRecord) -> Result<(), InstallPlatformError> {
    if !unit.id().as_str().starts_with("legacy-") {
        return Err(error("synthetic macOS snapshot has a non-legacy unit ID"));
    }
    Ok(())
}

fn encode_entry(
    hasher: &mut Sha256,
    path: &str,
    entry: &MacosExactEntry,
    contents: Option<&[u8]>,
) -> Result<(), InstallPlatformError> {
    hasher.update(path.as_bytes());
    hasher.update(b"\0");
    match entry {
        MacosExactEntry::Absent => hasher.update(b"absent\0"),
        MacosExactEntry::Symlink { target } => {
            hasher.update(b"symlink\0");
            hasher.update(target.as_bytes());
            hasher.update(b"\0");
        }
        MacosExactEntry::RegularFile { mode, sha256, .. } => {
            let contents =
                contents.ok_or_else(|| error("legacy macOS identity lacks regular file bytes"))?;
            if super::model::hex_digest(contents) != *sha256 {
                return Err(error("legacy macOS regular file digest is inconsistent"));
            }
            hasher.update(b"file\0");
            hasher.update(mode.to_le_bytes());
            hasher.update(contents);
            hasher.update(b"\0");
        }
    }
    Ok(())
}
