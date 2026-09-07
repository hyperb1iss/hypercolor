use serde::{Deserialize, Serialize};

use super::model::{MacosLegacyExecutable, is_cdhash};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosLegacyExecutableRecord {
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) size: u64,
    pub(super) mode: u32,
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) designated_requirement: String,
    pub(super) designated_requirement_sha256: String,
    pub(super) cdhash: String,
    pub(super) version: String,
}

pub(super) fn executable_record(executable: &MacosLegacyExecutable) -> MacosLegacyExecutableRecord {
    MacosLegacyExecutableRecord {
        path: executable.path.clone(),
        sha256: executable.sha256.clone(),
        size: executable.size,
        mode: executable.mode,
        device: executable.device,
        inode: executable.inode,
        designated_requirement: executable.designated_requirement.clone(),
        designated_requirement_sha256: executable.designated_requirement_sha256.clone(),
        cdhash: executable.cdhash.clone(),
        version: executable.version.clone(),
    }
}

pub(super) fn stable_executable_matches(
    stored: &MacosLegacyExecutableRecord,
    current: &MacosLegacyExecutable,
) -> bool {
    current.device != 0
        && current.inode != 0
        && stored.path == current.path
        && stored.sha256 == current.sha256
        && stored.size == current.size
        && stored.mode == current.mode
        && stored.designated_requirement == current.designated_requirement
        && stored.designated_requirement_sha256 == current.designated_requirement_sha256
        && stored.cdhash == current.cdhash
        && stored.version == current.version
}

pub(super) fn legacy_executable(executable: &MacosLegacyExecutableRecord) -> MacosLegacyExecutable {
    MacosLegacyExecutable {
        path: executable.path.clone(),
        sha256: executable.sha256.clone(),
        size: executable.size,
        mode: executable.mode,
        device: executable.device,
        inode: executable.inode,
        designated_requirement: executable.designated_requirement.clone(),
        designated_requirement_sha256: executable.designated_requirement_sha256.clone(),
        cdhash: executable.cdhash.clone(),
        version: executable.version.clone(),
    }
}

pub(super) fn has_valid_cdhash(executable: &MacosLegacyExecutableRecord) -> bool {
    is_cdhash(&executable.cdhash)
}
