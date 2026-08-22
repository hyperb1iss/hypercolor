use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, PrivateStagingDirectory, ReadOnlyDirectoryAuthority,
};

#[cfg(target_os = "macos")]
mod macos;
mod manifest;
mod tree;

#[cfg(target_os = "macos")]
pub use macos::{MacosReleaseProvenance, bind_macos_release_provenance};
pub use manifest::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES,
};

use self::manifest::ValidatedManifest;
use super::model::{UnitId, UnitRecord};
use super::store::{InstallLock, InstallStore, InstallStoreError};

const MAX_STAGE_ATTEMPTS: usize = 128;
const STAGE_PREFIX: &str = ".hypercolor-stage-payload-";
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MANIFEST_NAME: &str = "manifest.json";

/// Validate and stage one verifier-extracted release payload.
///
/// `expected_unit` must be the SHA-256 digest of the exact manifest bytes
/// accepted by the verifier. The digest is checked before any install-unit or
/// staging mutation.
///
/// `candidate_executable` must already identify the running candidate. The
/// function never reopens a public executable pathname. Source member modes
/// must match the manifest exactly. Installed modes remove every write bit,
/// preserve execute bits, set `manifest.json` to `0444`, and set the unit root
/// to `0555` before the digest-named directory becomes visible.
///
/// The caller must hold `lock` for `store`. The lock and its retained root
/// authority stay alive through validation, staging, publication, and final
/// validation.
///
/// # Errors
///
/// Returns an error for an invalid manifest, unsafe or drifting source tree,
/// candidate identity mismatch, corrupt existing unit, staging failure,
/// publication failure, or durability failure.
pub fn stage_release_payload(
    store: &InstallStore,
    lock: &InstallLock,
    source_root: &Path,
    candidate_executable: &File,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let source = ReadOnlyDirectoryAuthority::open(source_root).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "open the extracted release root",
            source,
        }
    })?;
    stage_release_payload_from_authority(store, lock, &source, candidate_executable, expected_unit)
}

/// Validate and stage a release through an already-open source authority.
///
/// This variant lets a verifier hand off the exact directory handle it proved.
/// Renaming or replacing the source pathname after that handoff cannot redirect
/// validation or staging.
///
/// # Errors
///
/// Returns the same errors as [`stage_release_payload`].
pub fn stage_release_payload_from_authority(
    store: &InstallStore,
    lock: &InstallLock,
    source: &ReadOnlyDirectoryAuthority,
    candidate_executable: &File,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let manifest = validate_release_payload_authority(source, candidate_executable, expected_unit)?;

    let units = store.units_authority(lock)?;
    let unit_name = Path::new(manifest.unit_id.as_str());
    if let Some(metadata) =
        units
            .entry_metadata(unit_name)
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "inspect an existing digest unit",
                source,
            })?
    {
        if metadata.kind() != DirectoryEntryKind::Directory {
            return Err(ReleasePayloadError::InvalidUnit(
                "the digest unit name is not a directory".to_owned(),
            ));
        }
        let existing = units.open_child_directory(unit_name).map_err(|source| {
            ReleasePayloadError::Filesystem {
                operation: "open an existing digest unit",
                source,
            }
        })?;
        tree::validate_installed(&existing, &manifest)?;
        return unit_record(store, manifest.unit_id, &existing);
    }

    let staging = create_staging_directory(&units)?;
    if let Err(error) = tree::populate_staging(&staging, source, &manifest)
        .and_then(|()| tree::validate_source(source, &manifest))
        .and_then(|()| tree::finalize_staging(&staging, &manifest))
        .and_then(|()| tree::validate_installed(staging.directory(), &manifest))
    {
        return remove_failed_staging(staging, error);
    }

    let published =
        staging
            .publish_or_remove(unit_name)
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "publish the digest unit without replacement",
                source,
            })?;
    tree::validate_installed(&published, &manifest)?;
    unit_record(store, manifest.unit_id, &published)
}

/// Validate one verifier-extracted release without mutating install state.
///
/// This preflight is not durable authorization for a later stage. Callers must
/// invoke [`stage_release_payload`] after acquiring the install lock so the
/// source tree and running executable are revalidated immediately before
/// publication.
///
/// # Errors
///
/// Returns an error for an invalid manifest, unsafe or drifting source tree,
/// candidate identity mismatch, or unexpected manifest digest.
pub fn validate_release_payload(
    source_root: &Path,
    candidate_executable: &File,
    expected_unit: &UnitId,
) -> Result<(), ReleasePayloadError> {
    let source = ReadOnlyDirectoryAuthority::open(source_root).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "open the extracted release root",
            source,
        }
    })?;
    validate_release_payload_from_authority(&source, candidate_executable, expected_unit)
}

/// Validate one already-open release authority without mutating install state.
///
/// # Errors
///
/// Returns the same errors as [`validate_release_payload`].
pub fn validate_release_payload_from_authority(
    source: &ReadOnlyDirectoryAuthority,
    candidate_executable: &File,
    expected_unit: &UnitId,
) -> Result<(), ReleasePayloadError> {
    validate_release_payload_authority(source, candidate_executable, expected_unit).map(drop)
}

fn validate_release_payload_authority(
    source: &ReadOnlyDirectoryAuthority,
    candidate_executable: &File,
    expected_unit: &UnitId,
) -> Result<ValidatedManifest, ReleasePayloadError> {
    let manifest_bytes = tree::read_manifest_bytes(source)?;
    let manifest = ValidatedManifest::parse(manifest_bytes)?;
    if manifest.unit_id != *expected_unit {
        return Err(ReleasePayloadError::UnexpectedManifestDigest {
            expected: expected_unit.as_str().to_owned(),
            actual: manifest.unit_id.as_str().to_owned(),
        });
    }
    tree::validate_source(source, &manifest)?;
    tree::bind_candidate_executable(source, candidate_executable, &manifest)?;
    Ok(manifest)
}

pub(crate) fn retain_installed_release_unit(
    store: &InstallStore,
    lock: &InstallLock,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let directory = store.open_unit_directory(lock, expected_unit)?;
    let manifest_bytes = tree::read_installed_manifest_bytes(&directory)?;
    let manifest = ValidatedManifest::parse(manifest_bytes)?;
    if manifest.unit_id != *expected_unit {
        return Err(ReleasePayloadError::UnexpectedManifestDigest {
            expected: expected_unit.as_str().to_owned(),
            actual: manifest.unit_id.as_str().to_owned(),
        });
    }
    tree::validate_installed(&directory, &manifest)?;
    unit_record(store, manifest.unit_id, &directory)
}

fn unit_record(
    store: &InstallStore,
    unit_id: UnitId,
    directory: &DirectoryAuthority,
) -> Result<UnitRecord, ReleasePayloadError> {
    let authority = directory
        .read_only()
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "retain the exact immutable unit authority",
            source,
        })?;
    UnitRecord::new(unit_id.clone(), store.unit_path(&unit_id), authority).map_err(|source| {
        ReleasePayloadError::Filesystem {
            operation: "bind the immutable unit identity",
            source,
        }
    })
}

fn create_staging_directory(
    units: &DirectoryAuthority,
) -> Result<PrivateStagingDirectory, ReleasePayloadError> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("{STAGE_PREFIX}{}-{sequence}", std::process::id());
        match units.create_private_staging_directory(Path::new(&name)) {
            Ok(staging) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(ReleasePayloadError::Filesystem {
                    operation: "create a private release staging directory",
                    source,
                });
            }
        }
    }
    Err(ReleasePayloadError::StageCollisions {
        attempts: MAX_STAGE_ATTEMPTS,
    })
}

fn remove_failed_staging(
    staging: PrivateStagingDirectory,
    original: ReleasePayloadError,
) -> Result<UnitRecord, ReleasePayloadError> {
    match staging.remove() {
        Ok(()) => Err(original),
        Err(cleanup) => Err(ReleasePayloadError::Cleanup {
            original: original.to_string(),
            cleanup,
        }),
    }
}

/// Release payload validation or immutable staging failure.
#[derive(Debug, thiserror::Error)]
pub enum ReleasePayloadError {
    /// The manifest exceeded its bounded byte limit.
    #[error("release manifest exceeds {limit} bytes")]
    ManifestTooLarge { limit: usize },
    /// Strict JSON decoding failed.
    #[error("release manifest is not valid strict JSON: {0}")]
    DecodeManifest(#[from] serde_json::Error),
    /// Manifest semantics or inventory are invalid.
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    /// The extracted manifest does not match the verifier-accepted digest.
    #[error("release manifest digest {actual} does not match verified digest {expected}")]
    UnexpectedManifestDigest { expected: String, actual: String },
    /// The extracted source tree does not exactly match the manifest.
    #[error("invalid release source: {0}")]
    InvalidSource(String),
    /// The already-open candidate does not identify source `bin/hypercolor`.
    #[error("the running candidate does not match source bin/hypercolor")]
    CandidateMismatch,
    /// An existing or newly published immutable unit is not exact.
    #[error("invalid immutable release unit: {0}")]
    InvalidUnit(String),
    /// A bounded private staging namespace was exhausted.
    #[error("could not allocate a private staging directory after {attempts} attempts")]
    StageCollisions { attempts: usize },
    /// A handle-relative filesystem operation failed.
    #[error("failed to {operation}: {source}")]
    Filesystem {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    /// Install-store authority could not be retained.
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    /// Failure cleanup could not safely remove the exact private staging tree.
    #[error("release staging failed ({original}) and exact cleanup failed: {cleanup}")]
    Cleanup {
        original: String,
        #[source]
        cleanup: io::Error,
    },
}
