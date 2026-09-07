use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, UnitId};
use super::systemd::parse_systemd_exec;

pub(super) const LINUX_RECORD_SCHEMA_VERSION: u32 = 1;
pub(super) const LINUX_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub(super) const MAX_SYSTEMD_SHOW_BYTES: usize = 16 * 1024;
pub(super) const MAX_LAUNCHER_BYTES: usize = 4 * 1024;
pub(super) const MAX_HTTP_RESPONSE_BYTES: usize = 64 * 1024;
pub(super) const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
pub(super) const DAEMON_RELATIVE_PATH: &str = "bin/hypercolor-daemon";
pub(super) const LAUNCHER_MODE: u32 = 0o644;
pub(super) const PUBLIC_DIRECTORY_MODE: u32 = 0o755;
pub(super) const MAX_LEGACY_DEPTH: usize = 16;
pub(super) const MAX_LEGACY_MEMBERS: usize = 4096;
pub(super) const MAX_LEGACY_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_LEGACY_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MAX_LEGACY_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxDirectoryItem {
    Local,
    LocalBin,
    LocalShare,
    Applications,
    BashCompletion,
    BashCompletions,
    Zsh,
    ZshSiteFunctions,
    Fish,
    FishVendorCompletions,
    Icons,
    IconsHicolor,
    Icon48,
    Icon48Apps,
    Icon128,
    Icon128Apps,
    Icon256,
    Icon256Apps,
    Config,
    Systemd,
    SystemdUser,
}

impl LinuxDirectoryItem {
    pub(super) fn parent(self) -> Option<Self> {
        match self {
            Self::Local | Self::Config => None,
            Self::LocalBin | Self::LocalShare => Some(Self::Local),
            Self::Applications | Self::BashCompletion | Self::Zsh | Self::Fish | Self::Icons => {
                Some(Self::LocalShare)
            }
            Self::BashCompletions => Some(Self::BashCompletion),
            Self::ZshSiteFunctions => Some(Self::Zsh),
            Self::FishVendorCompletions => Some(Self::Fish),
            Self::IconsHicolor => Some(Self::Icons),
            Self::Icon48 | Self::Icon128 | Self::Icon256 => Some(Self::IconsHicolor),
            Self::Icon48Apps => Some(Self::Icon48),
            Self::Icon128Apps => Some(Self::Icon128),
            Self::Icon256Apps => Some(Self::Icon256),
            Self::Systemd => Some(Self::Config),
            Self::SystemdUser => Some(Self::Systemd),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Local => ".local",
            Self::LocalBin => "bin",
            Self::LocalShare => "share",
            Self::Applications => "applications",
            Self::BashCompletion => "bash-completion",
            Self::BashCompletions => "completions",
            Self::Zsh => "zsh",
            Self::ZshSiteFunctions => "site-functions",
            Self::Fish => "fish",
            Self::FishVendorCompletions => "vendor_completions.d",
            Self::Icons => "icons",
            Self::IconsHicolor => "hicolor",
            Self::Icon48 => "48x48",
            Self::Icon48Apps | Self::Icon128Apps | Self::Icon256Apps => "apps",
            Self::Icon128 => "128x128",
            Self::Icon256 => "256x256",
            Self::Config => ".config",
            Self::Systemd => "systemd",
            Self::SystemdUser => "user",
        }
    }
}

pub const LINUX_DIRECTORY_ITEMS: [LinuxDirectoryItem; 21] = [
    LinuxDirectoryItem::Local,
    LinuxDirectoryItem::LocalBin,
    LinuxDirectoryItem::LocalShare,
    LinuxDirectoryItem::Applications,
    LinuxDirectoryItem::BashCompletion,
    LinuxDirectoryItem::BashCompletions,
    LinuxDirectoryItem::Zsh,
    LinuxDirectoryItem::ZshSiteFunctions,
    LinuxDirectoryItem::Fish,
    LinuxDirectoryItem::FishVendorCompletions,
    LinuxDirectoryItem::Icons,
    LinuxDirectoryItem::IconsHicolor,
    LinuxDirectoryItem::Icon48,
    LinuxDirectoryItem::Icon48Apps,
    LinuxDirectoryItem::Icon128,
    LinuxDirectoryItem::Icon128Apps,
    LinuxDirectoryItem::Icon256,
    LinuxDirectoryItem::Icon256Apps,
    LinuxDirectoryItem::Config,
    LinuxDirectoryItem::Systemd,
    LinuxDirectoryItem::SystemdUser,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinuxDirectoryState {
    Absent,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinuxLayoutItem {
    Hypercolor,
    HypercolorDaemon,
    HypercolorApp,
    HypercolorTui,
    HypercolorOpen,
    DesktopEntry,
    BashCompletion,
    ZshCompletion,
    FishCompletion,
    Icon48,
    Icon128,
    Icon256,
}

impl LinuxLayoutItem {
    pub(super) fn unit_path(self) -> &'static str {
        match self {
            Self::Hypercolor => "bin/hypercolor",
            Self::HypercolorDaemon => "bin/hypercolor-daemon",
            Self::HypercolorApp => "bin/hypercolor-app",
            Self::HypercolorTui => "bin/hypercolor-tui",
            Self::HypercolorOpen => "bin/hypercolor-open",
            Self::DesktopEntry => "share/applications/hypercolor.desktop",
            Self::BashCompletion => "share/bash-completion/completions/hypercolor",
            Self::ZshCompletion => "share/zsh/site-functions/_hypercolor",
            Self::FishCompletion => "share/fish/vendor_completions.d/hypercolor.fish",
            Self::Icon48 => "share/icons/hicolor/48x48/apps/hypercolor.png",
            Self::Icon128 => "share/icons/hicolor/128x128/apps/hypercolor.png",
            Self::Icon256 => "share/icons/hicolor/256x256/apps/hypercolor.png",
        }
    }
}

pub const LINUX_LAYOUT_ITEMS: [LinuxLayoutItem; 12] = [
    LinuxLayoutItem::Hypercolor,
    LinuxLayoutItem::HypercolorDaemon,
    LinuxLayoutItem::HypercolorApp,
    LinuxLayoutItem::HypercolorTui,
    LinuxLayoutItem::HypercolorOpen,
    LinuxLayoutItem::DesktopEntry,
    LinuxLayoutItem::BashCompletion,
    LinuxLayoutItem::ZshCompletion,
    LinuxLayoutItem::FishCompletion,
    LinuxLayoutItem::Icon48,
    LinuxLayoutItem::Icon128,
    LinuxLayoutItem::Icon256,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum LinuxExactEntry {
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
pub struct LinuxFilePublication {
    pub mode: u32,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxLayoutPublication {
    RegularFile(LinuxFilePublication),
    Symlink(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxProcessExecutable {
    pub path: String,
    pub sha256: String,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinuxLegacyFile {
    pub path: String,
    pub mode: u32,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxLegacySnapshot {
    pub unit: UnitId,
    pub version: String,
    pub launcher: Option<LinuxFilePublication>,
    pub layout: Vec<(LinuxLayoutItem, LinuxExactEntry)>,
    pub inventory: Vec<LinuxLegacyFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxInstallConfig {
    pub direct_fragment_path: String,
    pub immutable_units_root: PathBuf,
    pub active_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinuxRecord {
    pub(super) candidate: LinuxUnitBinding,
    pub(super) prior: Option<LinuxUnitBinding>,
    pub(super) baseline_systemd: LinuxSystemdObservation,
    pub(super) prior_launcher: LinuxExactEntry,
    pub(super) prior_launcher_bytes: Vec<u8>,
    pub(super) candidate_launcher: Option<LinuxLauncher>,
    pub(super) prior_directories: BTreeMap<LinuxDirectoryItem, LinuxDirectoryState>,
    pub(super) layout: Vec<LinuxLayoutOperation>,
    pub(super) first_conversion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinuxUnitBinding {
    pub(super) unit: UnitId,
    pub(super) daemon_path: String,
    pub(super) daemon_sha256: String,
    pub(super) daemon_size: u64,
    pub(super) daemon_device: u64,
    pub(super) daemon_inode: u64,
    pub(super) version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinuxLauncher {
    pub(super) mode: u32,
    pub(super) bytes: Vec<u8>,
    pub(super) exec_start: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinuxLayoutOperation {
    pub(super) effect: LinuxLayoutEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(super) enum LinuxLayoutEffect {
    Directory {
        item: LinuxDirectoryItem,
    },
    Entry {
        item: LinuxLayoutItem,
        prior: LinuxExactEntry,
        candidate_target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinuxOwnerReceipt {
    pub(super) invocation_id: String,
    pub(super) main_pid: u32,
    pub(super) unit: UnitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinuxSystemdObservation {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub fragment_path: String,
    pub exec_start: String,
    pub main_pid: u32,
    pub invocation_id: String,
}

pub fn parse_systemd_show(bytes: &[u8]) -> Result<LinuxSystemdObservation, InstallPlatformError> {
    if bytes.len() > MAX_SYSTEMD_SHOW_BYTES {
        return Err(error("systemctl show output exceeds its byte bound"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| error("systemctl show output is not UTF-8"))?;
    let expected: BTreeSet<&str> = [
        "LoadState",
        "ActiveState",
        "SubState",
        "UnitFileState",
        "FragmentPath",
        "ExecStart",
        "MainPID",
        "InvocationID",
    ]
    .into_iter()
    .collect();
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| error("malformed systemctl show field"))?;
        if !expected.contains(name) {
            return Err(error(format!("unknown systemctl show field {name}")));
        }
        if fields.insert(name, value).is_some() {
            return Err(error(format!("duplicate systemctl show field {name}")));
        }
    }
    // A nonexistent unit has no service interface, so systemctl omits ExecStart.
    if fields.get("LoadState") == Some(&"not-found") {
        fields.entry("ExecStart").or_insert("");
    }
    if fields.len() != expected.len() {
        return Err(error("systemctl show output is missing required fields"));
    }
    let main_pid = canonical_u32(fields["MainPID"])?;
    let exec_start = if fields["ExecStart"].is_empty() {
        String::new()
    } else {
        let parsed = parse_systemd_exec(fields["ExecStart"])?;
        if parsed.runtime_pid != main_pid {
            return Err(error("systemd ExecStart pid disagrees with MainPID"));
        }
        parsed.canonical_argv
    };
    let observation = LinuxSystemdObservation {
        load_state: fields["LoadState"].to_owned(),
        active_state: fields["ActiveState"].to_owned(),
        sub_state: fields["SubState"].to_owned(),
        unit_file_state: fields["UnitFileState"].to_owned(),
        fragment_path: fields["FragmentPath"].to_owned(),
        exec_start,
        main_pid,
        invocation_id: fields["InvocationID"].to_owned(),
    };
    observation.validate()?;
    Ok(observation)
}

impl LinuxSystemdObservation {
    pub(super) fn validate(&self) -> Result<(), InstallPlatformError> {
        if !matches!(self.load_state.as_str(), "loaded" | "not-found")
            || !matches!(self.active_state.as_str(), "active" | "inactive")
            || !matches!(self.sub_state.as_str(), "running" | "dead")
        {
            return Err(error("systemctl show reported an unsupported third state"));
        }
        if (self.load_state == "loaded"
            && !matches!(self.unit_file_state.as_str(), "enabled" | "disabled"))
            || (self.load_state == "not-found" && !self.unit_file_state.is_empty())
        {
            return Err(error(
                "systemctl unit-file state is inconsistent with load state",
            ));
        }
        let active = self.active_state == "active";
        if active != (self.sub_state == "running")
            || active != (self.main_pid != 0)
            || active && self.invocation_id.is_empty()
        {
            return Err(error("systemctl runtime fields are inconsistent"));
        }
        if !self.invocation_id.is_empty()
            && (self.invocation_id.len() != 32
                || !self
                    .invocation_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(error(
                "systemd InvocationID is not a canonical 128-bit token",
            ));
        }
        if self.load_state == "not-found"
            && (!self.fragment_path.is_empty() || !self.exec_start.is_empty() || active)
        {
            return Err(error("absent systemd unit has loaded fields"));
        }
        if self.load_state == "loaded"
            && (self.fragment_path.is_empty() || self.exec_start.is_empty())
        {
            return Err(error("loaded systemd unit lacks exact fragment identity"));
        }
        Ok(())
    }
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn entries_match(left: &LinuxExactEntry, right: &LinuxExactEntry) -> bool {
    match (left, right) {
        (LinuxExactEntry::Absent, LinuxExactEntry::Absent) => true,
        (
            LinuxExactEntry::RegularFile {
                mode: left_mode,
                sha256: left_sha256,
                ..
            },
            LinuxExactEntry::RegularFile {
                mode: right_mode,
                sha256: right_sha256,
                ..
            },
        ) => left_mode == right_mode && left_sha256 == right_sha256,
        (
            LinuxExactEntry::Symlink {
                target: left_target,
            },
            LinuxExactEntry::Symlink {
                target: right_target,
            },
        ) => left_target == right_target,
        _ => false,
    }
}

fn canonical_u32(value: &str) -> Result<u32, InstallPlatformError> {
    if value.is_empty()
        || value
            != value
                .parse::<u32>()
                .map_err(|_| error("invalid canonical MainPID"))?
                .to_string()
    {
        return Err(error("invalid canonical MainPID"));
    }
    value
        .parse()
        .map_err(|_| error("invalid canonical MainPID"))
}

pub(super) fn require_absolute(path: &str, description: &str) -> Result<(), InstallPlatformError> {
    if !PathBuf::from(path).is_absolute() || path.len() > 4096 || path.contains('\0') {
        return Err(error(format!(
            "{description} path is not bounded absolute text"
        )));
    }
    Ok(())
}

pub(super) fn error(detail: impl Into<String>) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}
