use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::error::MacosOwnerStoreError;
use crate::journal::{MacosHandoverJournal, validate_handover_journal};
use crate::model::{
    MACOS_OWNER_RECORD_SCHEMA_VERSION, MAX_MACOS_OWNER_ARTIFACT_BYTES, MacosDaemonOwner,
    MacosDaemonSessionAttestation, MacosOwnerIdentity, MacosOwnerRecord,
};
use crate::validation::{validate_daemon_session_attestation, validate_owner_record};

const MAX_TEMPORARY_CREATE_ATTEMPTS: usize = 64;
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct CoordinationLock {
    pub(crate) file: File,
}

impl Drop for CoordinationLock {
    fn drop(&mut self) {
        drop(self.file.unlock());
    }
}

pub(crate) fn successor_owner_record(
    previous: MacosOwnerRecord,
    active_owner: MacosDaemonOwner,
    active_identity: MacosOwnerIdentity,
) -> Result<MacosOwnerRecord, MacosOwnerStoreError> {
    let owner_epoch = previous
        .owner_epoch
        .checked_add(1)
        .ok_or(MacosOwnerStoreError::OwnerEpochOverflow)?;
    let conflict = previous
        .conflict
        .filter(|conflict| {
            conflict.contender_owner != active_owner
                || conflict.contender_identity.executable_path != active_identity.executable_path
                || conflict.contender_identity.designated_requirement_hash
                    != active_identity.designated_requirement_hash
        })
        .map(|mut conflict| {
            conflict.active_owner = active_owner;
            conflict.active_epoch = owner_epoch;
            conflict
        });
    Ok(MacosOwnerRecord {
        owner_epoch,
        schema_version: MACOS_OWNER_RECORD_SCHEMA_VERSION,
        active_owner,
        active_identity,
        conflict,
        selected_external_owner: previous.selected_external_owner,
    })
}

pub(crate) fn read_owner_record(
    path: &Path,
) -> Result<Option<MacosOwnerRecord>, MacosOwnerStoreError> {
    let Some(bytes) = read_optional(path, "owner record")? else {
        return Ok(None);
    };
    let record = serde_json::from_slice::<MacosOwnerRecord>(&bytes).map_err(|source| {
        MacosOwnerStoreError::Decode {
            artifact: "owner record",
            source,
        }
    })?;
    validate_owner_record(&record)?;
    Ok(Some(record))
}

pub(crate) fn read_handover_journal(
    path: &Path,
) -> Result<Option<MacosHandoverJournal>, MacosOwnerStoreError> {
    let Some(bytes) = read_optional(path, "handover journal")? else {
        return Ok(None);
    };
    let journal = serde_json::from_slice::<MacosHandoverJournal>(&bytes).map_err(|source| {
        MacosOwnerStoreError::Decode {
            artifact: "handover journal",
            source,
        }
    })?;
    validate_handover_journal(&journal)?;
    Ok(Some(journal))
}

pub(crate) fn read_daemon_session_attestation(
    path: &Path,
) -> Result<Option<MacosDaemonSessionAttestation>, MacosOwnerStoreError> {
    let Some(bytes) = read_private_optional(path, "daemon session attestation")? else {
        return Ok(None);
    };
    let attestation =
        serde_json::from_slice::<MacosDaemonSessionAttestation>(&bytes).map_err(|source| {
            MacosOwnerStoreError::Decode {
                artifact: "daemon session attestation",
                source,
            }
        })?;
    validate_daemon_session_attestation(&attestation)?;
    Ok(Some(attestation))
}

fn read_private_optional(
    path: &Path,
    artifact: &'static str,
) -> Result<Option<Vec<u8>>, MacosOwnerStoreError> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(MacosOwnerStoreError::Read {
                artifact,
                path: path.to_path_buf(),
                source,
            });
        }
    };
    validate_private_file_metadata(&path_metadata, artifact)?;
    let file = File::open(path).map_err(|source| MacosOwnerStoreError::Read {
        artifact,
        path: path.to_path_buf(),
        source,
    })?;
    let file_metadata = file
        .metadata()
        .map_err(|source| MacosOwnerStoreError::Read {
            artifact,
            path: path.to_path_buf(),
            source,
        })?;
    validate_private_file_metadata(&file_metadata, artifact)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != file_metadata.dev() || path_metadata.ino() != file_metadata.ino()
        {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact,
                detail: "file changed while it was opened",
            });
        }
    }
    read_bounded_file(file, path, artifact).map(Some)
}

fn validate_private_file_metadata(
    metadata: &fs::Metadata,
    artifact: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact,
            detail: "must be a regular file",
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 {
            return Err(MacosOwnerStoreError::InvalidArtifact {
                artifact,
                detail: "mode must be 0600",
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        validate_private_file_uid(
            metadata.uid(),
            nix::unistd::Uid::current().as_raw(),
            artifact,
        )?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn validate_private_file_uid(
    owner_uid: u32,
    current_uid: u32,
    artifact: &'static str,
) -> Result<(), MacosOwnerStoreError> {
    if owner_uid != current_uid {
        return Err(MacosOwnerStoreError::InvalidArtifact {
            artifact,
            detail: "owner UID must match the current user",
        });
    }
    Ok(())
}

fn read_bounded_file(
    file: File,
    path: &Path,
    artifact: &'static str,
) -> Result<Vec<u8>, MacosOwnerStoreError> {
    let mut bytes = Vec::new();
    file.take((MAX_MACOS_OWNER_ARTIFACT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| MacosOwnerStoreError::Read {
            artifact,
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > MAX_MACOS_OWNER_ARTIFACT_BYTES {
        return Err(MacosOwnerStoreError::ArtifactTooLarge {
            artifact,
            maximum_bytes: MAX_MACOS_OWNER_ARTIFACT_BYTES,
        });
    }
    Ok(bytes)
}

fn read_optional(
    path: &Path,
    artifact: &'static str,
) -> Result<Option<Vec<u8>>, MacosOwnerStoreError> {
    match File::open(path) {
        Ok(file) => read_bounded_file(file, path, artifact).map(Some),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MacosOwnerStoreError::Read {
            artifact,
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(crate) fn write_json_atomic<T>(
    data_dir: &Path,
    path: &Path,
    artifact: &'static str,
    value: &T,
) -> Result<(), MacosOwnerStoreError>
where
    T: Serialize + ?Sized,
{
    let mut payload = serde_json::to_vec_pretty(value)
        .map_err(|source| MacosOwnerStoreError::Encode { artifact, source })?;
    payload.push(b'\n');
    if payload.len() > MAX_MACOS_OWNER_ARTIFACT_BYTES {
        return Err(MacosOwnerStoreError::ArtifactTooLarge {
            artifact,
            maximum_bytes: MAX_MACOS_OWNER_ARTIFACT_BYTES,
        });
    }
    let (mut temporary, temporary_path) = create_temporary_file(data_dir, path)?;
    let result = (|| {
        temporary
            .write_all(&payload)
            .map_err(|source| MacosOwnerStoreError::WriteTemporary {
                path: path.to_path_buf(),
                source,
            })?;
        temporary
            .sync_all()
            .map_err(|source| MacosOwnerStoreError::SyncTemporary {
                path: path.to_path_buf(),
                source,
            })?;
        drop(temporary);
        hypercolor_platform_fs::replace_file(&temporary_path, path).map_err(|source| {
            MacosOwnerStoreError::Replace {
                path: path.to_path_buf(),
                source,
            }
        })?;
        sync_parent_directory(data_dir)
    })();
    if result.is_err() {
        drop(fs::remove_file(&temporary_path));
    }
    result
}

fn create_temporary_file(
    data_dir: &Path,
    path: &Path,
) -> Result<(File, PathBuf), MacosOwnerStoreError> {
    for _ in 0..MAX_TEMPORARY_CREATE_ATTEMPTS {
        let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_path = data_dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("macos-owner"),
            std::process::id(),
            sequence
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary_path) {
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))
                        .map_err(|source| MacosOwnerStoreError::CreateTemporary {
                            path: path.to_path_buf(),
                            source,
                        })?;
                }
                return Ok((file, temporary_path));
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(MacosOwnerStoreError::CreateTemporary {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }
    Err(MacosOwnerStoreError::CreateTemporary {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "temporary file collision limit reached",
        ),
    })
}

#[cfg(unix)]
pub(crate) fn sync_parent_directory(data_dir: &Path) -> Result<(), MacosOwnerStoreError> {
    File::open(data_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| MacosOwnerStoreError::SyncDirectory {
            path: data_dir.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fallible Unix implementation"
)]
pub(crate) fn sync_parent_directory(_data_dir: &Path) -> Result<(), MacosOwnerStoreError> {
    Ok(())
}
