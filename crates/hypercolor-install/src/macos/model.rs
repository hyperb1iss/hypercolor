use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, UnitId};

pub(super) const MACOS_RECORD_SCHEMA_VERSION: u32 = 2;
pub(super) const MACOS_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub(super) const MAX_LAUNCHER_BYTES: usize = 32 * 1024;
pub(super) const MAX_PUBLIC_PATH_BYTES: usize = 4 * 1024;
pub(super) const MAX_LAYOUT_OPERATIONS: usize = 16 * 1024;
pub(super) const MAX_LEGACY_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_LEGACY_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MAX_LEGACY_DEPTH: usize = 32;
pub(super) const MAX_LEGACY_MEMBERS: usize = 16 * 1024;
pub(super) const DAEMON_RELATIVE_PATH: &str = "bin/hypercolor-daemon";
pub(super) const MANIFEST_RELATIVE_PATH: &str = "manifest.json";
pub(super) const LAUNCHER_MODE: u32 = 0o644;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosInstallConfig {
    pub direct_plist_path: String,
    pub immutable_units_root: PathBuf,
    pub active_root: PathBuf,
    pub log_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum MacosExactEntry {
    Absent,
    RegularFile {
        mode: u32,
        sha256: String,
        snapshot_unit: Option<UnitId>,
        snapshot_path: Option<String>,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosFilePublication {
    pub mode: u32,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosEntryPublication {
    RegularFile(MacosFilePublication),
    Symlink(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosMutationOutcome {
    Complete,
    SubmittedUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosLauncherSnapshot {
    pub snapshot_id: String,
    pub relative_path: String,
    pub content_sha256: String,
    pub mode: u32,
    pub size: u64,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosRuntimeExecutable {
    pub unit: UnitId,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
    pub designated_requirement: String,
    pub designated_requirement_sha256: String,
    pub cdhash: String,
    pub synthetic_legacy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosStopAuthority {
    pub owner_epoch: u64,
    pub audit_token_identity: String,
    pub executable_path: PathBuf,
    pub designated_requirement_hash: String,
    pub pid: u32,
    pub unit: UnitId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosRuntimeTransition {
    Stop {
        authority: MacosStopAuthority,
    },
    Start {
        executable: MacosRuntimeExecutable,
        launcher_snapshot: MacosLauncherSnapshot,
        after_epoch: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosDirectoryState {
    Absent,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosCandidateLayout {
    pub directories: Vec<String>,
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosPublicSnapshot {
    pub directories: BTreeMap<String, MacosDirectoryState>,
    pub entries: BTreeMap<String, MacosExactEntry>,
    pub regular_bytes: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MacosLegacyFile {
    pub path: String,
    pub mode: u32,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosLegacySnapshot {
    pub unit: UnitId,
    pub version: String,
    pub launcher: Option<MacosFilePublication>,
    pub entries: Vec<(String, MacosExactEntry)>,
    pub regular_files: Vec<MacosLegacyFile>,
    pub executable: MacosLegacyExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosLegacyExecutable {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
    pub designated_requirement: String,
    pub designated_requirement_sha256: String,
    pub cdhash: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosRecord {
    pub(super) candidate: MacosUnitBinding,
    pub(super) prior: Option<MacosUnitBinding>,
    pub(super) baseline_launchd: MacosLaunchdObservation,
    pub(super) baseline_owner_epoch: Option<u64>,
    pub(super) baseline_stop_authority: Option<MacosStopAuthority>,
    pub(super) prior_launcher: MacosExactEntry,
    pub(super) prior_launcher_bytes: String,
    pub(super) prior_launcher_snapshot: Option<MacosLauncherSnapshot>,
    pub(super) candidate_launcher: Option<MacosLauncher>,
    pub(super) candidate_launcher_snapshot: Option<MacosLauncherSnapshot>,
    pub(super) prior_directories: BTreeMap<String, MacosDirectoryState>,
    pub(super) prior_entries: BTreeMap<String, MacosExactEntry>,
    pub(super) layout: Vec<MacosLayoutOperation>,
    pub(super) first_conversion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosUnitBinding {
    pub(super) unit: UnitId,
    pub(super) daemon_path: String,
    pub(super) daemon_sha256: String,
    pub(super) daemon_size: u64,
    pub(super) daemon_mode: u32,
    pub(super) daemon_device: u64,
    pub(super) daemon_inode: u64,
    pub(super) designated_requirement: String,
    pub(super) designated_requirement_sha256: String,
    pub(super) cdhash: String,
    pub(super) version: String,
    pub(super) synthetic_legacy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosLauncher {
    pub(super) mode: u32,
    pub(super) bytes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosLayoutOperation {
    pub(super) effect: MacosLayoutEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(super) enum MacosLayoutEffect {
    Directory {
        path: String,
    },
    Entry {
        path: String,
        prior: MacosExactEntry,
        candidate_target: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosLaunchdObservation {
    pub pid: Option<u32>,
    pub autostart_enabled: bool,
}

impl MacosLaunchdObservation {
    pub(super) fn validate(&self) -> Result<(), InstallPlatformError> {
        if self.pid == Some(0) {
            return Err(error("launchd reported a zero process ID"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MacosOwnerReceipt {
    pub(super) owner_epoch: u64,
    pub(super) audit_token_identity: String,
    pub(super) executable_path: PathBuf,
    pub(super) designated_requirement_hash: String,
    pub(super) pid: u32,
    pub(super) unit: UnitId,
}

pub(super) fn validate_candidate_layout(
    layout: &MacosCandidateLayout,
) -> Result<(), InstallPlatformError> {
    if layout.directories.len() + layout.entries.len() > MAX_LAYOUT_OPERATIONS {
        return Err(error("macOS public projection exceeds its operation bound"));
    }
    let mut paths = BTreeSet::new();
    let mut prior = None;
    for path in &layout.directories {
        validate_public_path(path)?;
        if prior.is_some_and(|previous: &String| compare_directory_paths(previous, path).is_ge())
            || !paths.insert(path)
        {
            return Err(error("macOS public directories are not canonical"));
        }
        prior = Some(path);
    }
    prior = None;
    for (path, target) in &layout.entries {
        validate_public_path(path)?;
        validate_public_path(target)?;
        if prior.is_some_and(|previous: &String| previous >= path) || !paths.insert(path) {
            return Err(error("macOS public entries are not canonical"));
        }
        prior = Some(path);
    }
    Ok(())
}

pub(super) fn compare_directory_paths(left: &str, right: &str) -> std::cmp::Ordering {
    public_path_depth(left)
        .cmp(&public_path_depth(right))
        .then_with(|| left.cmp(right))
}

pub(super) fn public_path_depth(path: &str) -> usize {
    std::path::Path::new(path).components().count()
}

pub(super) fn validate_public_path(path: &str) -> Result<(), InstallPlatformError> {
    let path = std::path::Path::new(path);
    if path.as_os_str().len() > MAX_PUBLIC_PATH_BYTES
        || !path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(error(
            "macOS public path is not bounded canonical absolute UTF-8",
        ));
    }
    Ok(())
}

pub(super) fn entries_match(left: &MacosExactEntry, right: &MacosExactEntry) -> bool {
    match (left, right) {
        (MacosExactEntry::Absent, MacosExactEntry::Absent) => true,
        (
            MacosExactEntry::RegularFile {
                mode: left_mode,
                sha256: left_sha,
                ..
            },
            MacosExactEntry::RegularFile {
                mode: right_mode,
                sha256: right_sha,
                ..
            },
        ) => left_mode == right_mode && left_sha == right_sha,
        (
            MacosExactEntry::Symlink {
                target: left_target,
            },
            MacosExactEntry::Symlink {
                target: right_target,
            },
        ) => left_target == right_target,
        _ => false,
    }
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        })
}

pub(super) fn launcher_snapshot_id(mode: u32, bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"hypercolor-macos-launcher-v1\0");
    digest.update(mode.to_be_bytes());
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
            encoded
        })
}

pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn is_cdhash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn error(detail: impl Into<String>) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}
