//! Streamed, content-addressed storage for portable setup archives and sync.

pub mod archive;

use hypercolor_platform_fs::FileChangeStamp;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hypercolor_types::portable_setup::{PortableBlob, PortableSetupError};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

/// An opened, verified immutable blob. Consumers retain the file handle rather
/// than reopening an unverified path after validation.
#[derive(Debug)]
pub struct SetupBlob {
    pub descriptor: PortableBlob,
    pub file: File,
}

#[derive(Debug, thiserror::Error)]
pub enum SetupBlobError {
    #[error(transparent)]
    Manifest(#[from] PortableSetupError),
    #[error("portable blob I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("portable blob differs from its declared digest or byte length")]
    ContentMismatch,
    #[error("portable blob source changed while it was captured")]
    SourceChanged,
}

/// Private staging directory owned by one installation. Metadata documents
/// never supply filesystem destinations; only checked digests select objects.
#[derive(Debug, Clone)]
pub struct SetupBlobStore {
    root: PathBuf,
    captured: Arc<Mutex<HashMap<PathBuf, (FileChangeStamp, PortableBlob)>>>,
    verified: Arc<Mutex<HashMap<String, (FileChangeStamp, u64)>>>,
}

impl SetupBlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, SetupBlobError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
            captured: Arc::default(),
            verified: Arc::default(),
        })
    }

    /// Stream and verify an object before publishing its content-addressed name.
    /// Memory usage stays fixed regardless of media size.
    pub fn stage(
        &self,
        descriptor: &PortableBlob,
        mut source: impl Read,
    ) -> Result<(), SetupBlobError> {
        descriptor.validate()?;
        let destination = self.root.join(&descriptor.sha256);
        let mut temporary = NamedTempFile::new_in(&self.root)?;
        let actual = copy_hashed(
            &mut source,
            temporary.as_file_mut(),
            Some(descriptor.byte_len),
        )?;
        if actual != *descriptor {
            return Err(SetupBlobError::ContentMismatch);
        }
        temporary.as_file_mut().sync_all()?;
        hypercolor_platform_fs::durable_replace(temporary.path(), &destination)?;
        self.remember_verified(descriptor)?;
        Ok(())
    }

    /// Add a local authored file while computing its portable descriptor.
    pub fn capture(&self, path: &Path) -> Result<PortableBlob, SetupBlobError> {
        let mut source = hypercolor_platform_fs::open_no_follow(path)?;
        if !source.metadata()?.is_file() {
            return Err(std::io::Error::other("blob source is not a regular file").into());
        }
        let before = hypercolor_platform_fs::file_change_stamp(&source)?;
        let cached = self
            .captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .cloned();
        if let Some((stamp, descriptor)) = cached
            && before == Some(stamp)
            && self.read(&descriptor).is_ok()
        {
            return Ok(descriptor);
        }
        let mut temporary = NamedTempFile::new_in(&self.root)?;
        let descriptor = copy_hashed(&mut source, temporary.as_file_mut(), None)?;
        if before != hypercolor_platform_fs::file_change_stamp(&source)? {
            return Err(SetupBlobError::SourceChanged);
        }
        temporary.as_file_mut().sync_all()?;
        hypercolor_platform_fs::durable_replace(
            temporary.path(),
            &self.root.join(&descriptor.sha256),
        )?;
        self.remember_verified(&descriptor)?;
        if let Some(stamp) = before {
            self.captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(path.to_owned(), (stamp, descriptor.clone()));
        }
        Ok(descriptor)
    }

    /// Read through a verified handle. Corruption and truncated transfers are
    /// reported before any restore operation can consume the object.
    pub fn read(&self, descriptor: &PortableBlob) -> Result<SetupBlob, SetupBlobError> {
        descriptor.validate()?;
        let mut file = hypercolor_platform_fs::open_no_follow(&self.root.join(&descriptor.sha256))?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("blob is not a regular file").into());
        }
        let before = hypercolor_platform_fs::file_change_stamp(&file)?;
        let cached = self
            .verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&descriptor.sha256)
            .copied();
        if let Some((stamp, length)) = cached
            && before == Some(stamp)
            && length == descriptor.byte_len
        {
            return Ok(SetupBlob {
                descriptor: descriptor.clone(),
                file,
            });
        }
        let actual = copy_hashed(&mut file, &mut std::io::sink(), Some(descriptor.byte_len))?;
        if actual != *descriptor {
            return Err(SetupBlobError::ContentMismatch);
        }
        if before != hypercolor_platform_fs::file_change_stamp(&file)? {
            return Err(SetupBlobError::SourceChanged);
        }
        if let Some(stamp) = before {
            self.verified
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(descriptor.sha256.clone(), (stamp, descriptor.byte_len));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(SetupBlob {
            descriptor: descriptor.clone(),
            file,
        })
    }

    fn remember_verified(&self, descriptor: &PortableBlob) -> Result<(), SetupBlobError> {
        self.read(descriptor)?;
        Ok(())
    }
}

fn copy_hashed(
    source: &mut impl Read,
    destination: &mut impl Write,
    expected_length: Option<u64>,
) -> Result<PortableBlob, SetupBlobError> {
    let mut digest = Sha256::new();
    let mut byte_len = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or(SetupBlobError::ContentMismatch)?;
        if expected_length.is_some_and(|expected| byte_len > expected) {
            return Err(SetupBlobError::ContentMismatch);
        }
        digest.update(&buffer[..read]);
        destination.write_all(&buffer[..read])?;
    }
    let mut sha256 = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut sha256, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(PortableBlob { sha256, byte_len })
}
