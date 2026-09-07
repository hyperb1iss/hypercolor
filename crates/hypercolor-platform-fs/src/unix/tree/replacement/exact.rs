use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read as _};
use std::mem::MaybeUninit;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{AtFlags, readlinkat_raw, unlinkat};
use sha2::{Digest as _, Sha256};

use super::super::traversal::{
    entry_metadata_at, metadata_for_file, open_regular_file_at, require_same_entry, unsafe_entry,
};
use super::super::{DirectoryEntryKind, DirectoryEntryMetadata, ExactEntry};

/// Maximum regular-file size accepted by exact public entry observation.
pub const MAX_EXACT_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PUBLIC_SYMLINK_TARGET_BYTES: usize = 4096;

pub(super) enum RetainedExpectedEntry {
    NoHandle,
    RegularFile {
        file: File,
        metadata: DirectoryEntryMetadata,
    },
}

impl RetainedExpectedEntry {
    pub(super) fn open(directory: &File, name: &OsStr, expected: &ExactEntry) -> io::Result<Self> {
        let ExactEntry::RegularFile { .. } = expected else {
            return Ok(Self::NoHandle);
        };
        let mut opened = open_regular_file_at(directory, name)?;
        let sha256 = hash_file(opened.file_mut())?;
        let metadata = metadata_for_file(opened.file())?;
        let exact = exact_regular_entry(metadata, sha256)?;
        require_expected(
            expected,
            &exact,
            "retained public replacement handle does not match expectation",
        )?;
        let named = entry_metadata_at(directory, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public replacement destination disappeared while retaining its handle",
            )
        })?;
        require_exact_metadata(
            metadata,
            named,
            "public replacement destination changed while retaining its handle",
        )?;
        Ok(Self::RegularFile {
            file: opened.into_file(),
            metadata,
        })
    }

    pub(super) fn validate_at(&self, directory: &File, name: &OsStr) -> io::Result<()> {
        let Self::RegularFile { file, metadata } = self else {
            return Ok(());
        };
        let handle = metadata_for_file(file)?;
        require_exact_metadata(
            *metadata,
            handle,
            "retained public replacement handle metadata changed",
        )?;
        let named = entry_metadata_at(directory, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "retained public replacement entry disappeared",
            )
        })?;
        require_exact_metadata(
            handle,
            named,
            "displaced public replacement does not match its retained handle",
        )
    }
}

pub(super) fn require_exact_metadata(
    expected: DirectoryEntryMetadata,
    actual: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if expected != actual {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

pub(super) fn remove_exact_entry(
    directory: &File,
    name: &OsStr,
    expected: &ExactEntry,
) -> io::Result<()> {
    require_expected_at(
        directory,
        name,
        expected,
        "private public-entry cleanup target changed",
    )?;
    unlinkat(directory, name, AtFlags::empty()).map_err(io::Error::from)?;
    directory.sync_all()
}

pub(super) fn require_expected(
    expected: &ExactEntry,
    actual: &ExactEntry,
    message: &'static str,
) -> io::Result<()> {
    if expected != actual {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

pub(super) fn require_expected_at(
    directory: &File,
    name: &OsStr,
    expected: &ExactEntry,
    message: &'static str,
) -> io::Result<()> {
    if !exact_entry_matches_at(directory, name, expected)? {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

pub(super) fn exact_entry_matches_at(
    directory: &File,
    name: &OsStr,
    expected: &ExactEntry,
) -> io::Result<bool> {
    match expected {
        ExactEntry::Absent => Ok(entry_metadata_at(directory, name)?.is_none()),
        ExactEntry::RegularFile {
            mode,
            size,
            device,
            inode,
            ..
        } => {
            let Some(metadata) = entry_metadata_at(directory, name)? else {
                return Ok(false);
            };
            if metadata.kind != DirectoryEntryKind::RegularFile
                || metadata.link_count != 1
                || metadata.mode != *mode
                || metadata.size != *size
                || metadata.device != *device
                || metadata.inode != *inode
                || metadata.size > MAX_EXACT_ENTRY_BYTES
            {
                return Ok(false);
            }
            match observe_entry_at(directory, name) {
                Ok(actual) => Ok(&actual == expected),
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(false),
                Err(error) => Err(error),
            }
        }
        ExactEntry::Symlink {
            target,
            device,
            inode,
        } => symlink_matches_at(directory, name, target, *device, *inode),
    }
}

fn symlink_matches_at(
    directory: &File,
    name: &OsStr,
    target: &Path,
    device: u64,
    inode: u64,
) -> io::Result<bool> {
    let Some(before) = entry_metadata_at(directory, name)? else {
        return Ok(false);
    };
    if before.kind != DirectoryEntryKind::SymbolicLink
        || before.link_count != 1
        || before.device != device
        || before.inode != inode
        || before.size > MAX_PUBLIC_SYMLINK_TARGET_BYTES as u64
    {
        return Ok(false);
    }
    let mut buffer = [MaybeUninit::<u8>::uninit(); MAX_PUBLIC_SYMLINK_TARGET_BYTES];
    let (bytes, _) = readlinkat_raw(directory, name, &mut buffer[..]).map_err(io::Error::from)?;
    let after = entry_metadata_at(directory, name)?;
    Ok(after == Some(before)
        && bytes.len() as u64 == before.size
        && bytes == target.as_os_str().as_bytes())
}

pub(super) fn combine_cleanup<T>(
    error: io::Error,
    cleanup: io::Result<()>,
    label: &'static str,
) -> io::Result<T> {
    match cleanup {
        Ok(()) => Err(error),
        Err(cleanup_error) => Err(io::Error::other(format!(
            "{error}; {label} failed: {cleanup_error}"
        ))),
    }
}

pub(super) fn combine_two_cleanups<T>(
    error: io::Error,
    first: io::Result<()>,
    first_label: &'static str,
    second: io::Result<()>,
    second_label: &'static str,
) -> io::Result<T> {
    let error = match first {
        Ok(()) => error,
        Err(cleanup_error) => {
            io::Error::other(format!("{error}; {first_label} failed: {cleanup_error}"))
        }
    };
    match second {
        Ok(()) => Err(error),
        Err(cleanup_error) => Err(io::Error::other(format!(
            "{error}; {second_label} failed: {cleanup_error}"
        ))),
    }
}

pub(super) fn observe_entry_at(directory: &File, name: &OsStr) -> io::Result<ExactEntry> {
    let Some(metadata) = entry_metadata_at(directory, name)? else {
        return Ok(ExactEntry::Absent);
    };
    match metadata.kind {
        DirectoryEntryKind::RegularFile => observe_regular_file(directory, name, metadata),
        DirectoryEntryKind::SymbolicLink => observe_symlink(directory, name, metadata),
        DirectoryEntryKind::Directory => Err(unsafe_entry(
            "public entry observation refuses directory destinations",
        )),
        DirectoryEntryKind::Special => Err(unsafe_entry(
            "public entry observation refuses special destinations",
        )),
    }
}

fn observe_regular_file(
    directory: &File,
    name: &OsStr,
    named: DirectoryEntryMetadata,
) -> io::Result<ExactEntry> {
    if named.size > MAX_EXACT_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "public regular file exceeds the exact observation limit",
        ));
    }
    let mut opened = open_regular_file_at(directory, name)?;
    require_same_entry(
        named,
        opened.metadata,
        "public regular file changed before hashing",
    )?;
    let sha256 = hash_file(opened.file_mut())?;
    let handle_after = metadata_for_file(opened.file())?;
    require_same_entry(
        named,
        handle_after,
        "public regular file handle changed while hashing",
    )?;
    if handle_after.mode != named.mode || handle_after.size != named.size {
        return Err(unsafe_entry(
            "public regular file metadata changed while hashing",
        ));
    }
    let named_after = entry_metadata_at(directory, name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "public regular file disappeared while hashing",
        )
    })?;
    if named_after != named {
        return Err(unsafe_entry(
            "public regular file name metadata changed while hashing",
        ));
    }
    exact_regular_entry(named, sha256)
}

pub(super) fn exact_regular_entry(
    metadata: DirectoryEntryMetadata,
    sha256: [u8; 32],
) -> io::Result<ExactEntry> {
    if metadata.kind != DirectoryEntryKind::RegularFile
        || metadata.link_count != 1
        || metadata.mode & !0o777 != 0
        || metadata.size > MAX_EXACT_ENTRY_BYTES
    {
        return Err(unsafe_entry(
            "retained public entry is not an exact ordinary regular file",
        ));
    }
    Ok(ExactEntry::RegularFile {
        mode: metadata.mode,
        size: metadata.size,
        sha256,
        device: metadata.device,
        inode: metadata.inode,
    })
}

fn observe_symlink(
    directory: &File,
    name: &OsStr,
    named: DirectoryEntryMetadata,
) -> io::Result<ExactEntry> {
    if named.link_count != 1 || named.size > MAX_PUBLIC_SYMLINK_TARGET_BYTES as u64 {
        return Err(unsafe_entry(
            "public symbolic link is multiply linked or oversized",
        ));
    }
    let mut buffer = [MaybeUninit::<u8>::uninit(); MAX_PUBLIC_SYMLINK_TARGET_BYTES];
    let (bytes, _) = readlinkat_raw(directory, name, &mut buffer[..]).map_err(io::Error::from)?;
    if bytes.len() as u64 != named.size {
        return Err(unsafe_entry(
            "public symbolic link size changed while reading",
        ));
    }
    let target = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    validate_public_symlink_target(&target)?;
    let named_after = entry_metadata_at(directory, name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "public symbolic link disappeared while reading",
        )
    })?;
    if named_after != named {
        return Err(unsafe_entry(
            "public symbolic link metadata changed while reading",
        ));
    }
    Ok(ExactEntry::Symlink {
        target,
        device: named.device,
        inode: named.inode,
    })
}

pub(super) fn validate_public_symlink_target(target: &Path) -> io::Result<()> {
    if target.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public symbolic-link target must not be empty",
        ));
    }
    let mut meaningful = false;
    for component in target.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(_) | Component::ParentDir => meaningful = true,
            Component::CurDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "public symbolic-link target has an unsupported component",
                ));
            }
        }
    }
    if !meaningful {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public symbolic-link target must name an entry",
        ));
    }
    Ok(())
}

pub(super) fn hash_file(file: &mut File) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

pub(super) fn remove_exact_empty_directory(
    parent: &File,
    name: &OsStr,
    directory: &File,
) -> io::Result<()> {
    let expected = metadata_for_file(directory)?;
    let current = entry_metadata_at(parent, name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "created public directory disappeared before cleanup",
        )
    })?;
    require_same_entry(
        expected,
        current,
        "created public directory changed before cleanup",
    )?;
    unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
    parent.sync_all()
}
