//! Staging a Windows release unit into the store (RFC 57, B1.3).
//!
//! The source is an extracted unit tree another principal may still be able
//! to change, such as the daemon's own staging under the state root. Every
//! member is therefore copied rather than moved, through handles that never
//! follow a reparse point, and hashed as it is written: the bytes that land
//! in the unit are the bytes that matched the manifest, whatever the source
//! does meanwhile. The copy is published under its digest name only after
//! the staged tree proves exact, and a published unit is proved again
//! before it is trusted.

use std::ffi::OsStr;
use std::io::{self, Read as _, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess, WindowsEntryKind};
use sha2::{Digest as _, Sha256};

use super::error::ReleasePayloadError;
use super::machine_layout::{
    WINDOWS_APP_PROGRAM, WINDOWS_BOOTSTRAP_PROGRAM, WINDOWS_BROKER_PROGRAM, WINDOWS_DAEMON_PROGRAM,
};
use super::manifest::{
    MANIFEST_NAME, MAX_RELEASE_MANIFEST_BYTES, Member, PermissionClass, ValidatedManifest,
    fold_case, split_parent,
};
use super::model::{WindowsFileIdentity, WindowsUnitBinding};
use super::store::{InstallLock, InstallStore};
use crate::model::{UnitId, UnitRecord};

const STAGE_PREFIX: &str = ".hypercolor-stage-";
const MAX_STAGE_ATTEMPTS: usize = 128;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Validate one extracted unit without touching the store.
///
/// This preflight is not authorization: [`stage_release_payload`] proves
/// the source again while it copies, under the install lock.
///
/// # Errors
///
/// Returns an error for an invalid manifest, a digest other than
/// `expected_unit`, or a source tree that differs from its manifest.
pub fn validate_release_payload(
    source: &WindowsDirectory,
    expected_unit: &UnitId,
) -> Result<(), ReleasePayloadError> {
    let manifest = read_manifest(source, expected_unit)?;
    walk_source(source, &manifest, "")
}

/// Copy one extracted unit into the store as an immutable unit named by its
/// digest, or bind the identical unit already there.
///
/// The caller must hold `lock` for `store`.
///
/// # Errors
///
/// Returns an error for an invalid manifest, a digest other than
/// `expected_unit`, a source that differs from its manifest, an existing
/// unit that is not exact, or a staging, publication or durability failure.
pub fn stage_release_payload(
    store: &InstallStore,
    lock: &InstallLock,
    source: &WindowsDirectory,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let manifest = read_manifest(source, expected_unit)?;
    let units = store.units_directory(lock, true)?.ok_or_else(|| {
        ReleasePayloadError::InvalidUnit("the units directory vanished".to_owned())
    })?;
    let unit_name = OsStr::new(manifest.unit_id.as_str());
    if units
        .entry_metadata(unit_name)
        .map_err(filesystem("inspect an existing digest unit"))?
        .is_some()
    {
        let existing = units
            .open_child_directory(unit_name, WindowsDirectoryAccess::Read)
            .map_err(filesystem("open an existing digest unit"))?;
        validate_installed(store, &existing, &manifest)?;
        return unit_record(store, &manifest.unit_id, existing);
    }

    let (staging_name, staging) = create_staging(&units)?;
    let staged = copy_tree(source, &staging, &manifest, "")
        .and_then(|()| write_manifest(&staging, &manifest))
        .and_then(|()| validate_installed(store, &staging, &manifest));
    drop(staging);
    if let Err(error) = staged {
        return match units.remove_tree(OsStr::new(&staging_name)) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(ReleasePayloadError::Cleanup {
                original: error.to_string(),
                cleanup,
            }),
        };
    }
    units
        .rename(OsStr::new(&staging_name), unit_name, false)
        .map_err(filesystem("publish the digest unit without replacement"))?;
    let published = units
        .open_child_directory(unit_name, WindowsDirectoryAccess::Read)
        .map_err(filesystem("open the published digest unit"))?;
    validate_installed(store, &published, &manifest)?;
    unit_record(store, &manifest.unit_id, published)
}

/// Bind and fully prove one unit already in the store.
///
/// # Errors
///
/// Returns an error for a missing unit, a manifest that does not hash to
/// `expected_unit`, or a unit tree, digest or ACL that is not exact.
pub fn retain_installed_release_unit(
    store: &InstallStore,
    lock: &InstallLock,
    expected_unit: &UnitId,
) -> Result<UnitRecord, ReleasePayloadError> {
    let directory = store.open_unit_directory(lock, expected_unit)?;
    let manifest = read_manifest(&directory, expected_unit)?;
    validate_installed(store, &directory, &manifest)?;
    unit_record(store, expected_unit, directory)
}

/// What a retained unit's manifest and files state about the programs a
/// transaction proves: the manifest's digests, and the file IDs those
/// members hold on their volume.
///
/// # Errors
///
/// Returns an error when the manifest does not hash to the unit's digest
/// or a program is missing.
pub(super) fn bind_windows_unit(
    unit: &UnitRecord,
) -> Result<WindowsUnitBinding, ReleasePayloadError> {
    let directory = unit.directory();
    let manifest = read_manifest(directory, unit.id())?;
    let identity = |member| member_identity(directory, &manifest, member);
    Ok(WindowsUnitBinding {
        unit: unit.id().clone(),
        version: manifest.version.clone(),
        app: identity(WINDOWS_APP_PROGRAM)?,
        daemon: identity(WINDOWS_DAEMON_PROGRAM)?,
        broker: identity(WINDOWS_BROKER_PROGRAM)?,
        bootstrap: identity(WINDOWS_BOOTSTRAP_PROGRAM)?,
    })
}

fn member_identity(
    root: &WindowsDirectory,
    manifest: &ValidatedManifest,
    member: &str,
) -> Result<WindowsFileIdentity, ReleasePayloadError> {
    let Some(Member::File { sha256, .. }) = manifest.members.get(member) else {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "the unit has no program {member}"
        )));
    };
    let (parent, name) = split_parent(member);
    let mut holder = None;
    for component in parent.split('/').filter(|part| !part.is_empty()) {
        let opened = holder
            .as_ref()
            .unwrap_or(root)
            .open_child_directory(OsStr::new(component), WindowsDirectoryAccess::Read)
            .map_err(filesystem("open a unit directory"))?;
        holder = Some(opened);
    }
    let metadata = holder
        .as_ref()
        .unwrap_or(root)
        .entry_metadata(OsStr::new(name))
        .map_err(filesystem("inspect a unit program"))?
        .ok_or_else(|| ReleasePayloadError::InvalidUnit(format!("{member} is missing")))?;
    Ok(WindowsFileIdentity {
        volume_serial: metadata.volume_serial(),
        file_id: format!("{:032x}", metadata.file_id()),
        sha256: sha256.clone(),
    })
}

fn read_manifest(
    directory: &WindowsDirectory,
    expected_unit: &UnitId,
) -> Result<ValidatedManifest, ReleasePayloadError> {
    let file = directory
        .open_file(OsStr::new(MANIFEST_NAME))
        .map_err(filesystem("open manifest.json without following links"))?;
    let mut bytes = Vec::new();
    file.take(MAX_RELEASE_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(filesystem("read manifest.json"))?;
    let manifest = ValidatedManifest::parse(bytes)?;
    if manifest.unit_id != *expected_unit {
        return Err(ReleasePayloadError::UnexpectedManifestDigest {
            expected: expected_unit.as_str().to_owned(),
            actual: manifest.unit_id.as_str().to_owned(),
        });
    }
    Ok(manifest)
}

/// Require that a source directory holds exactly its declared members, each
/// file matching its size and digest.
fn walk_source(
    directory: &WindowsDirectory,
    manifest: &ValidatedManifest,
    path: &str,
) -> Result<(), ReleasePayloadError> {
    require_exact_entries(
        directory,
        manifest,
        path,
        ReleasePayloadError::InvalidSource,
    )?;
    for name in &manifest.children[path] {
        let member_path = join(path, name);
        match &manifest.members[&member_path] {
            Member::Directory => {
                let child = directory
                    .open_child_directory(OsStr::new(name), WindowsDirectoryAccess::Read)
                    .map_err(|error| source_error(&member_path, &error))?;
                walk_source(&child, manifest, &member_path)?;
            }
            Member::File { size, sha256, .. } => {
                let file = directory
                    .open_file(OsStr::new(name))
                    .map_err(|error| source_error(&member_path, &error))?;
                hash_exact(file, None, *size, sha256)
                    .map_err(|error| source_error(&member_path, &error))?;
            }
        }
    }
    Ok(())
}

fn copy_tree(
    source: &WindowsDirectory,
    destination: &WindowsDirectory,
    manifest: &ValidatedManifest,
    path: &str,
) -> Result<(), ReleasePayloadError> {
    require_exact_entries(source, manifest, path, ReleasePayloadError::InvalidSource)?;
    for name in &manifest.children[path] {
        let member_path = join(path, name);
        let os_name = OsStr::new(name);
        match &manifest.members[&member_path] {
            Member::Directory => {
                let source_child = source
                    .open_child_directory(os_name, WindowsDirectoryAccess::Read)
                    .map_err(|error| source_error(&member_path, &error))?;
                let destination_child = destination
                    .create_child_directory(os_name)
                    .map_err(filesystem("create a staged unit directory"))?;
                copy_tree(&source_child, &destination_child, manifest, &member_path)?;
            }
            Member::File { size, sha256, .. } => {
                let file = source
                    .open_file(os_name)
                    .map_err(|error| source_error(&member_path, &error))?;
                let mut staged = destination
                    .create_file(os_name)
                    .map_err(filesystem("create a staged unit file"))?;
                hash_exact(file, Some(&mut staged), *size, sha256)
                    .map_err(|error| source_error(&member_path, &error))?;
                staged
                    .sync_all()
                    .map_err(filesystem("flush a staged unit file"))?;
            }
        }
    }
    destination
        .sync()
        .map_err(filesystem("flush a staged unit directory"))
}

fn write_manifest(
    staging: &WindowsDirectory,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    let mut file = staging
        .create_file(OsStr::new(MANIFEST_NAME))
        .map_err(filesystem("create the staged manifest"))?;
    file.write_all(&manifest.bytes)
        .and_then(|()| file.sync_all())
        .map_err(filesystem("write the staged manifest"))?;
    staging
        .sync()
        .map_err(filesystem("flush the staged unit root"))
}

/// Prove an installed or staged unit exact: the same tree and digests as its
/// manifest, no reparse point, no entry an untrusted principal can change,
/// and every entry usable by a standard user as its class says.
fn validate_installed(
    store: &InstallStore,
    unit: &WindowsDirectory,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    let installed = read_installed_manifest(unit)?;
    if installed != manifest.bytes {
        return Err(ReleasePayloadError::InvalidUnit(
            "the unit's manifest.json differs from the release manifest".to_owned(),
        ));
    }
    require_trusted(store, unit, None, "")?;
    require_trusted(
        store,
        unit,
        Some((OsStr::new(MANIFEST_NAME), PermissionClass::Data)),
        MANIFEST_NAME,
    )?;
    validate_installed_tree(store, unit, manifest, "")
}

fn read_installed_manifest(unit: &WindowsDirectory) -> Result<Vec<u8>, ReleasePayloadError> {
    let file = unit
        .open_file(OsStr::new(MANIFEST_NAME))
        .map_err(filesystem("open the unit's manifest.json"))?;
    let mut bytes = Vec::new();
    file.take(MAX_RELEASE_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(filesystem("read the unit's manifest.json"))?;
    Ok(bytes)
}

fn validate_installed_tree(
    store: &InstallStore,
    directory: &WindowsDirectory,
    manifest: &ValidatedManifest,
    path: &str,
) -> Result<(), ReleasePayloadError> {
    require_exact_entries(directory, manifest, path, ReleasePayloadError::InvalidUnit)?;
    for name in &manifest.children[path] {
        let member_path = join(path, name);
        let os_name = OsStr::new(name);
        match &manifest.members[&member_path] {
            Member::Directory => {
                let child = directory
                    .open_child_directory(os_name, WindowsDirectoryAccess::Read)
                    .map_err(|error| unit_error(&member_path, &error))?;
                require_trusted(store, &child, None, &member_path)?;
                validate_installed_tree(store, &child, manifest, &member_path)?;
            }
            Member::File {
                class,
                size,
                sha256,
            } => {
                let file = directory
                    .open_file(os_name)
                    .map_err(|error| unit_error(&member_path, &error))?;
                hash_exact(file, None, *size, sha256)
                    .map_err(|error| unit_error(&member_path, &error))?;
                require_trusted(store, directory, Some((os_name, *class)), &member_path)?;
            }
        }
    }
    Ok(())
}

/// Require that no untrusted principal can change a unit entry, and that a
/// standard user can use it: traverse a directory, run a program, read data.
/// `file` names a child of `directory` and its class; without it the
/// directory itself is judged.
fn require_trusted(
    store: &InstallStore,
    directory: &WindowsDirectory,
    file: Option<(&OsStr, PermissionClass)>,
    path: &str,
) -> Result<(), ReleasePayloadError> {
    let security = match file {
        Some((name, _)) => directory.entry_security(name),
        None => directory.security(),
    }
    .map_err(|error| unit_error(path, &error))?;
    let execute = file.is_none_or(|(_, class)| class == PermissionClass::Program);
    let policy = store.ownership_policy();
    policy
        .require_installer_owned(&security)
        .and_then(|()| policy.require_usable(&security, execute))
        .map_err(|refusal| {
            ReleasePayloadError::InvalidUnit(format!(
                "{} cannot be trusted: {refusal}",
                display(path)
            ))
        })
}

/// Require that a directory holds exactly the manifest's children of `path`
/// (and `manifest.json` at the root), none a reparse point.
fn require_exact_entries(
    directory: &WindowsDirectory,
    manifest: &ValidatedManifest,
    path: &str,
    error: fn(String) -> ReleasePayloadError,
) -> Result<(), ReleasePayloadError> {
    let mut expected: Vec<String> = manifest.children[path]
        .iter()
        .map(|name| fold_case(name))
        .collect();
    if path.is_empty() {
        expected.push(fold_case(MANIFEST_NAME));
    }
    expected.sort();
    let mut actual = Vec::new();
    for name in directory
        .entries()
        .map_err(|source| error(format!("cannot list {}: {source}", display(path))))?
    {
        let Some(text) = name.to_str() else {
            return Err(error(format!(
                "{} holds a name that is not Unicode",
                display(path)
            )));
        };
        let metadata = directory
            .entry_metadata(&name)
            .map_err(|source| error(format!("cannot inspect {text}: {source}")))?;
        if metadata.is_some_and(|metadata| metadata.kind() == WindowsEntryKind::ReparsePoint) {
            return Err(error(format!(
                "{} holds a reparse point {text}",
                display(path)
            )));
        }
        actual.push(fold_case(text));
    }
    actual.sort();
    if actual == expected {
        Ok(())
    } else {
        Err(error(format!(
            "{} holds {actual:?}, not the manifest's {expected:?}",
            display(path)
        )))
    }
}

/// Read a whole file, copying it to `destination` when given, and require
/// its exact size and SHA-256.
fn hash_exact(
    mut file: std::fs::File,
    mut destination: Option<&mut std::fs::File>,
    size: u64,
    sha256: &str,
) -> io::Result<()> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file is longer than its manifest size",
            ));
        }
        hasher.update(&buffer[..read]);
        if let Some(destination) = destination.as_deref_mut() {
            destination.write_all(&buffer[..read])?;
        }
    }
    if total != size || hex::encode(hasher.finalize()) != sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the file does not match its manifest size and SHA-256",
        ));
    }
    Ok(())
}

fn create_staging(
    units: &WindowsDirectory,
) -> Result<(String, WindowsDirectory), ReleasePayloadError> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("{STAGE_PREFIX}{}-{sequence}", std::process::id());
        match units.create_child_directory(OsStr::new(&name)) {
            Ok(staging) => return Ok((name, staging)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(ReleasePayloadError::Filesystem {
                    operation: "create a release staging directory",
                    source,
                });
            }
        }
    }
    Err(ReleasePayloadError::StageCollisions {
        attempts: MAX_STAGE_ATTEMPTS,
    })
}

fn unit_record(
    store: &InstallStore,
    unit: &UnitId,
    directory: WindowsDirectory,
) -> Result<UnitRecord, ReleasePayloadError> {
    UnitRecord::new(unit.clone(), store.unit_path(unit), directory)
        .map_err(filesystem("bind the immutable unit identity"))
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn display(path: &str) -> &str {
    if path.is_empty() {
        "the unit root"
    } else {
        path
    }
}

fn source_error(path: &str, error: &io::Error) -> ReleasePayloadError {
    ReleasePayloadError::InvalidSource(format!("{path}: {error}"))
}

fn unit_error(path: &str, error: &io::Error) -> ReleasePayloadError {
    ReleasePayloadError::InvalidUnit(format!("{}: {error}", display(path)))
}

fn filesystem(operation: &'static str) -> impl Fn(io::Error) -> ReleasePayloadError {
    move |source| ReleasePayloadError::Filesystem { operation, source }
}

#[cfg(test)]
#[path = "payload_tests.rs"]
mod tests;
