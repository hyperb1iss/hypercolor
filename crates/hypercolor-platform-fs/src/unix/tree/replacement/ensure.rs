use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use rustix::fs::{AtFlags, chmodat, mkdirat};
use rustix::io::Errno;

use super::super::traversal::{
    entry_metadata_at, entry_name, metadata_for_file, open_directory_at, require_same_entry,
    rustix_mode, set_exact_mode, unsafe_entry, validate_mode,
};
use super::super::{
    DirectoryEntryKind, DirectoryEntryMetadata, PERMISSION_BITS, PRIVATE_DIRECTORY_MODE,
    PublicDirectoryAuthority,
};

impl PublicDirectoryAuthority {
    pub(super) fn ensure_child_directory_with(
        &self,
        name: &Path,
        mode: u32,
        after_mkdir: impl FnOnce() -> io::Result<()>,
        after_mode: impl FnOnce() -> io::Result<()>,
        after_child_sync: impl FnOnce() -> io::Result<()>,
        sync: impl Fn(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        validate_directory_mode(mode)?;
        let name = entry_name(name, "public child directory name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        let prepared_ancestry = self.prepare_extended_ancestry(name)?;
        self.validate_ancestry_inner()?;
        let mut observed = entry_metadata_at(&self.directory, name)?;

        if observed.is_none() {
            match mkdirat(&self.directory, name, rustix_mode(PRIVATE_DIRECTORY_MODE)?) {
                Ok(()) => after_mkdir()?,
                Err(Errno::EXIST) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
            observed = entry_metadata_at(&self.directory, name)?;
        }

        let observed = observed.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "ensured public child directory disappeared before acquisition",
            )
        })?;
        require_replayable_directory_mode(observed, mode)?;
        if observed.mode != mode && observed.mode != PRIVATE_DIRECTORY_MODE {
            chmodat(
                &self.directory,
                name,
                rustix_mode(PRIVATE_DIRECTORY_MODE)?,
                AtFlags::empty(),
            )
            .map_err(io::Error::from)?;
        }

        let child = open_directory_at(&self.directory, name)?;
        let opened = require_named_handle(
            &self.directory,
            name,
            &child,
            "ensured public child directory changed during acquisition",
        )?;
        require_replayable_directory_mode(opened, mode)?;
        if opened.mode != mode {
            set_exact_mode(&child, mode)?;
        }
        after_mode()?;

        let expected = require_exact_directory(
            &self.directory,
            name,
            &child,
            mode,
            "ensured public child directory changed before durability",
        )?;
        sync(&child)?;
        after_child_sync()?;
        sync(&self.directory)?;
        self.validate_ancestry_inner()?;
        require_exact_directory(
            &self.directory,
            name,
            &child,
            mode,
            "ensured public child directory changed after durability",
        )?;

        Ok(Self {
            directory: child,
            ancestry: prepared_ancestry.finish(expected),
            shared: Arc::clone(&self.shared),
        })
    }
}

fn validate_directory_mode(mode: u32) -> io::Result<()> {
    validate_mode(mode)?;
    if mode & PRIVATE_DIRECTORY_MODE != PRIVATE_DIRECTORY_MODE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public directory mode must grant owner read, write, and search permission",
        ));
    }
    Ok(())
}

fn require_replayable_directory_mode(
    metadata: DirectoryEntryMetadata,
    requested_mode: u32,
) -> io::Result<()> {
    if metadata.kind != DirectoryEntryKind::Directory || metadata.mode & !PERMISSION_BITS != 0 {
        return Err(unsafe_entry(
            "ensured public child is not a directory with ordinary permission bits",
        ));
    }
    if metadata.mode != requested_mode && metadata.mode & !PRIVATE_DIRECTORY_MODE != 0 {
        return Err(unsafe_entry(
            "existing public child directory mode is not an exact or replayable state",
        ));
    }
    Ok(())
}

fn require_named_handle(
    parent: &File,
    name: &OsStr,
    child: &File,
    message: &'static str,
) -> io::Result<DirectoryEntryMetadata> {
    let opened = metadata_for_file(child)?;
    let named = entry_metadata_at(parent, name)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, message))?;
    require_same_entry(opened, named, message)?;
    Ok(opened)
}

fn require_exact_directory(
    parent: &File,
    name: &OsStr,
    child: &File,
    mode: u32,
    message: &'static str,
) -> io::Result<DirectoryEntryMetadata> {
    let opened = require_named_handle(parent, name, child, message)?;
    if opened.kind != DirectoryEntryKind::Directory || opened.mode != mode {
        return Err(unsafe_entry(message));
    }
    Ok(opened)
}
