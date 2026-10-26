//! The manifest a Windows release unit carries (RFC 57, B1.3).
//!
//! A Windows unit is the installed application tree: the app, daemon and
//! CLI programs at its root beside their DLLs, with the UI, bundled effects
//! and tools beneath. Members carry a permission class instead of a unix
//! mode, and every path component is a name the Win32 layer reads the way
//! the handle-relative store does. The unit's identity is the SHA-256 of
//! these exact bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;

use hypercolor_platform_fs::windows_child_name_is_valid;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::payload::ReleasePayloadError;
use crate::model::UnitId;

/// Maximum accepted byte length of `manifest.json`.
pub const MAX_RELEASE_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
/// Maximum number of entries described by one release manifest.
pub const MAX_RELEASE_MEMBERS: usize = 16 * 1024;
/// Maximum UTF-8 byte length of one release member path.
pub const MAX_RELEASE_PATH_BYTES: usize = 1024;
/// Maximum accepted size of one release file.
pub const MAX_RELEASE_MEMBER_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum combined size of all files described by one release manifest.
pub const MAX_RELEASE_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;

pub(super) const MANIFEST_NAME: &str = "manifest.json";
const MAX_IDENTITY_BYTES: usize = 128;
/// Programs every unit runs, at its root.
const REQUIRED_PROGRAMS: [&str; 3] = [
    "hypercolor-app.exe",
    "hypercolor-daemon.exe",
    "hypercolor.exe",
];
/// Asset directories every unit ships with at least one file beneath.
const REQUIRED_ASSET_DIRECTORIES: [&str; 2] = ["ui", "effects/bundled"];

/// How an installed member may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionClass {
    /// Code the release runs: everyone who may read the unit may also
    /// execute it.
    Program,
    /// Everything else: everyone who may read the unit may read it.
    Data,
}

#[derive(Debug)]
pub(super) struct ValidatedManifest {
    pub(super) bytes: Vec<u8>,
    pub(super) unit_id: UnitId,
    pub(super) members: BTreeMap<String, Member>,
    /// Child names of every directory member, the root as "".
    pub(super) children: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Member {
    Directory,
    File {
        class: PermissionClass,
        size: u64,
        sha256: String,
    },
}

impl ValidatedManifest {
    pub(super) fn parse(bytes: Vec<u8>) -> Result<Self, ReleasePayloadError> {
        if bytes.len() > MAX_RELEASE_MANIFEST_BYTES {
            return Err(ReleasePayloadError::ManifestTooLarge {
                limit: MAX_RELEASE_MANIFEST_BYTES,
            });
        }
        let raw: RawManifest = serde_json::from_slice(&bytes)?;
        raw.validate(bytes)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    name: String,
    version: String,
    platform: String,
    rust_target: String,
    programs: Vec<String>,
    members: Vec<RawMember>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMember {
    path: String,
    #[serde(rename = "type")]
    kind: RawMemberKind,
    class: Option<PermissionClass>,
    size: Option<u64>,
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawMemberKind {
    Directory,
    File,
}

impl RawManifest {
    fn validate(self, bytes: Vec<u8>) -> Result<ValidatedManifest, ReleasePayloadError> {
        if self.name != "hypercolor"
            || !valid_identity(&self.version)
            || !valid_identity(&self.platform)
            || !valid_identity(&self.rust_target)
            || !self.platform.starts_with("windows-")
            || !self.rust_target.ends_with("-pc-windows-msvc")
        {
            return invalid("release identity fields do not name a Windows release");
        }
        let programs: BTreeSet<_> = self.programs.iter().map(String::as_str).collect();
        if programs != REQUIRED_PROGRAMS.into_iter().collect()
            || self.programs.len() != REQUIRED_PROGRAMS.len()
        {
            return invalid("manifest programs do not match the release payload");
        }
        if self.members.is_empty() || self.members.len() > MAX_RELEASE_MEMBERS {
            return invalid(format!(
                "manifest member count must be in 1..={MAX_RELEASE_MEMBERS}"
            ));
        }

        let mut members = BTreeMap::new();
        let mut folded = BTreeSet::new();
        let mut total_size = 0_u64;
        for raw in self.members {
            let path = validate_member_path(&raw.path)?;
            if !folded.insert(path.to_lowercase()) {
                return invalid(format!(
                    "manifest member {path} repeats a path a case-insensitive volume merges"
                ));
            }
            let member = raw.validate(&path)?;
            if let Member::File { size, .. } = member {
                total_size = total_size
                    .checked_add(size)
                    .filter(|total| *total <= MAX_RELEASE_PAYLOAD_BYTES)
                    .ok_or_else(|| {
                        ReleasePayloadError::InvalidManifest(format!(
                            "manifest payload exceeds {MAX_RELEASE_PAYLOAD_BYTES} bytes"
                        ))
                    })?;
            }
            members.insert(path, member);
        }

        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        children.insert(String::new(), Vec::new());
        for (path, member) in &members {
            if *member == Member::Directory {
                children.entry(path.clone()).or_default();
            }
        }
        for path in members.keys() {
            let (parent, name) = split_parent(path);
            match children.get_mut(parent) {
                Some(names) => names.push(name.to_owned()),
                None => {
                    return invalid(format!(
                        "manifest member {path} has no declared directory parent"
                    ));
                }
            }
        }
        for program in REQUIRED_PROGRAMS {
            match members.get(program) {
                Some(Member::File {
                    class: PermissionClass::Program,
                    ..
                }) => {}
                _ => {
                    return invalid(format!(
                        "required program {program} must be a program file at the unit root"
                    ));
                }
            }
        }
        for directory in REQUIRED_ASSET_DIRECTORIES {
            let prefix = format!("{directory}/");
            let has_file = members.iter().any(|(path, member)| {
                path.starts_with(&prefix) && matches!(member, Member::File { .. })
            });
            if members.get(directory) != Some(&Member::Directory) || !has_file {
                return invalid(format!(
                    "required asset directory {directory} must hold at least one file"
                ));
            }
        }

        let digest = hex::encode(Sha256::digest(&bytes));
        let unit_id = UnitId::new(digest).map_err(|error| {
            ReleasePayloadError::InvalidManifest(format!("invalid manifest digest: {error}"))
        })?;
        Ok(ValidatedManifest {
            bytes,
            unit_id,
            members,
            children,
        })
    }
}

impl RawMember {
    fn validate(self, path: &str) -> Result<Member, ReleasePayloadError> {
        match (self.kind, self.class, self.size, self.sha256) {
            (RawMemberKind::Directory, None, None, None) => Ok(Member::Directory),
            (RawMemberKind::File, Some(class), Some(size), Some(sha256))
                if size <= MAX_RELEASE_MEMBER_BYTES && valid_sha256(&sha256) =>
            {
                Ok(Member::File {
                    class,
                    size,
                    sha256,
                })
            }
            (RawMemberKind::Directory, ..) => invalid(format!(
                "directory member {path} must carry no class, size or digest"
            )),
            (RawMemberKind::File, ..) => invalid(format!(
                "file member {path} needs a class, a bounded size and a SHA-256"
            )),
        }
    }
}

/// Split a member path into its parent ("" at the root) and final name.
pub(super) fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn validate_member_path(path: &str) -> Result<String, ReleasePayloadError> {
    let valid = !path.is_empty()
        && path.len() <= MAX_RELEASE_PATH_BYTES
        && !path.eq_ignore_ascii_case(MANIFEST_NAME)
        && path
            .split('/')
            .all(|name| !name.is_empty() && windows_child_name_is_valid(OsStr::new(name)));
    if valid {
        Ok(path.to_owned())
    } else {
        invalid(format!(
            "manifest member path is not a safe relative Windows path: {path:?}"
        ))
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTITY_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ReleasePayloadError> {
    Err(ReleasePayloadError::InvalidManifest(message.into()))
}
