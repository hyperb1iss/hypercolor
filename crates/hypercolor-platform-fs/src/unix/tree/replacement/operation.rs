use std::ffi::OsStr;
use std::fs::File;
use std::io;

use rustix::fs::{AtFlags, RenameFlags, renameat_with, unlinkat};

use super::super::publication::{PublicationRecoverySlot, remove_publication_recovery_slot};
use super::super::traversal::{entry_metadata_at, unsafe_entry};
use super::super::{EntryReplacement, ExactEntry, PublicDirectoryAuthority};
use super::exact::{
    RetainedExpectedEntry, combine_cleanup, remove_exact_entry, require_expected_at,
};
use super::rollback::{
    removal_before_visibility_failed, replacement_before_visibility_failed, rollback_removal,
    rollback_replacement, validate_placeholder_at,
};
use super::staging::stage_replacement;

pub(super) fn replace_entry_with(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    expected: &ExactEntry,
    replacement: EntryReplacement<'_>,
    before_visibility: impl FnOnce() -> io::Result<()>,
    after_visibility: impl FnOnce() -> io::Result<()>,
    sync: impl Fn(&File) -> io::Result<()>,
) -> io::Result<ExactEntry> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        authority.validate_ancestry_inner()?;
        require_expected_at(
            &authority.directory,
            destination,
            expected,
            "public replacement destination does not match its expected state",
        )?;
        let retained_expected =
            RetainedExpectedEntry::open(&authority.directory, destination, expected)?;
        let staged = stage_replacement(&authority.directory, replacement)?;
        authority.validate_ancestry_inner()?;
        let recovery = match PublicationRecoverySlot::reserve(&authority.directory) {
            Ok(recovery) => recovery,
            Err(error) => {
                let cleanup = remove_exact_entry(&authority.directory, &staged.name, &staged.exact);
                return combine_cleanup(error, cleanup, "staged replacement cleanup");
            }
        };
        if let Err(error) = sync(&authority.directory) {
            return replacement_before_visibility_failed(authority, staged, recovery, error);
        }
        if let Err(error) = before_visibility() {
            return replacement_before_visibility_failed(authority, staged, recovery, error);
        }
        let previsibility_proof = (|| {
            authority.validate_ancestry_inner()?;
            require_expected_at(
                &authority.directory,
                &staged.name,
                &staged.exact,
                "staged public replacement changed before visibility",
            )?;
            require_expected_at(
                &authority.directory,
                destination,
                expected,
                "public replacement destination changed before visibility",
            )?;
            retained_expected.validate_at(&authority.directory, destination)?;
            recovery.validate_name(
                &authority.directory,
                "public replacement recovery handle changed before visibility",
                "public replacement recovery name changed before visibility",
            )?;
            Ok(())
        })();
        if let Err(error) = previsibility_proof {
            return replacement_before_visibility_failed(authority, staged, recovery, error);
        }

        let visibility = if matches!(expected, ExactEntry::Absent) {
            renameat_with(
                &authority.directory,
                &staged.name,
                &authority.directory,
                destination,
                RenameFlags::NOREPLACE,
            )
        } else {
            renameat_with(
                &authority.directory,
                &staged.name,
                &authority.directory,
                destination,
                RenameFlags::EXCHANGE,
            )
        };
        if let Err(error) = visibility {
            return replacement_before_visibility_failed(
                authority,
                staged,
                recovery,
                io::Error::from(error),
            );
        }

        let proof = (|| {
            after_visibility()?;
            authority.validate_ancestry_inner()?;
            require_expected_at(
                &authority.directory,
                destination,
                &staged.exact,
                "published public replacement does not match staged contents",
            )?;
            if !matches!(expected, ExactEntry::Absent) {
                retained_expected.validate_at(&authority.directory, &staged.name)?;
                require_expected_at(
                    &authority.directory,
                    &staged.name,
                    expected,
                    "displaced public entry does not match its expected state",
                )?;
            }
            recovery.validate_name(
                &authority.directory,
                "public replacement recovery handle changed before durability",
                "public replacement recovery name changed before durability",
            )?;
            sync(&authority.directory)?;
            authority.validate_ancestry_inner()?;
            require_expected_at(
                &authority.directory,
                destination,
                &staged.exact,
                "published public replacement changed after durability",
            )?;
            Ok(())
        })();
        if let Err(error) = proof {
            return rollback_replacement(
                authority,
                destination,
                expected,
                &retained_expected,
                &staged,
                &recovery,
                error,
            );
        }

        if !matches!(expected, ExactEntry::Absent) {
            remove_exact_entry(&authority.directory, &staged.name, expected)?;
        }
        remove_publication_recovery_slot(&authority.directory, &recovery)?;
        authority.validate_ancestry_inner()?;
        Ok(staged.exact)
    }

    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (
            authority,
            destination,
            expected,
            replacement,
            before_visibility,
            after_visibility,
            sync,
        );
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exact public replacement is unsupported on this Unix platform",
        ))
    }
}

pub(super) fn remove_entry_with(
    authority: &PublicDirectoryAuthority,
    destination: &OsStr,
    expected: &ExactEntry,
    before_visibility: impl FnOnce() -> io::Result<()>,
    after_visibility: impl FnOnce() -> io::Result<()>,
    sync: impl Fn(&File) -> io::Result<()>,
) -> io::Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        authority.validate_ancestry_inner()?;
        require_expected_at(
            &authority.directory,
            destination,
            expected,
            "public removal destination does not match its expected state",
        )?;
        let retained_expected =
            RetainedExpectedEntry::open(&authority.directory, destination, expected)?;
        let recovery = PublicationRecoverySlot::reserve(&authority.directory)?;
        let quarantine = match PublicationRecoverySlot::reserve(&authority.directory) {
            Ok(quarantine) => quarantine,
            Err(error) => {
                return combine_cleanup(
                    error,
                    remove_publication_recovery_slot(&authority.directory, &recovery),
                    "public removal recovery cleanup",
                );
            }
        };
        if let Err(error) = sync(&authority.directory) {
            return removal_before_visibility_failed(authority, recovery, quarantine, error);
        }
        if let Err(error) = before_visibility() {
            return removal_before_visibility_failed(authority, recovery, quarantine, error);
        }
        let previsibility_proof = (|| {
            authority.validate_ancestry_inner()?;
            require_expected_at(
                &authority.directory,
                destination,
                expected,
                "public removal destination changed before visibility",
            )?;
            retained_expected.validate_at(&authority.directory, destination)?;
            recovery.validate_name(
                &authority.directory,
                "public removal recovery handle changed before exchange",
                "public removal recovery name changed before exchange",
            )?;
            quarantine.validate_name(
                &authority.directory,
                "public removal quarantine handle changed before exchange",
                "public removal quarantine name changed before exchange",
            )?;
            Ok(())
        })();
        if let Err(error) = previsibility_proof {
            return removal_before_visibility_failed(authority, recovery, quarantine, error);
        }
        if let Err(error) = renameat_with(
            &authority.directory,
            destination,
            &authority.directory,
            &recovery.name,
            RenameFlags::EXCHANGE,
        ) {
            return removal_before_visibility_failed(
                authority,
                recovery,
                quarantine,
                io::Error::from(error),
            );
        }
        let proof = (|| {
            after_visibility()?;
            authority.validate_ancestry_inner()?;
            retained_expected.validate_at(&authority.directory, &recovery.name)?;
            require_expected_at(
                &authority.directory,
                &recovery.name,
                expected,
                "quarantined public removal entry does not match expectation",
            )?;
            validate_placeholder_at(
                &authority.directory,
                destination,
                &recovery,
                "public removal placeholder changed before unlink",
            )?;
            unlinkat(&authority.directory, destination, AtFlags::empty())
                .map_err(io::Error::from)?;
            authority.validate_ancestry_inner()?;
            sync(&authority.directory)?;
            authority.validate_ancestry_inner()?;
            if entry_metadata_at(&authority.directory, destination)?.is_some() {
                return Err(unsafe_entry(
                    "public removal destination reappeared after durability",
                ));
            }
            Ok(())
        })();
        if let Err(error) = proof {
            return rollback_removal(
                authority,
                destination,
                expected,
                &recovery,
                &quarantine,
                error,
            );
        }
        remove_exact_entry(&authority.directory, &recovery.name, expected)?;
        remove_publication_recovery_slot(&authority.directory, &quarantine)?;
        authority.validate_ancestry_inner()
    }

    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (
            authority,
            destination,
            expected,
            before_visibility,
            after_visibility,
            sync,
        );
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exact public removal is unsupported on this Unix platform",
        ))
    }
}
