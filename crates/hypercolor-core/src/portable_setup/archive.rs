//! Streaming backup container. Configuration imports verify and discard old
//! resource payloads; durable resource transactions retain a separate reader.

use super::{SetupBlobError, SetupBlobStore};
use hypercolor_types::portable_setup::{PortableBackupV1, PortableBlob, PortableSetupError};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};

const MANIFEST: &str = "manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum BackupArchiveError {
    #[error(transparent)]
    Manifest(#[from] PortableSetupError),
    #[error(transparent)]
    Blob(#[from] SetupBlobError),
    #[error("backup JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("backup I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("backup container is invalid: {0}")]
    Container(String),
}

/// Write a deterministic tar archive with one JSON manifest and one entry per
/// content digest. Media bytes never accumulate in memory. A caller publishing
/// a file should write to a temporary file and rename it only after success.
pub fn write_archive(
    output: impl Write,
    backup: &PortableBackupV1,
    blobs: &SetupBlobStore,
) -> Result<(), BackupArchiveError> {
    backup.validate()?;
    let descriptors = backup.blobs()?;
    // Tar needs the manifest length before its header. A temporary file avoids
    // holding a second serialized copy of a potentially large authored setup.
    let mut manifest = tempfile::tempfile()?;
    serde_json::to_writer(&mut manifest, backup)?;
    let length = manifest.stream_position()?;
    manifest.seek(SeekFrom::Start(0))?;
    let mut archive = tar::Builder::new(output);
    append(&mut archive, MANIFEST, length, &mut manifest)?;
    for descriptor in descriptors.values() {
        let blob = blobs.read(descriptor)?;
        let mut source = CheckedReader {
            source: blob.file,
            digest: Sha256::new(),
            bytes: 0,
        };
        append(
            &mut archive,
            &blob_path(descriptor),
            descriptor.byte_len,
            &mut source,
        )?;
        // Check the bytes actually emitted, including concurrent source edits
        // after the blob store's verified handle was acquired.
        source.verify(descriptor)?;
    }
    archive.finish()?;
    archive.into_inner()?.flush()?;
    Ok(())
}

/// Parse and verify the complete container before returning its manifest.
/// Unreferenced entries, duplicates, links, path aliases, corrupt/truncated
/// objects, and trailing archives are rejected. Valid objects staged before a
/// later error remain harmless content-addressed cache entries.
pub fn read_archive(
    source: impl Read,
    blobs: &SetupBlobStore,
) -> Result<PortableBackupV1, BackupArchiveError> {
    read_archive_impl(source, Some(blobs))
}

/// Validate old or new backups without retaining resource payloads. The raw
/// manifest remains available for operation identity; restore applies only its
/// configuration and preserves all destination-local resources.
pub fn read_configuration_archive(
    source: impl Read,
) -> Result<PortableBackupV1, BackupArchiveError> {
    read_archive_impl(source, None)
}

fn read_archive_impl(
    source: impl Read,
    blobs: Option<&SetupBlobStore>,
) -> Result<PortableBackupV1, BackupArchiveError> {
    let mut archive = tar::Archive::new(source);
    let backup;
    {
        // Raw iteration exposes extension/link headers instead of interpreting
        // them as filesystem instructions or alternate entry names.
        let mut entries = archive.entries()?.raw(true);
        let mut manifest = entries
            .next()
            .ok_or_else(|| invalid("missing manifest"))??;
        regular_entry(&manifest)?;
        if manifest.path_bytes().as_ref() != MANIFEST.as_bytes() {
            return Err(invalid("manifest must be the first entry"));
        }
        backup = serde_json::from_reader::<_, PortableBackupV1>(&mut manifest)?;
        backup.validate()?;
        drop(manifest);
        let mut remaining = backup.blobs()?;
        for entry in entries {
            let mut entry = entry?;
            regular_entry(&entry)?;
            let path = entry.path_bytes();
            let digest = std::str::from_utf8(&path)
                .ok()
                .and_then(|path| path.strip_prefix("blobs/"))
                .ok_or_else(|| invalid("unexpected entry path"))?;
            let descriptor = remaining
                .remove(digest)
                .ok_or_else(|| invalid("unreferenced or duplicate object"))?;
            if entry.size() != descriptor.byte_len {
                return Err(invalid("object size differs from the manifest"));
            }
            if let Some(blobs) = blobs {
                blobs.stage(&descriptor, &mut entry)?;
            } else {
                let mut source = CheckedReader {
                    source: &mut entry,
                    digest: Sha256::new(),
                    bytes: 0,
                };
                io::copy(&mut source, &mut io::sink())?;
                source.verify(&descriptor)?;
            }
        }
        if !remaining.is_empty() {
            return Err(invalid("referenced objects are missing"));
        }
    }
    // tar stops at its first zero block (and also tolerates bare EOF). Require
    // the second terminator block and reject appended nonzero payloads.
    let mut remainder = archive.into_inner();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = remainder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if buffer[..count].iter().any(|byte| *byte != 0) {
            return Err(invalid("nonzero data follows the archive terminator"));
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| invalid("trailer length overflow"))?;
    }
    if bytes < 512 || !bytes.is_multiple_of(512) {
        return Err(invalid("missing or truncated archive terminator"));
    }
    Ok(backup)
}

fn append(
    archive: &mut tar::Builder<impl Write>,
    path: &str,
    length: u64,
    source: impl Read,
) -> io::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(length);
    archive.append_data(&mut header, path, source)
}

fn regular_entry<R: Read>(entry: &tar::Entry<'_, R>) -> Result<(), BackupArchiveError> {
    if !entry.header().entry_type().is_file() || entry.header().link_name_bytes().is_some() {
        return Err(invalid("only ordinary files are accepted"));
    }
    Ok(())
}

fn blob_path(descriptor: &PortableBlob) -> String {
    format!("blobs/{}", descriptor.sha256)
}

fn invalid(message: &str) -> BackupArchiveError {
    BackupArchiveError::Container(message.into())
}

struct CheckedReader<R> {
    source: R,
    digest: Sha256,
    bytes: u64,
}

impl<R: Read> Read for CheckedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.source.read(buffer)?;
        self.digest.update(&buffer[..count]);
        self.bytes = self
            .bytes
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("blob length overflow"))?;
        Ok(count)
    }
}

impl<R: Read> CheckedReader<R> {
    fn verify(self, descriptor: &PortableBlob) -> Result<(), BackupArchiveError> {
        use std::fmt::Write as _;
        let mut digest = String::with_capacity(64);
        for byte in self.digest.finalize() {
            write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
        }
        if self.bytes != descriptor.byte_len || digest != descriptor.sha256 {
            return Err(SetupBlobError::ContentMismatch.into());
        }
        Ok(())
    }
}
