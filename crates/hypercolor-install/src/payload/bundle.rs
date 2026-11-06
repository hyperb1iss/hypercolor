//! Release units of the macOS app bundle layout.
//!
//! A unit is `manifest.json`, the store inventory at
//! `share/hypercolor/durable-stores.json` and `Hypercolor.app/` exactly as
//! it was signed, notarized and stapled. Nothing is added inside the
//! bundle: strict code validation refuses any file its seal does not cover.

use std::path::Path;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use super::manifest::{
    MACOS_BUNDLE_EXECUTABLES, MACOS_BUNDLE_ROOT, MacosBundleManifestFacts, ValidatedManifest,
    ValidatedMember,
};
use super::{ReleasePayloadError, tree};
use crate::macos_bundle::{
    MacosBundleIdentity, MacosBundleMember, MacosBundleMemberKind, MacosBundleUnit,
};
use crate::model::{UnitId, UnitRecord};
use crate::store::{InstallLock, InstallStore};

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
        .directory()
        .open_child_directory(Path::new(first))
        .map_err(open_error)?;
    for component in components {
        directory = directory
            .open_child_directory(Path::new(component))
            .map_err(open_error)?;
    }
    let mut opened = directory
        .open_regular_file(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a bundle unit executable",
            source,
        })?;
    if opened.metadata().size() != *size {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "{path} differs from its manifest size"
        )));
    }
    crate::macho_cdhash::thin_macho_cdhash(opened.file_mut(), *size)
        .map_err(|error| ReleasePayloadError::InvalidUnit(format!("{path}: {error}")))
}
