use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{AtFlags, OFlags, RenameFlags, openat, renameat_with, unlinkat};
use rustix::io::Errno;

use super::traversal::{
    entry_metadata_at, metadata_for_file, open_directory_at, require_same_entry, rustix_mode,
    set_exact_mode, unsafe_entry,
};
use super::{DirectoryEntryKind, DirectoryEntryMetadata, SECRET_FILE_MODE};

pub(super) const RECOVERY_NAME_PREFIX: &str = ".hypercolor-recovery-";
static RECOVERY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct PublicationRecoverySlot {
    pub(super) name: OsString,
    pub(super) placeholder: File,
    pub(super) baseline: DirectoryEntryMetadata,
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[derive(Debug)]
struct ArmedRecoveryReservation<'a> {
    parent: &'a File,
    name: &'a OsStr,
    placeholder: &'a File,
    proof: ReservationProof,
    armed: bool,
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[derive(Debug, Clone, Copy)]
enum ReservationProof {
    Unproven,
    Created(DirectoryEntryMetadata),
    Chmodded(DirectoryEntryMetadata),
    Final(DirectoryEntryMetadata),
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl ArmedRecoveryReservation<'_> {
    fn prove_created(&mut self, expected: DirectoryEntryMetadata) {
        self.proof = ReservationProof::Created(expected);
    }

    fn mark_chmodded(&mut self) {
        let ReservationProof::Created(expected) = self.proof else {
            unreachable!("recovery reservation is create-time proven before chmod")
        };
        self.proof = ReservationProof::Chmodded(expected);
    }

    fn prove_final(&mut self, expected: DirectoryEntryMetadata) {
        self.proof = ReservationProof::Final(expected);
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn cleanup(&mut self) -> io::Result<bool> {
        if !self.armed {
            return Ok(false);
        }
        let expected = match self.proof {
            ReservationProof::Unproven => metadata_for_file(self.placeholder)?,
            ReservationProof::Created(expected)
            | ReservationProof::Chmodded(expected)
            | ReservationProof::Final(expected) => expected,
        };
        let handle_metadata = metadata_for_file(self.placeholder)?;
        self.validate_cleanup_metadata(expected, handle_metadata, "handle")?;
        let Some(current) = entry_metadata_at(self.parent, self.name)? else {
            self.armed = false;
            return Ok(false);
        };
        self.validate_cleanup_metadata(expected, current, "name")?;
        unlinkat(self.parent, self.name, AtFlags::empty()).map_err(io::Error::from)?;
        self.parent.sync_all()?;
        self.armed = false;
        Ok(true)
    }

    fn validate_cleanup_metadata(
        &self,
        expected: DirectoryEntryMetadata,
        current: DirectoryEntryMetadata,
        authority: &'static str,
    ) -> io::Result<()> {
        let message = match authority {
            "handle" => "publication recovery reservation handle metadata drifted",
            _ => "publication recovery reservation name metadata drifted",
        };
        match self.proof {
            ReservationProof::Unproven | ReservationProof::Created(_) => {
                validate_created_placeholder(expected, current, message)
            }
            ReservationProof::Chmodded(_) => {
                validate_chmodded_placeholder(expected, current, message)
            }
            ReservationProof::Final(_) => validate_recovery_placeholder(expected, current, message),
        }
    }

    fn fail(mut self, reservation_error: io::Error) -> io::Error {
        match self.cleanup() {
            Ok(_) => reservation_error,
            Err(cleanup_error) => io::Error::other(format!(
                "{reservation_error}; publication recovery reservation cleanup failed: {cleanup_error}"
            )),
        }
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl Drop for ArmedRecoveryReservation<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl PublicationRecoverySlot {
    pub(super) fn reserve(parent: &File) -> io::Result<Self> {
        Self::reserve_with(parent, || Ok(()), || Ok(()))
    }

    pub(super) fn reserve_with(
        parent: &File,
        before_proof: impl FnOnce() -> io::Result<()>,
        after_proof: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::reserve_with_steps(parent, before_proof, || Ok(()), after_proof)
    }

    pub(super) fn reserve_with_steps(
        parent: &File,
        before_proof: impl FnOnce() -> io::Result<()>,
        before_chmod: impl FnOnce() -> io::Result<()>,
        after_proof: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        for _ in 0..128 {
            let sequence = RECOVERY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = OsString::from(format!(
                "{RECOVERY_NAME_PREFIX}{}-{sequence}",
                std::process::id()
            ));
            match openat(
                parent,
                &name,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                rustix_mode(SECRET_FILE_MODE)?,
            ) {
                Ok(placeholder) => {
                    let placeholder = File::from(placeholder);
                    let mut reservation = ArmedRecoveryReservation {
                        parent,
                        name: &name,
                        placeholder: &placeholder,
                        proof: ReservationProof::Unproven,
                        armed: true,
                    };
                    let result = (|| {
                        before_proof()?;
                        let expected = metadata_for_file(&placeholder)?;
                        let current = entry_metadata_at(parent, &name)?.ok_or_else(|| {
                            unsafe_entry("publication recovery slot disappeared during reservation")
                        })?;
                        validate_created_placeholder(
                            expected,
                            current,
                            "publication recovery slot metadata is unsafe during reservation",
                        )?;
                        reservation.prove_created(expected);
                        before_chmod()?;
                        set_exact_mode(&placeholder, SECRET_FILE_MODE)?;
                        reservation.mark_chmodded();
                        let final_metadata = metadata_for_file(&placeholder)?;
                        validate_chmodded_placeholder(
                            expected,
                            final_metadata,
                            "publication recovery slot handle metadata drifted during reservation",
                        )?;
                        let final_name = entry_metadata_at(parent, &name)?.ok_or_else(|| {
                            unsafe_entry("publication recovery slot disappeared after finalization")
                        })?;
                        validate_recovery_placeholder(
                            final_metadata,
                            final_name,
                            "publication recovery slot name metadata drifted after finalization",
                        )?;
                        reservation.prove_final(final_metadata);
                        after_proof()?;
                        Ok(())
                    })();
                    return match result {
                        Ok(()) => {
                            let ReservationProof::Final(baseline) = reservation.proof else {
                                unreachable!("successful reservation has final baseline metadata")
                            };
                            reservation.disarm();
                            drop(reservation);
                            Ok(Self {
                                name,
                                placeholder,
                                baseline,
                            })
                        }
                        Err(error) => Err(reservation.fail(error)),
                    };
                }
                Err(Errno::EXIST) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a private publication recovery slot",
        ))
    }

    pub(super) fn validate_handle(
        &self,
        message: &'static str,
    ) -> io::Result<DirectoryEntryMetadata> {
        let current = metadata_for_file(&self.placeholder)?;
        validate_recovery_placeholder(self.baseline, current, message)?;
        Ok(current)
    }

    pub(super) fn validate_name(
        &self,
        parent: &File,
        handle_message: &'static str,
        name_message: &'static str,
    ) -> io::Result<DirectoryEntryMetadata> {
        let expected = self.validate_handle(handle_message)?;
        let current = entry_metadata_at(parent, &self.name)?
            .ok_or_else(|| unsafe_entry("publication recovery slot disappeared"))?;
        validate_recovery_placeholder(expected, current, name_message)?;
        Ok(expected)
    }
}

fn validate_recovery_placeholder(
    baseline: DirectoryEntryMetadata,
    current: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if baseline.kind != DirectoryEntryKind::RegularFile
        || baseline.link_count != 1
        || baseline.mode != SECRET_FILE_MODE
        || baseline.size != 0
        || current.kind != DirectoryEntryKind::RegularFile
        || current.device != baseline.device
        || current.inode != baseline.inode
        || current.link_count != 1
        || current.mode != SECRET_FILE_MODE
        || current.size != 0
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

fn validate_created_placeholder(
    created: DirectoryEntryMetadata,
    current: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if created.kind != DirectoryEntryKind::RegularFile
        || created.link_count != 1
        || created.mode & !SECRET_FILE_MODE != 0
        || created.size != 0
        || current.kind != DirectoryEntryKind::RegularFile
        || current.device != created.device
        || current.inode != created.inode
        || current.link_count != 1
        || current.mode != created.mode
        || current.size != 0
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

fn validate_chmodded_placeholder(
    created: DirectoryEntryMetadata,
    current: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if created.kind != DirectoryEntryKind::RegularFile
        || created.link_count != 1
        || created.mode & !SECRET_FILE_MODE != 0
        || created.size != 0
        || current.kind != DirectoryEntryKind::RegularFile
        || current.device != created.device
        || current.inode != created.inode
        || current.link_count != 1
        || current.mode != SECRET_FILE_MODE
        || current.size != 0
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

pub(super) fn durable_publish_directory_with(
    parent: &File,
    staged: &OsStr,
    destination: &OsStr,
    expected: DirectoryEntryMetadata,
    before_rename: impl FnOnce() -> io::Result<()>,
    after_rename: impl FnOnce() -> io::Result<()>,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<File> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        let current = entry_metadata_at(parent, staged)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "private staging directory is missing",
            )
        })?;
        require_same_entry(expected, current, "private staging source changed")?;
        let recovery_slot = PublicationRecoverySlot::reserve(parent)?;
        if let Err(error) = before_rename() {
            return publication_before_visibility_failed(parent, &recovery_slot, error);
        }
        if let Err(error) = recovery_slot.validate_name(
            parent,
            "publication recovery slot handle metadata drifted before publication",
            "publication recovery slot name metadata drifted before publication",
        ) {
            return publication_before_visibility_failed(parent, &recovery_slot, error);
        }
        if let Err(error) =
            renameat_with(parent, staged, parent, destination, RenameFlags::NOREPLACE)
        {
            return publication_before_visibility_failed(
                parent,
                &recovery_slot,
                io::Error::from(error),
            );
        }
        if let Err(error) = after_rename() {
            return publication_proof_failed(parent, staged, destination, &recovery_slot, error);
        }
        let published = match open_directory_at(parent, destination) {
            Ok(published) => published,
            Err(error) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    error,
                );
            }
        };
        let published_metadata = match metadata_for_file(&published) {
            Ok(metadata) => metadata,
            Err(error) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    error,
                );
            }
        };
        if let Err(error) = require_same_entry(
            expected,
            published_metadata,
            "published directory identity does not match staging",
        ) {
            return publication_proof_failed(parent, staged, destination, &recovery_slot, error);
        }
        match entry_metadata_at(parent, staged) {
            Ok(None) => {}
            Ok(Some(_)) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    unsafe_entry("private staging name remains after publication"),
                );
            }
            Err(error) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    error,
                );
            }
        }
        if let Err(error) = sync(parent) {
            return publication_proof_failed(parent, staged, destination, &recovery_slot, error);
        }
        let final_metadata = match entry_metadata_at(parent, destination) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "published directory disappeared after durability barrier",
                    ),
                );
            }
            Err(error) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    error,
                );
            }
        };
        if let Err(error) = require_same_entry(
            expected,
            final_metadata,
            "published directory identity changed after durability barrier",
        ) {
            return publication_proof_failed(parent, staged, destination, &recovery_slot, error);
        }
        let staging_returned = match entry_metadata_at(parent, staged) {
            Ok(metadata) => metadata.is_some(),
            Err(error) => {
                return publication_proof_failed(
                    parent,
                    staged,
                    destination,
                    &recovery_slot,
                    error,
                );
            }
        };
        if staging_returned {
            return publication_proof_failed(
                parent,
                staged,
                destination,
                &recovery_slot,
                unsafe_entry("private staging name returned after durability barrier"),
            );
        }
        remove_publication_recovery_slot(parent, &recovery_slot)?;
        Ok(published)
    }

    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (
            parent,
            staged,
            destination,
            expected,
            before_rename,
            after_rename,
            sync,
        );
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace directory publication is unsupported",
        ))
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn publication_proof_failed<T>(
    parent: &File,
    staged: &OsStr,
    destination: &OsStr,
    recovery_slot: &PublicationRecoverySlot,
    proof_error: io::Error,
) -> io::Result<T> {
    match renameat_with(parent, destination, parent, staged, RenameFlags::NOREPLACE) {
        Ok(()) => {
            if let Err(recovery_error) = remove_publication_recovery_slot(parent, recovery_slot) {
                return Err(io::Error::other(format!(
                    "{proof_error}; publication rollback cleanup failed: {recovery_error}"
                )));
            }
            Err(proof_error)
        }
        Err(rollback_error) => {
            match quarantine_publication_destination(parent, destination, recovery_slot) {
                Ok(true) => Err(io::Error::other(format!(
                    "{proof_error}; direct publication rollback failed: {rollback_error}; unverified destination quarantined as {}",
                    recovery_slot.name.to_string_lossy()
                ))),
                Ok(false) => Err(io::Error::other(format!(
                    "{proof_error}; direct publication rollback failed: {rollback_error}; destination was already absent"
                ))),
                Err(recovery_error) => Err(io::Error::other(format!(
                    "{proof_error}; direct publication rollback failed: {rollback_error}; destination quarantine failed: {recovery_error}"
                ))),
            }
        }
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn publication_before_visibility_failed<T>(
    parent: &File,
    recovery_slot: &PublicationRecoverySlot,
    publication_error: io::Error,
) -> io::Result<T> {
    match remove_publication_recovery_slot(parent, recovery_slot) {
        Ok(()) => Err(publication_error),
        Err(cleanup_error) => Err(io::Error::other(format!(
            "{publication_error}; publication recovery slot cleanup failed: {cleanup_error}"
        ))),
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn remove_publication_recovery_slot(
    parent: &File,
    recovery_slot: &PublicationRecoverySlot,
) -> io::Result<()> {
    recovery_slot.validate_name(
        parent,
        "publication recovery slot handle metadata drifted before removal",
        "publication recovery slot name metadata drifted before removal",
    )?;
    unlinkat(parent, &recovery_slot.name, AtFlags::empty()).map_err(io::Error::from)?;
    parent.sync_all()
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn quarantine_publication_destination(
    parent: &File,
    destination: &OsStr,
    recovery_slot: &PublicationRecoverySlot,
) -> io::Result<bool> {
    let Some(observed) = entry_metadata_at(parent, destination)? else {
        remove_publication_recovery_slot(parent, recovery_slot)?;
        return Ok(false);
    };
    let reserved = recovery_slot.validate_name(
        parent,
        "publication recovery slot handle metadata drifted before exchange",
        "publication recovery slot name metadata drifted before exchange",
    )?;
    renameat_with(
        parent,
        destination,
        parent,
        &recovery_slot.name,
        RenameFlags::EXCHANGE,
    )
    .map_err(io::Error::from)?;

    let quarantine_proof = match entry_metadata_at(parent, &recovery_slot.name) {
        Ok(Some(quarantined)) => require_same_entry(
            observed,
            quarantined,
            "publication quarantine identity changed",
        ),
        Ok(None) => Err(unsafe_entry("publication quarantine disappeared")),
        Err(error) => Err(error),
    };
    let placeholder_proof = match entry_metadata_at(parent, destination) {
        Ok(Some(placeholder)) => validate_recovery_placeholder(
            reserved,
            placeholder,
            "publication recovery placeholder metadata drifted after exchange",
        ),
        Ok(None) => Err(unsafe_entry("publication recovery placeholder disappeared")),
        Err(error) => Err(error),
    };
    let removal = if placeholder_proof.is_ok() {
        unlinkat(parent, destination, AtFlags::empty()).map_err(io::Error::from)
    } else {
        Ok(())
    };
    let sync_result = parent.sync_all();
    quarantine_proof?;
    placeholder_proof?;
    removal?;
    sync_result?;
    Ok(true)
}
