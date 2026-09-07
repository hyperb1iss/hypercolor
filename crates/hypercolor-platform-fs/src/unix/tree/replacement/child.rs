use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rustix::fs::{RenameFlags, mkdirat, renameat_with};
use rustix::io::Errno;

use super::super::traversal::{
    directory_is_empty, entry_metadata_at, entry_name, metadata_for_file, open_directory_at,
    rustix_mode, set_exact_mode, unsafe_entry, validate_mode,
};
use super::super::{
    DirectoryEntryKind, DirectoryEntryMetadata, PRIVATE_DIRECTORY_MODE, PublicDirectoryAuthority,
};
use super::exact::{combine_cleanup, remove_exact_empty_directory, require_exact_metadata};
use super::staging::{MAX_STAGE_ATTEMPTS, STAGE_SEQUENCE};

impl PublicDirectoryAuthority {
    pub(super) fn create_child_directory_with(
        &self,
        name: &Path,
        mode: u32,
        before_visibility: impl FnOnce() -> io::Result<()>,
        after_visibility: impl FnOnce() -> io::Result<()>,
        sync: impl Fn(&File) -> io::Result<()>,
    ) -> io::Result<Self> {
        validate_mode(mode)?;
        let name = entry_name(name, "public child directory name")?;
        let _operation = self.operation_guard()?;
        self.validate_ancestry_inner()?;
        if entry_metadata_at(&self.directory, name)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "public child directory destination already exists",
            ));
        }
        let prepared_ancestry = self.prepare_extended_ancestry(name)?;
        self.validate_ancestry_inner()?;
        let install_device = metadata_for_file(&self.shared.directory)?.device;
        let public_device = metadata_for_file(&self.directory)?.device;
        require_same_filesystem(install_device, public_device)?;
        let (staged_name, child, expected) = stage_child_directory(&self.shared.directory, mode)?;
        if let Err(error) = sync(&self.shared.directory).and_then(|()| before_visibility()) {
            let cleanup =
                remove_exact_empty_directory(&self.shared.directory, &staged_name, &child);
            return combine_cleanup(error, cleanup, "staged public directory cleanup");
        }
        let previsibility = (|| {
            self.validate_ancestry_inner()?;
            require_exact_empty_directory(
                &self.shared.directory,
                &staged_name,
                &child,
                mode,
                "staged public child directory changed before visibility",
            )?;
            if entry_metadata_at(&self.directory, name)?.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "public child directory destination appeared before visibility",
                ));
            }
            Ok(())
        })();
        if let Err(error) = previsibility {
            let cleanup =
                remove_exact_empty_directory(&self.shared.directory, &staged_name, &child);
            return combine_cleanup(error, cleanup, "staged public directory cleanup");
        }
        if let Err(error) = renameat_with(
            &self.shared.directory,
            &staged_name,
            &self.directory,
            name,
            RenameFlags::NOREPLACE,
        ) {
            let cleanup =
                remove_exact_empty_directory(&self.shared.directory, &staged_name, &child);
            return combine_cleanup(
                io::Error::from(error),
                cleanup,
                "staged public directory cleanup",
            );
        }
        let proof = (|| {
            after_visibility()?;
            self.validate_ancestry_inner()?;
            require_exact_empty_directory(
                &self.directory,
                name,
                &child,
                mode,
                "published public child directory changed before durability",
            )?;
            if entry_metadata_at(&self.shared.directory, &staged_name)?.is_some() {
                return Err(unsafe_entry(
                    "staged public child directory name remained after publication",
                ));
            }
            sync(&self.shared.directory)?;
            sync(&self.directory)?;
            self.validate_ancestry_inner()?;
            require_exact_empty_directory(
                &self.directory,
                name,
                &child,
                mode,
                "published public child directory changed after durability",
            )
        })();
        if let Err(error) = proof {
            return rollback_child_directory(self, name, &staged_name, &child, mode, error);
        }
        Ok(Self {
            directory: child,
            ancestry: prepared_ancestry.finish(expected),
            shared: Arc::clone(&self.shared),
        })
    }
}

fn stage_child_directory(
    parent: &File,
    mode: u32,
) -> io::Result<(OsString, File, DirectoryEntryMetadata)> {
    for _ in 0..MAX_STAGE_ATTEMPTS {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = OsString::from(format!(
            ".hypercolor-public-directory-stage-{}-{sequence}",
            std::process::id()
        ));
        match mkdirat(parent, &name, rustix_mode(PRIVATE_DIRECTORY_MODE)?) {
            Ok(()) => {
                let child = open_directory_at(parent, &name)?;
                let result = set_exact_mode(&child, mode)
                    .and_then(|()| child.sync_all())
                    .and_then(|()| {
                        require_exact_empty_directory(
                            parent,
                            &name,
                            &child,
                            mode,
                            "staged public child directory changed during construction",
                        )
                    })
                    .and_then(|()| metadata_for_file(&child));
                return match result {
                    Ok(metadata) => Ok((name, child, metadata)),
                    Err(error) => combine_cleanup(
                        error,
                        remove_exact_empty_directory(parent, &name, &child),
                        "staged public directory cleanup",
                    ),
                };
            }
            Err(Errno::EXIST) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a staged public child directory",
    ))
}

pub(super) fn require_same_filesystem(
    source_device: u64,
    destination_device: u64,
) -> io::Result<()> {
    if source_device != destination_device {
        return Err(io::Error::from(Errno::XDEV));
    }
    Ok(())
}

fn require_exact_empty_directory(
    parent: &File,
    name: &OsStr,
    child: &File,
    mode: u32,
    message: &'static str,
) -> io::Result<()> {
    let handle = metadata_for_file(child)?;
    let named = entry_metadata_at(parent, name)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, message))?;
    require_exact_metadata(handle, named, message)?;
    if handle.kind != DirectoryEntryKind::Directory
        || handle.mode != mode
        || !directory_is_empty(child)?
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

fn rollback_child_directory<T>(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    staged_name: &OsStr,
    child: &File,
    mode: u32,
    proof_error: io::Error,
) -> io::Result<T> {
    if let Err(changed_error) = require_exact_empty_directory(
        &authority.directory,
        destination,
        child,
        mode,
        "published public child changed before rollback",
    ) {
        return Err(io::Error::other(format!(
            "{proof_error}; published public child remains untouched: {changed_error}"
        )));
    }
    if let Err(rollback_error) = renameat_with(
        &authority.directory,
        destination,
        &authority.shared.directory,
        staged_name,
        RenameFlags::NOREPLACE,
    ) {
        return Err(io::Error::other(format!(
            "{proof_error}; public child rollback failed: {rollback_error}"
        )));
    }
    authority.directory.sync_all()?;
    combine_cleanup(
        proof_error,
        remove_exact_empty_directory(&authority.shared.directory, staged_name, child),
        "rolled-back public child cleanup",
    )
}
