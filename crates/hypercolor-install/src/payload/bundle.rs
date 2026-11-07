//! Release units of the macOS app bundle layout.
//!
//! A unit is `manifest.json`, the store inventory at
//! `share/hypercolor/durable-stores.json` and `Hypercolor.app/` exactly as
//! it was signed, notarized and stapled. Nothing is added inside the
//! bundle: strict code validation refuses any file its seal does not cover.
//!
//! That is also why a first install has no manifest to bring: the DMG holds a
//! bundle, and a bundle cannot carry a file that hashes its own seal. Adoption
//! describes the unit from the bytes it found instead, with the store
//! inventory the bundle seals among its resources.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{DirectoryEntryKind, OpenedRegularFile, ReadOnlyDirectoryAuthority};
use serde::Serialize;

use super::manifest::{
    MACOS_BUNDLE_CLI, MACOS_BUNDLE_EXECUTABLES, MACOS_BUNDLE_ROOT, MAX_RELEASE_MEMBERS,
    MacosBundleManifestFacts, ValidatedManifest, ValidatedMember,
};
use super::{MANIFEST_NAME, ReleasePayloadError, tree};
use crate::macos_bundle::{
    MacosBundleIdentity, MacosBundleMember, MacosBundleMemberKind, MacosBundleUnit,
};
use crate::model::{UnitId, UnitRecord};
use crate::store::{InstallLock, InstallStore};

/// Depth of the deepest member adoption will describe.
const MAX_DESCRIBED_DEPTH: usize = 64;
/// The mode an unpacked unit's `manifest.json` carries.
const MANIFEST_SOURCE_MODE: u32 = 0o644;

/// Validate one extracted app bundle unit and publish it into `store`.
///
/// `expected_unit` must be the SHA-256 of the exact `manifest.json` bytes
/// the verifier accepted. The tree must match the manifest exactly, with
/// the source modes it records; the published unit drops every write bit.
///
/// # Errors
/// Returns an error for a manifest that is not a bundle unit's, a tree that
/// differs from it, a digest mismatch, or a staging or publication failure.
pub fn stage_macos_bundle_unit(
    store: &InstallStore,
    lock: &InstallLock,
    source_root: &Path,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let source = ReadOnlyDirectoryAuthority::open(source_root).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "open the extracted bundle unit",
            source,
        }
    })?;
    stage_macos_bundle_unit_from_authority(store, lock, &source, expected_unit)
}

/// [`stage_macos_bundle_unit`] through a source authority the verifier
/// already holds, so renaming the source path cannot redirect staging.
///
/// # Errors
/// Returns the same errors as [`stage_macos_bundle_unit`].
pub fn stage_macos_bundle_unit_from_authority(
    store: &InstallStore,
    lock: &InstallLock,
    source: &ReadOnlyDirectoryAuthority,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let (manifest, _) = ValidatedManifest::parse_macos_bundle(tree::read_manifest_bytes(source)?)?;
    require_unit(&manifest, expected_unit)?;
    tree::validate_source(source, &manifest)?;
    super::stage_validated_payload(store, lock, source, manifest, tree::TreeMode::Source)
}

/// Reopen an installed app bundle unit and revalidate every member.
///
/// # Errors
/// Returns an error when the unit is missing, is not a bundle unit, or any
/// member differs from its manifest.
pub fn retain_macos_bundle_unit(
    store: &InstallStore,
    lock: &InstallLock,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let directory = store.open_unit_directory(lock, expected_unit)?;
    let (manifest, _) =
        ValidatedManifest::parse_macos_bundle(tree::read_installed_manifest_bytes(&directory)?)?;
    require_unit(&manifest, expected_unit)?;
    tree::validate_installed(&directory, &manifest)?;
    super::unit_record(store, manifest.unit_id, &directory)
}

/// Write the `manifest.json` of the unit tree at `root`, describing every
/// member exactly as it stands there, and return the unit's digest.
///
/// The members are sorted by path and the JSON is pretty-printed, so one
/// tree always describes to one digest.
///
/// # Errors
/// Returns an error for a member that is neither a directory nor a regular
/// file, a tree past the unit bounds, a host outside the macOS release
/// targets, or a manifest that does not parse back as a bundle unit's.
pub fn describe_macos_bundle_unit(
    root: &Path,
    version: &str,
) -> Result<UnitId, ReleasePayloadError> {
    let (platform, rust_target) = host_target()?;
    let source = ReadOnlyDirectoryAuthority::open(root).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "open the bundle unit to describe",
            source,
        }
    })?;
    let mut members = Vec::new();
    describe_directory(&source, "", 0, &mut members)?;
    members.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = serde_json::to_vec_pretty(&DescribedUnit {
        name: "hypercolor",
        version,
        platform,
        rust_target,
        members,
    })
    .map_err(|source| ReleasePayloadError::InvalidManifest(source.to_string()))?;
    let (manifest, _) = ValidatedManifest::parse_macos_bundle(bytes.clone())?;
    let path = root.join(MANIFEST_NAME);
    fs::write(&path, &bytes)
        .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(MANIFEST_SOURCE_MODE)))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "write the described manifest",
            source,
        })?;
    Ok(manifest.unit_id)
}

#[derive(Serialize)]
struct DescribedUnit<'a> {
    name: &'static str,
    version: &'a str,
    platform: &'static str,
    rust_target: &'static str,
    members: Vec<DescribedMember>,
}

#[derive(Serialize)]
struct DescribedMember {
    path: String,
    #[serde(rename = "type")]
    kind: &'static str,
    mode: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
}

fn describe_directory(
    directory: &ReadOnlyDirectoryAuthority,
    prefix: &str,
    depth: usize,
    members: &mut Vec<DescribedMember>,
) -> Result<(), ReleasePayloadError> {
    let failed = |operation: &'static str| {
        move |source: io::Error| ReleasePayloadError::Filesystem { operation, source }
    };
    if depth > MAX_DESCRIBED_DEPTH {
        return Err(ReleasePayloadError::InvalidUnit(
            "the bundle exceeds its depth bound".to_owned(),
        ));
    }
    for entry in directory
        .entries()
        .map_err(failed("list a described directory"))?
    {
        let name = entry.to_str().ok_or_else(|| {
            ReleasePayloadError::InvalidUnit("a bundle member name is not UTF-8".to_owned())
        })?;
        if prefix.is_empty() && name == MANIFEST_NAME {
            continue;
        }
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = directory
            .entry_metadata(Path::new(name))
            .map_err(failed("inspect a described member"))?
            .ok_or_else(|| {
                ReleasePayloadError::InvalidUnit(format!("{path} disappeared while described"))
            })?;
        match metadata.kind() {
            DirectoryEntryKind::Directory => {
                let child = directory
                    .open_child_directory(Path::new(name))
                    .map_err(failed("open a described directory"))?;
                members.push(DescribedMember {
                    path: path.clone(),
                    kind: "directory",
                    mode: metadata.mode(),
                    size: None,
                    sha256: None,
                });
                describe_directory(&child, &path, depth + 1, members)?;
            }
            DirectoryEntryKind::RegularFile => {
                let opened = directory
                    .open_regular_file(Path::new(name))
                    .map_err(failed("open a described file"))?;
                let size = opened.metadata().size();
                let sha256 = tree::hash_file_exact(opened.file(), size, "hash a described file")?;
                members.push(DescribedMember {
                    path,
                    kind: "file",
                    mode: opened.metadata().mode(),
                    size: Some(size),
                    sha256: Some(sha256),
                });
            }
            DirectoryEntryKind::SymbolicLink | DirectoryEntryKind::Special => {
                return Err(ReleasePayloadError::InvalidUnit(format!(
                    "{path} is neither a directory nor a regular file"
                )));
            }
        }
        if members.len() > MAX_RELEASE_MEMBERS {
            return Err(ReleasePayloadError::InvalidUnit(format!(
                "the bundle holds more than {MAX_RELEASE_MEMBERS} members"
            )));
        }
    }
    Ok(())
}

/// The manifest's `platform` and `rust_target` for this architecture.
fn host_target() -> Result<(&'static str, &'static str), ReleasePayloadError> {
    match std::env::consts::ARCH {
        "aarch64" => Ok(("macos-aarch64", "aarch64-apple-darwin")),
        "x86_64" => Ok(("macos-x86_64", "x86_64-apple-darwin")),
        other => Err(ReleasePayloadError::InvalidUnit(format!(
            "no macOS release target for {other}"
        ))),
    }
}

/// Prove the CLI of the retained unit `unit` holds the bytes its manifest
/// records, without the install lock, and return the path to execute.
///
/// A program that must run a release while a transaction is open cannot take
/// the lock that transaction holds. macOS has no `fexecve`, so the kernel
/// opens the path again; the release root is this user's own, and the code
/// signature is checked on that open.
///
/// # Errors
/// Returns an error when the unit is not a digest unit of the release root or
/// its CLI differs from its manifest.
pub fn verified_macos_bundle_cli(
    release_root: &Path,
    unit: &UnitId,
) -> Result<PathBuf, ReleasePayloadError> {
    let unit_root = release_root.join("units").join(unit.as_str());
    let directory = ReadOnlyDirectoryAuthority::open(&unit_root).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "open a retained bundle unit",
            source,
        }
    })?;
    let (manifest, _) =
        ValidatedManifest::parse_macos_bundle(tree::read_retained_manifest_bytes(&directory)?)?;
    require_unit(&manifest, unit)?;
    let opened = open_member(&directory, MACOS_BUNDLE_CLI)?;
    let Some(ValidatedMember::File { size, sha256, .. }) = manifest.members.get(MACOS_BUNDLE_CLI)
    else {
        return Err(ReleasePayloadError::InvalidUnit(
            "bundle unit lacks its CLI".to_owned(),
        ));
    };
    let actual = opened.metadata().size();
    if actual != *size
        || tree::hash_file_exact(opened.file(), actual, "hash a bundle unit's CLI")? != *sha256
    {
        return Err(ReleasePayloadError::InvalidUnit(
            "the unit's CLI differs from its manifest".to_owned(),
        ));
    }
    Ok(unit_root.join(MACOS_BUNDLE_CLI))
}

/// Publish the CLI of `unit` as the stable launcher `name` in the release
/// root, an exact byte copy that keeps the signature embedded in the binary
/// and none of the source file's extended attributes.
///
/// # Errors
/// Returns an error when the CLI cannot be read or the copy published.
pub(crate) fn publish_macos_bundle_launcher(
    store: &InstallStore,
    lock: &InstallLock,
    unit: &UnitRecord,
    name: &Path,
) -> Result<(), ReleasePayloadError> {
    let mut opened = open_member(unit.directory(), MACOS_BUNDLE_CLI)?;
    let size = opened.metadata().size();
    store
        .publish_program(lock, name, size, opened.file_mut())
        .map_err(ReleasePayloadError::from)
}

/// Read what a retained bundle unit states: its manifest facts, its bundle
/// members and the cdhash of each executable, which is how a running daemon
/// is mapped back to its unit.
///
/// The members themselves are not rehashed here; retaining or staging the
/// unit proved them, and every materialization verifies its copy again.
///
/// # Errors
/// Returns an error when the unit's manifest is not a bundle unit's or an
/// executable is not a thin Mach-O for this architecture.
pub fn bind_macos_bundle_unit(unit: &UnitRecord) -> Result<MacosBundleUnit, ReleasePayloadError> {
    let (manifest, facts) = ValidatedManifest::parse_macos_bundle(
        tree::read_retained_manifest_bytes(unit.directory())?,
    )?;
    require_unit(&manifest, unit.id())?;
    let [app, daemon, cli] =
        MACOS_BUNDLE_EXECUTABLES.map(|path| executable_cdhash(unit, &manifest, path));
    let identity = MacosBundleIdentity {
        app: app?,
        daemon: daemon?,
        cli: cli?,
    };
    let daemon_sha256 = match manifest.members.get(MACOS_BUNDLE_EXECUTABLES[1]) {
        Some(ValidatedMember::File { sha256, .. }) => sha256.clone(),
        _ => {
            return Err(ReleasePayloadError::InvalidUnit(
                "bundle unit lacks its daemon".to_owned(),
            ));
        }
    };
    let bundle_mode = match manifest.members.get(MACOS_BUNDLE_ROOT) {
        Some(ValidatedMember::Directory { source_mode }) => *source_mode,
        _ => {
            return Err(ReleasePayloadError::InvalidUnit(
                "bundle unit lacks its bundle directory".to_owned(),
            ));
        }
    };
    let prefix = format!("{MACOS_BUNDLE_ROOT}/");
    let members = manifest
        .members
        .iter()
        .filter_map(|(path, member)| {
            let relative = path.strip_prefix(&prefix)?;
            Some(MacosBundleMember {
                path: relative.to_owned(),
                kind: match member {
                    ValidatedMember::Directory { source_mode } => {
                        MacosBundleMemberKind::Directory { mode: *source_mode }
                    }
                    ValidatedMember::File {
                        source_mode,
                        size,
                        sha256,
                    } => MacosBundleMemberKind::File {
                        mode: *source_mode,
                        size: *size,
                        sha256: sha256.clone(),
                    },
                },
            })
        })
        .collect();
    let MacosBundleManifestFacts {
        version,
        installed_bytes,
    } = facts;
    Ok(MacosBundleUnit::new(
        manifest.unit_id,
        version,
        installed_bytes,
        identity,
        daemon_sha256,
        bundle_mode,
        members,
    ))
}

fn require_unit(
    manifest: &ValidatedManifest,
    expected: &UnitId,
) -> Result<(), ReleasePayloadError> {
    if manifest.unit_id == *expected {
        Ok(())
    } else {
        Err(ReleasePayloadError::UnexpectedManifestDigest {
            expected: expected.as_str().to_owned(),
            actual: manifest.unit_id.as_str().to_owned(),
        })
    }
}

fn executable_cdhash(
    unit: &UnitRecord,
    manifest: &ValidatedManifest,
    path: &str,
) -> Result<String, ReleasePayloadError> {
    let Some(ValidatedMember::File { size, .. }) = manifest.members.get(path) else {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "bundle unit lacks {path}"
        )));
    };
    let mut opened = open_member(unit.directory(), path)?;
    if opened.metadata().size() != *size {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "{path} differs from its manifest size"
        )));
    }
    crate::macho_cdhash::thin_macho_cdhash(opened.file_mut(), *size)
        .map_err(|error| ReleasePayloadError::InvalidUnit(format!("{path}: {error}")))
}

/// Open the regular file at the `/`-separated member `path` of a unit, one
/// component at a time without following a link.
fn open_member(
    unit: &ReadOnlyDirectoryAuthority,
    path: &str,
) -> Result<OpenedRegularFile, ReleasePayloadError> {
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or_else(|| ReleasePayloadError::InvalidUnit(format!("{path} has no parent")))?;
    let open_error = |source| ReleasePayloadError::Filesystem {
        operation: "open a bundle unit directory",
        source,
    };
    let mut components = parent.split('/');
    let first = components.next().unwrap_or_default();
    let mut directory = unit
        .open_child_directory(Path::new(first))
        .map_err(open_error)?;
    for component in components {
        directory = directory
            .open_child_directory(Path::new(component))
            .map_err(open_error)?;
    }
    directory
        .open_regular_file(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a bundle unit file",
            source,
        })
}
