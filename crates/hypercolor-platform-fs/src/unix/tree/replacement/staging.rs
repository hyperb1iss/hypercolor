use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{AtFlags, OFlags, openat, symlinkat, unlinkat};
use rustix::io::Errno;
use sha2::{Digest as _, Sha256};

use super::super::traversal::{
    entry_metadata_at, metadata_for_file, require_same_entry, rustix_mode, unsafe_entry,
    validate_mode,
};
use super::super::{DirectoryEntryKind, EntryReplacement, ExactEntry, SECRET_FILE_MODE};
use super::exact::{
    MAX_EXACT_ENTRY_BYTES, combine_cleanup, observe_entry_at, require_exact_metadata,
    validate_public_symlink_target,
};

pub(super) const MAX_STAGE_ATTEMPTS: usize = 128;
pub(super) static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct StagedEntry {
    pub(super) name: OsString,
    pub(super) exact: ExactEntry,
}

pub(super) fn stage_replacement(
    directory: &File,
    replacement: EntryReplacement<'_>,
) -> io::Result<StagedEntry> {
    stage_replacement_with(directory, replacement, |_| Ok(()))
}

pub(super) fn stage_replacement_with(
    directory: &File,
    replacement: EntryReplacement<'_>,
    after_create: impl FnOnce(&OsStr) -> io::Result<()>,
) -> io::Result<StagedEntry> {
    match replacement {
        EntryReplacement::RegularFile { mode, contents } => {
            validate_mode(mode)?;
            if u64::try_from(contents.len()).map_or(true, |size| size > MAX_EXACT_ENTRY_BYTES) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "public replacement contents exceed the exact entry limit",
                ));
            }
            for _ in 0..MAX_STAGE_ATTEMPTS {
                let name = next_stage_name();
                match openat(
                    directory,
                    &name,
                    OFlags::RDWR
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::CLOEXEC
                        | OFlags::NOFOLLOW,
                    rustix_mode(SECRET_FILE_MODE)?,
                ) {
                    Ok(file) => {
                        let mut file = File::from(file);
                        let result = (|| {
                            after_create(&name)?;
                            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
                            std::io::Write::write_all(&mut file, contents)?;
                            file.sync_all()?;
                            let expected = metadata_for_file(&file)?;
                            let named = entry_metadata_at(directory, &name)?.ok_or_else(|| {
                                unsafe_entry("staged public regular file disappeared")
                            })?;
                            require_same_entry(
                                expected,
                                named,
                                "staged public regular file identity changed",
                            )?;
                            let exact = observe_entry_at(directory, &name)?;
                            validate_staged_regular(&exact, mode, contents)?;
                            Ok(exact)
                        })();
                        return match result {
                            Ok(exact) => Ok(StagedEntry { name, exact }),
                            Err(error) => {
                                let cleanup = remove_created_regular_stage(directory, &name, &file);
                                combine_cleanup(error, cleanup, "staged regular file cleanup")
                            }
                        };
                    }
                    Err(Errno::EXIST) => {}
                    Err(error) => return Err(io::Error::from(error)),
                }
            }
        }
        EntryReplacement::Symlink { target } => {
            validate_public_symlink_target(target)?;
            for _ in 0..MAX_STAGE_ATTEMPTS {
                let name = next_stage_name();
                match symlinkat(target, directory, &name) {
                    Ok(()) => {
                        let result = (|| {
                            after_create(&name)?;
                            let exact = observe_entry_at(directory, &name)?;
                            validate_staged_symlink(&exact, target)?;
                            Ok(exact)
                        })();
                        return result.map(|exact| StagedEntry { name, exact });
                    }
                    Err(Errno::EXIST) => {}
                    Err(error) => return Err(io::Error::from(error)),
                }
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a staged public replacement entry",
    ))
}

fn validate_staged_regular(exact: &ExactEntry, mode: u32, contents: &[u8]) -> io::Result<()> {
    let ExactEntry::RegularFile {
        mode: actual_mode,
        size,
        sha256,
        ..
    } = exact
    else {
        return Err(unsafe_entry("staged public regular file changed kind"));
    };
    let expected_size = u64::try_from(contents.len())
        .map_err(|_| unsafe_entry("staged public regular file size does not fit u64"))?;
    let expected_sha256: [u8; 32] = Sha256::digest(contents).into();
    if *actual_mode != mode || *size != expected_size || *sha256 != expected_sha256 {
        return Err(unsafe_entry(
            "staged public regular file does not match requested contents",
        ));
    }
    Ok(())
}

fn validate_staged_symlink(exact: &ExactEntry, target: &Path) -> io::Result<()> {
    if !matches!(exact, ExactEntry::Symlink { target: actual, .. } if actual == target) {
        return Err(unsafe_entry(
            "staged public symbolic link does not match requested target",
        ));
    }
    Ok(())
}

fn remove_created_regular_stage(directory: &File, name: &OsStr, file: &File) -> io::Result<()> {
    let handle = metadata_for_file(file)?;
    if handle.kind != DirectoryEntryKind::RegularFile
        || handle.link_count != 1
        || handle.mode & !0o777 != 0
    {
        return Err(unsafe_entry(
            "created staged regular file handle changed before cleanup",
        ));
    }
    let named = entry_metadata_at(directory, name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "created staged regular file disappeared before cleanup",
        )
    })?;
    require_exact_metadata(
        handle,
        named,
        "created staged regular file name changed before cleanup",
    )?;
    unlinkat(directory, name, AtFlags::empty()).map_err(io::Error::from)?;
    directory.sync_all()
}

fn next_stage_name() -> OsString {
    let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    OsString::from(format!(
        ".hypercolor-public-stage-{}-{sequence}",
        std::process::id()
    ))
}
