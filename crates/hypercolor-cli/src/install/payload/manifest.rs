use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::ReleasePayloadError;
use crate::install::model::UnitId;

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

const MAX_IDENTITY_BYTES: usize = 128;
const REQUIRED_BINARIES: [&str; 5] = [
    "hypercolor-daemon",
    "hypercolor",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
];

#[derive(Debug)]
pub(super) struct ValidatedManifest {
    pub(super) bytes: Vec<u8>,
    pub(super) unit_id: UnitId,
    #[cfg(target_os = "macos")]
    pub(super) platform: String,
    #[cfg(target_os = "macos")]
    pub(super) rust_target: String,
    pub(super) members: BTreeMap<String, ValidatedMember>,
    pub(super) children: BTreeMap<String, Vec<String>>,
}

impl ValidatedManifest {
    pub(super) fn parse(bytes: Vec<u8>) -> Result<Self, ReleasePayloadError> {
        let raw: RawManifest = serde_json::from_slice(&bytes)?;
        raw.validate(bytes)
    }
}

#[derive(Debug, Clone)]
pub(super) enum ValidatedMember {
    Directory {
        source_mode: u32,
    },
    File {
        source_mode: u32,
        size: u64,
        sha256: String,
    },
}

impl ValidatedMember {
    pub(super) const fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    name: String,
    version: String,
    platform: String,
    rust_target: String,
    binaries: Vec<String>,
    assets: RawAssets,
    members: Vec<RawMember>,
}

impl RawManifest {
    fn validate(self, bytes: Vec<u8>) -> Result<ValidatedManifest, ReleasePayloadError> {
        if self.name != "hypercolor"
            || !valid_identity(&self.version)
            || !valid_identity(&self.platform)
            || !valid_identity(&self.rust_target)
        {
            return Err(ReleasePayloadError::InvalidManifest(
                "release identity fields are invalid".to_owned(),
            ));
        }
        let binaries: BTreeSet<_> = self.binaries.iter().map(String::as_str).collect();
        let required: BTreeSet<_> = REQUIRED_BINARIES.into_iter().collect();
        if binaries != required || self.binaries.len() != REQUIRED_BINARIES.len() {
            return Err(ReleasePayloadError::InvalidManifest(
                "manifest binaries do not match the release payload".to_owned(),
            ));
        }
        self.assets.validate_minimums()?;
        if self.members.is_empty() || self.members.len() > MAX_RELEASE_MEMBERS {
            return Err(ReleasePayloadError::InvalidManifest(format!(
                "manifest member count must be in 1..={MAX_RELEASE_MEMBERS}"
            )));
        }

        let mut members = BTreeMap::new();
        let mut total_size = 0_u64;
        for raw in self.members {
            let path = validate_member_path(&raw.path)?;
            let member = raw.validate(&path)?;
            if let ValidatedMember::File { size, .. } = member {
                total_size = total_size.checked_add(size).ok_or_else(|| {
                    ReleasePayloadError::InvalidManifest(
                        "manifest file sizes overflow the payload bound".to_owned(),
                    )
                })?;
                if total_size > MAX_RELEASE_PAYLOAD_BYTES {
                    return Err(ReleasePayloadError::InvalidManifest(format!(
                        "manifest payload exceeds {MAX_RELEASE_PAYLOAD_BYTES} bytes"
                    )));
                }
            }
            if members.insert(path.clone(), member).is_some() {
                return Err(ReleasePayloadError::InvalidManifest(format!(
                    "duplicate manifest member path {path}"
                )));
            }
        }

        for path in members.keys() {
            let (parent, _) = split_parent(path);
            if !parent.is_empty()
                && !members
                    .get(parent)
                    .is_some_and(ValidatedMember::is_directory)
            {
                return Err(ReleasePayloadError::InvalidManifest(format!(
                    "manifest member {path} has no declared directory parent"
                )));
            }
        }
        for binary in REQUIRED_BINARIES {
            let path = format!("bin/{binary}");
            match members.get(&path) {
                Some(ValidatedMember::File { source_mode, .. }) if *source_mode == 0o755 => {}
                _ => {
                    return Err(ReleasePayloadError::InvalidManifest(format!(
                        "required binary {path} must be a 0755 regular file"
                    )));
                }
            }
        }
        validate_asset_counts(&members, &self.assets)?;

        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in members.keys() {
            let (parent, name) = split_parent(path);
            children
                .entry(parent.to_owned())
                .or_default()
                .push(name.to_owned());
        }
        let digest = hex_digest(&Sha256::digest(&bytes));
        let unit_id = UnitId::new(digest).map_err(|error| {
            ReleasePayloadError::InvalidManifest(format!("invalid manifest digest: {error}"))
        })?;
        Ok(ValidatedManifest {
            bytes,
            unit_id,
            #[cfg(target_os = "macos")]
            platform: self.platform,
            #[cfg(target_os = "macos")]
            rust_target: self.rust_target,
            members,
            children,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssets {
    #[serde(rename = "ui_files")]
    ui: u64,
    #[serde(rename = "bundled_effect_files")]
    bundled_effects: u64,
    #[serde(rename = "docs_files")]
    docs: u64,
    #[serde(rename = "skill_files")]
    skills: u64,
    #[serde(rename = "agent_files")]
    agents: u64,
    #[serde(rename = "site_files")]
    site: u64,
}

impl RawAssets {
    fn validate_minimums(&self) -> Result<(), ReleasePayloadError> {
        if self.ui == 0 || self.bundled_effects == 0 || self.skills == 0 || self.agents == 0 {
            return Err(ReleasePayloadError::InvalidManifest(
                "required release asset counts must be nonzero".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMember {
    path: String,
    #[serde(rename = "type")]
    kind: RawMemberKind,
    mode: u32,
    size: Option<u64>,
    sha256: Option<String>,
}

impl RawMember {
    fn validate(self, path: &str) -> Result<ValidatedMember, ReleasePayloadError> {
        if self.mode > 0o777 {
            return Err(ReleasePayloadError::InvalidManifest(format!(
                "manifest mode is invalid for {path}"
            )));
        }
        match (self.kind, self.size, self.sha256) {
            (RawMemberKind::Directory, None, None) if self.mode & 0o500 == 0o500 => {
                Ok(ValidatedMember::Directory {
                    source_mode: self.mode,
                })
            }
            (RawMemberKind::File, Some(size), Some(sha256))
                if self.mode & 0o400 == 0o400
                    && size <= MAX_RELEASE_MEMBER_BYTES
                    && valid_sha256(&sha256) =>
            {
                Ok(ValidatedMember::File {
                    source_mode: self.mode,
                    size,
                    sha256,
                })
            }
            (RawMemberKind::Directory, _, _) => Err(ReleasePayloadError::InvalidManifest(format!(
                "directory member fields or owner permissions are invalid for {path}"
            ))),
            (RawMemberKind::File, _, _) => Err(ReleasePayloadError::InvalidManifest(format!(
                "file member fields or owner permissions are invalid for {path}"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawMemberKind {
    Directory,
    File,
}

fn validate_member_path(path: &str) -> Result<String, ReleasePayloadError> {
    if path.is_empty()
        || path.len() > MAX_RELEASE_PATH_BYTES
        || path == super::MANIFEST_NAME
        || path.contains(['\\', '\0'])
        || path.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(ReleasePayloadError::InvalidManifest(format!(
            "manifest member path is invalid: {path:?}"
        )));
    }
    let parsed = Path::new(path);
    if parsed
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
        || parsed
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
            != path
    {
        return Err(ReleasePayloadError::InvalidManifest(format!(
            "manifest member path is not a canonical safe relative path: {path:?}"
        )));
    }
    Ok(path.to_owned())
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

fn validate_asset_counts(
    members: &BTreeMap<String, ValidatedMember>,
    assets: &RawAssets,
) -> Result<(), ReleasePayloadError> {
    let expected = [
        ("share/hypercolor/ui", assets.ui),
        ("share/hypercolor/effects/bundled", assets.bundled_effects),
        ("share/hypercolor/docs", assets.docs),
        ("share/hypercolor/agents/skills", assets.skills),
        ("share/hypercolor/agents/agents", assets.agents),
        ("share/hypercolor/site", assets.site),
    ];
    for (prefix, expected_count) in expected {
        if !members
            .get(prefix)
            .is_some_and(ValidatedMember::is_directory)
        {
            return Err(ReleasePayloadError::InvalidManifest(format!(
                "manifest asset root is missing or not a directory: {prefix}"
            )));
        }
        let prefix_with_separator = format!("{prefix}/");
        let actual = members
            .iter()
            .filter(|(path, member)| {
                path.starts_with(&prefix_with_separator)
                    && matches!(member, ValidatedMember::File { .. })
            })
            .count();
        if u64::try_from(actual).ok() != Some(expected_count) {
            return Err(ReleasePayloadError::InvalidManifest(format!(
                "manifest asset count is wrong for {prefix}"
            )));
        }
    }
    Ok(())
}

pub(super) fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}
