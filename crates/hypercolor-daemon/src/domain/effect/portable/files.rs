//! Verified file staging and resumable publication for one restore operation.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use hypercolor_core::portable_setup::SetupBlobStore;
use hypercolor_types::portable_setup::PortableBlob;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::domain::DomainError;

pub(super) fn io(error: impl std::fmt::Display) -> DomainError {
    DomainError::Internal(anyhow::anyhow!(error.to_string()))
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

pub(super) fn directory(path: &Path) -> Result<PathBuf, DomainError> {
    let mut missing = Vec::new();
    let mut ancestor = path;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if ancestor == path && metadata.file_type().is_symlink() {
                    return Err(DomainError::conflict(
                        "effect restore root is a symbolic link",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| DomainError::validation("effect directory has no name"))?
                        .to_owned(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| DomainError::validation("effect directory has no parent"))?;
            }
            Err(error) => return Err(io(error)),
        }
    }
    // Trusted local roots may pass through an OS alias such as macOS /var.
    // Resolve that prefix once; resource paths only use the canonical result.
    let mut current = ancestor.canonicalize().map_err(io)?;
    if !current.is_dir() {
        return Err(DomainError::conflict(
            "effect restore root is not a directory",
        ));
    }
    for name in missing.into_iter().rev() {
        let child = current.join(name);
        fs::create_dir(&child).map_err(io)?;
        sync_directory(&current)?;
        sync_directory(&child)?;
        current = child;
    }
    Ok(current)
}

pub(super) fn nested_directory(root: &Path, relative: &Path) -> Result<PathBuf, DomainError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(DomainError::validation(
                "effect resource path escapes its bundle",
            ));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(DomainError::conflict(
                    "effect resource directory is not an owned directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(io)?;
                sync_directory(current.parent().expect("child directory"))?;
                sync_directory(&current)?;
            }
            Err(error) => return Err(io(error)),
        }
    }
    Ok(current)
}

pub(super) fn sync_directory(path: &Path) -> Result<(), DomainError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(io)
    }
    #[cfg(target_os = "windows")]
    {
        hypercolor_platform_fs::WindowsDirectory::open(
            path,
            hypercolor_platform_fs::WindowsDirectoryAccess::Write,
        )
        .and_then(|directory| directory.sync())
        .map_err(io)
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        Err(DomainError::validation(
            "durable effect directories are unsupported",
        ))
    }
}

pub(super) fn verify(path: &Path, descriptor: &PortableBlob) -> Result<bool, DomainError> {
    for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            _ => {
                return Err(DomainError::conflict(
                    "effect resource parent is not an owned directory",
                ));
            }
        }
    }
    let mut file = match hypercolor_platform_fs::open_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io(error)),
    };
    if !file.metadata().map_err(io)?.is_file() {
        return Err(DomainError::conflict(
            "effect resource is not a regular file",
        ));
    }
    let actual = hash_copy(&mut file, &mut std::io::sink())?;
    if actual != *descriptor {
        return Err(DomainError::conflict(format!(
            "effect resource changed: {}",
            path.display()
        )));
    }
    Ok(true)
}

pub(super) fn copy_blob(
    blobs: &SetupBlobStore,
    descriptor: &PortableBlob,
    root: &Path,
    relative: &Path,
) -> Result<(), DomainError> {
    let parent = nested_directory(root, relative.parent().unwrap_or_else(|| Path::new("")))?;
    let destination = root.join(relative);
    if verify(&destination, descriptor)? {
        return sync_directory(&parent);
    }
    let mut source = blobs.read(descriptor).map_err(io)?.file;
    let mut staged = NamedTempFile::new_in(&parent).map_err(io)?;
    let actual = hash_copy(&mut source, staged.as_file_mut())?;
    if actual != *descriptor {
        return Err(DomainError::conflict("effect blob changed during staging"));
    }
    staged.as_file_mut().sync_all().map_err(io)?;
    hypercolor_platform_fs::durable_replace(staged.path(), &destination).map_err(io)
}

pub(super) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), DomainError> {
    let parent = path
        .parent()
        .ok_or_else(|| DomainError::validation("effect file has no parent"))?;
    let descriptor = PortableBlob {
        sha256: digest(bytes),
        byte_len: u64::try_from(bytes.len()).map_err(io)?,
    };
    if verify(path, &descriptor)? {
        return sync_directory(parent);
    }
    let mut staged = NamedTempFile::new_in(parent).map_err(io)?;
    staged.write_all(bytes).map_err(io)?;
    staged.as_file_mut().sync_all().map_err(io)?;
    hypercolor_platform_fs::durable_replace(staged.path(), path).map_err(io)
}

pub(super) fn remove_verified(path: &Path, descriptor: &PortableBlob) -> Result<(), DomainError> {
    if verify(path, descriptor)? {
        fs::remove_file(path).map_err(io)?;
    }
    sync_directory(
        path.parent()
            .ok_or_else(|| DomainError::validation("effect file has no parent"))?,
    )
}

fn hash_copy(
    source: &mut impl Read,
    destination: &mut impl Write,
) -> Result<PortableBlob, DomainError> {
    let mut hash = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = source.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        destination.write_all(&buffer[..read]).map_err(io)?;
        length = length
            .checked_add(u64::try_from(read).map_err(io)?)
            .ok_or_else(|| DomainError::validation("effect resource is too large"))?;
    }
    Ok(PortableBlob {
        sha256: hex_bytes(&hash.finalize()),
        byte_len: length,
    })
}

/// Enumerate only regular files; linked directories are never restore ownership.
pub(super) fn inventory(
    root: &Path,
    skip: &BTreeSet<PathBuf>,
) -> Result<BTreeSet<String>, DomainError> {
    let mut pending = vec![root.to_owned()];
    let mut names = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            if skip.contains(&path) {
                continue;
            }
            let kind = entry.file_type().map_err(io)?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                names.insert(
                    path.strip_prefix(root)
                        .map_err(io)?
                        .to_str()
                        .ok_or_else(|| DomainError::validation("effect path is not UTF-8"))?
                        .replace('\\', "/"),
                );
            } else {
                return Err(DomainError::conflict(
                    "effect bundle contains a linked or special resource",
                ));
            }
        }
    }
    Ok(names)
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            write!(&mut text, "{byte:02x}").expect("writing into a String cannot fail");
            text
        })
}
