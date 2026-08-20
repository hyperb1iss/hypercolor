use std::ffi::OsStr;
use std::fs::File;
use std::io;

use rustix::fs::{AtFlags, RenameFlags, renameat_with, unlinkat};

use super::super::publication::{PublicationRecoverySlot, remove_publication_recovery_slot};
use super::super::traversal::{entry_metadata_at, unsafe_entry};
use super::super::{
    DirectoryEntryKind, DirectoryEntryMetadata, ExactEntry, PublicDirectoryAuthority,
    SECRET_FILE_MODE,
};
use super::exact::{
    RetainedExpectedEntry, combine_two_cleanups, exact_entry_matches_at, remove_exact_entry,
    require_exact_metadata, require_expected_at,
};
use super::staging::StagedEntry;

pub(super) fn replacement_before_visibility_failed<T>(
    authority: &PublicDirectoryAuthority,
    staged: StagedEntry,
    recovery: PublicationRecoverySlot,
    error: io::Error,
) -> io::Result<T> {
    let staged_cleanup = remove_exact_entry(&authority.directory, &staged.name, &staged.exact);
    let recovery_cleanup = remove_publication_recovery_slot(&authority.directory, &recovery);
    combine_two_cleanups(
        error,
        staged_cleanup,
        "staged replacement cleanup",
        recovery_cleanup,
        "replacement recovery cleanup",
    )
}

pub(super) fn removal_before_visibility_failed<T>(
    authority: &PublicDirectoryAuthority,
    recovery: PublicationRecoverySlot,
    quarantine: PublicationRecoverySlot,
    error: io::Error,
) -> io::Result<T> {
    let recovery_cleanup = remove_publication_recovery_slot(&authority.directory, &recovery);
    let quarantine_cleanup = remove_publication_recovery_slot(&authority.directory, &quarantine);
    combine_two_cleanups(
        error,
        recovery_cleanup,
        "removal recovery cleanup",
        quarantine_cleanup,
        "removal quarantine cleanup",
    )
}

pub(super) fn rollback_replacement<T>(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    expected: &ExactEntry,
    retained_expected: &RetainedExpectedEntry,
    staged: &StagedEntry,
    recovery: &PublicationRecoverySlot,
    proof_error: io::Error,
) -> io::Result<T> {
    let destination_metadata = entry_metadata_at(&authority.directory, destination)?;
    let destination_is_staged =
        exact_entry_matches_at(&authority.directory, destination, &staged.exact)?;
    if matches!(expected, ExactEntry::Absent) {
        if destination_is_staged {
            renameat_with(
                &authority.directory,
                destination,
                &authority.directory,
                &staged.name,
                RenameFlags::NOREPLACE,
            )
            .map_err(io::Error::from)?;
            authority.directory.sync_all()?;
            remove_exact_entry(&authority.directory, &staged.name, &staged.exact)?;
            remove_publication_recovery_slot(&authority.directory, recovery)?;
            return Err(proof_error);
        }
        let Some(destination_metadata) = destination_metadata else {
            remove_publication_recovery_slot(&authority.directory, recovery)?;
            return Err(proof_error);
        };
        quarantine_destination(
            &authority.directory,
            destination,
            recovery,
            destination_metadata,
        )?;
        unlinkat(&authority.directory, destination, AtFlags::empty()).map_err(io::Error::from)?;
        authority.directory.sync_all()?;
        return Err(io::Error::other(format!(
            "{proof_error}; unverified replacement destination quarantined as {}",
            recovery.name.to_string_lossy()
        )));
    }

    if destination_is_staged {
        let displaced =
            entry_metadata_at(&authority.directory, &staged.name)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "displaced public entry disappeared before replacement rollback",
                )
            })?;
        let displaced_is_expected = retained_expected
            .validate_at(&authority.directory, &staged.name)
            .and_then(|()| {
                require_expected_at(
                    &authority.directory,
                    &staged.name,
                    expected,
                    "displaced public entry changed before replacement rollback",
                )
            })
            .is_ok();
        if !displaced_is_expected {
            quarantine_destination(&authority.directory, &staged.name, recovery, displaced)?;
            unlinkat(&authority.directory, &staged.name, AtFlags::empty())
                .map_err(io::Error::from)?;
            authority.directory.sync_all()?;
            return Err(io::Error::other(format!(
                "{proof_error}; unverified displaced entry quarantined as {}; exact replacement remains published",
                recovery.name.to_string_lossy()
            )));
        }
        renameat_with(
            &authority.directory,
            destination,
            &authority.directory,
            &staged.name,
            RenameFlags::EXCHANGE,
        )
        .map_err(io::Error::from)?;
        let restored = entry_metadata_at(&authority.directory, destination)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "displaced public entry disappeared during replacement rollback",
            )
        })?;
        require_exact_metadata(
            displaced,
            restored,
            "displaced public entry changed during replacement rollback",
        )?;
        require_expected_at(
            &authority.directory,
            &staged.name,
            &staged.exact,
            "staged replacement changed during rollback",
        )?;
        authority.directory.sync_all()?;
        remove_exact_entry(&authority.directory, &staged.name, &staged.exact)?;
        remove_publication_recovery_slot(&authority.directory, recovery)?;
        return Err(proof_error);
    }

    require_expected_at(
        &authority.directory,
        &staged.name,
        expected,
        "public replacement rollback source changed",
    )?;

    recovery.validate_name(
        &authority.directory,
        "public replacement recovery handle changed before quarantine",
        "public replacement recovery name changed before quarantine",
    )?;
    let Some(destination_metadata) = destination_metadata else {
        renameat_with(
            &authority.directory,
            &staged.name,
            &authority.directory,
            destination,
            RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)?;
        remove_publication_recovery_slot(&authority.directory, recovery)?;
        authority.directory.sync_all()?;
        return Err(proof_error);
    };
    quarantine_destination(
        &authority.directory,
        destination,
        recovery,
        destination_metadata,
    )?;
    renameat_with(
        &authority.directory,
        &staged.name,
        &authority.directory,
        destination,
        RenameFlags::EXCHANGE,
    )
    .map_err(io::Error::from)?;
    validate_placeholder_at(
        &authority.directory,
        &staged.name,
        recovery,
        "public replacement rollback placeholder changed",
    )?;
    unlinkat(&authority.directory, &staged.name, AtFlags::empty()).map_err(io::Error::from)?;
    authority.directory.sync_all()?;
    Err(io::Error::other(format!(
        "{proof_error}; unverified replacement destination quarantined as {}",
        recovery.name.to_string_lossy()
    )))
}

fn quarantine_destination(
    directory: &File,
    destination: &OsStr,
    quarantine: &PublicationRecoverySlot,
    expected: DirectoryEntryMetadata,
) -> io::Result<()> {
    quarantine.validate_name(
        directory,
        "public quarantine handle changed before exchange",
        "public quarantine name changed before exchange",
    )?;
    renameat_with(
        directory,
        destination,
        directory,
        &quarantine.name,
        RenameFlags::EXCHANGE,
    )
    .map_err(io::Error::from)?;
    let quarantined = entry_metadata_at(directory, &quarantine.name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "public quarantine entry disappeared during exchange",
        )
    })?;
    require_exact_metadata(
        expected,
        quarantined,
        "public quarantine entry changed during exchange",
    )?;
    validate_placeholder_at(
        directory,
        destination,
        quarantine,
        "public quarantine placeholder changed during exchange",
    )
}

pub(super) fn rollback_removal<T>(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    expected: &ExactEntry,
    recovery: &PublicationRecoverySlot,
    quarantine: &PublicationRecoverySlot,
    proof_error: io::Error,
) -> io::Result<T> {
    require_expected_at(
        &authority.directory,
        &recovery.name,
        expected,
        "public removal rollback source changed",
    )?;
    let destination_present = entry_metadata_at(&authority.directory, destination)?.is_some();
    if validate_placeholder_at(
        &authority.directory,
        destination,
        recovery,
        "public removal rollback placeholder changed",
    )
    .is_ok()
    {
        renameat_with(
            &authority.directory,
            &recovery.name,
            &authority.directory,
            destination,
            RenameFlags::EXCHANGE,
        )
        .map_err(io::Error::from)?;
        authority.directory.sync_all()?;
        remove_publication_recovery_slot(&authority.directory, recovery)?;
        remove_publication_recovery_slot(&authority.directory, quarantine)?;
        return Err(proof_error);
    }
    if !destination_present {
        renameat_with(
            &authority.directory,
            &recovery.name,
            &authority.directory,
            destination,
            RenameFlags::NOREPLACE,
        )
        .map_err(io::Error::from)?;
        remove_publication_recovery_slot(&authority.directory, quarantine)?;
        authority.directory.sync_all()?;
        Err(proof_error)
    } else {
        quarantine.validate_name(
            &authority.directory,
            "public removal quarantine handle changed before exchange",
            "public removal quarantine name changed before exchange",
        )?;
        renameat_with(
            &authority.directory,
            destination,
            &authority.directory,
            &quarantine.name,
            RenameFlags::EXCHANGE,
        )
        .map_err(io::Error::from)?;
        validate_placeholder_at(
            &authority.directory,
            destination,
            quarantine,
            "public removal quarantine placeholder changed",
        )?;
        renameat_with(
            &authority.directory,
            &recovery.name,
            &authority.directory,
            destination,
            RenameFlags::EXCHANGE,
        )
        .map_err(io::Error::from)?;
        validate_placeholder_at(
            &authority.directory,
            &recovery.name,
            quarantine,
            "public removal rollback placeholder changed",
        )?;
        unlinkat(&authority.directory, &recovery.name, AtFlags::empty())
            .map_err(io::Error::from)?;
        authority.directory.sync_all()?;
        Err(io::Error::other(format!(
            "{proof_error}; unverified removal destination quarantined as {}",
            quarantine.name.to_string_lossy()
        )))
    }
}

pub(super) fn validate_placeholder_at(
    directory: &File,
    name: &OsStr,
    slot: &PublicationRecoverySlot,
    message: &'static str,
) -> io::Result<()> {
    let expected = slot.validate_handle(message)?;
    let current = entry_metadata_at(directory, name)?
        .ok_or_else(|| unsafe_entry("public recovery placeholder disappeared"))?;
    if current.kind != DirectoryEntryKind::RegularFile
        || current.device != expected.device
        || current.inode != expected.inode
        || current.link_count != 1
        || current.mode != SECRET_FILE_MODE
        || current.size != 0
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}
