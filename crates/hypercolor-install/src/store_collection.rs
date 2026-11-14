//! Removal of installed units nothing references, and of what interrupted
//! staging and removal left behind.
//!
//! Every release lands in its own immutable unit and stays there after a
//! newer one commits, so without collection a store grows by a release per
//! update. Collection runs under the installation lock and never removes a
//! referenced unit (see [`InstallStore::referenced_units`]), whatever the
//! caller asks. What the installer rendered for a unit outside it (see
//! [`DESKTOP_ENTRIES_DIRECTORY`]) follows the same rule, so it stays exactly
//! as long as its unit can be active again.

use std::io;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::PublicDirectoryAuthority;

use super::super::model::{InstallDisposition, UnitId};
use super::{
    DESKTOP_ENTRIES_DIRECTORY, INSTALL_JOURNAL_FILE, InstallLock, InstallStore, InstallStoreError,
    UNITS_DIRECTORY,
};

/// Directory name prefixes of interrupted unit staging (payload, legacy
/// snapshot and platform staging) and of interrupted removals. Entries with
/// these names are only ever created and removed under the installation
/// lock, so any found while holding it belong to a process that died.
const LEFTOVER_UNIT_PREFIXES: [&str; 2] = [".hypercolor-stage-", ".hypercolor-removing-"];

/// What one collection removed, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitCollection {
    /// Installed units removed because nothing references them.
    pub removed_units: Vec<UnitId>,
    /// Leftovers of interrupted staging, removal and journal writes, and
    /// what was rendered for units that are gone or removed now.
    pub removed_leftovers: Vec<PathBuf>,
    /// Entries that looked like the installer's but could not be removed
    /// (for example not a directory, not owned by this user, or holding a
    /// multiply linked file), with the reason. Collection goes on past
    /// them, so one odd entry never stops the rest.
    pub refused: Vec<(PathBuf, String)>,
}

impl InstallStore {
    /// Units this store keeps whatever a caller asks: the active unit and
    /// every unit the journal names unless it committed.
    ///
    /// A transaction that has not settled needs both of its sides. A
    /// rolled-back one needs them too: the next transaction proves its
    /// prior against the rolled-back record, which binds both units by
    /// file identity, so they stay until a later transaction commits.
    ///
    /// # Errors
    /// Returns an error for a foreign lock or unreadable records.
    pub fn referenced_units(&self, lock: &InstallLock) -> Result<Vec<UnitId>, InstallStoreError> {
        let mut referenced: Vec<UnitId> = self.active_unit(lock)?.into_iter().collect();
        if let Some(journal) = self.load_journal(lock)?
            && journal.disposition != InstallDisposition::Committed
        {
            for unit in journal.named_units() {
                if !referenced.contains(&unit) {
                    referenced.push(unit);
                }
            }
        }
        Ok(referenced)
    }

    /// Durably remove one installed unit that nothing references, and what
    /// the installer rendered for it.
    ///
    /// Returns `false` when the unit is not installed.
    ///
    /// # Errors
    /// Refuses a referenced unit (see [`Self::referenced_units`]) without
    /// any effect, and returns an error when removal fails. An interrupted
    /// removal leaves a hidden tombstone that a later removal or
    /// [`Self::collect_units`] finishes.
    pub fn remove_unit(
        &self,
        lock: &InstallLock,
        unit: &UnitId,
    ) -> Result<bool, InstallStoreError> {
        if self.referenced_units(lock)?.contains(unit) {
            return Err(InstallStoreError::UnitReferenced(unit.as_str().to_owned()));
        }
        let removed = match self.units_directory(lock)? {
            Some(units) => units
                .durable_remove_child_tree(Path::new(unit.as_str()))
                .map_err(|source| InstallStoreError::RemoveUnit {
                    name: unit.as_str().to_owned(),
                    source,
                })?,
            None => false,
        };
        // After the unit: an interruption between the two leaves only what
        // was rendered for a unit that is gone, which collection removes.
        if let Some(rendered) = self.desktop_entries_directory(lock)? {
            rendered
                .durable_remove_child_tree(Path::new(unit.as_str()))
                .map_err(|source| InstallStoreError::RemoveUnit {
                    name: format!("{DESKTOP_ENTRIES_DIRECTORY}/{}", unit.as_str()),
                    source,
                })?;
        }
        Ok(removed)
    }

    /// Remove every installed unit except the referenced ones and `retain`,
    /// what was rendered for every unit but those, then every leftover of
    /// interrupted staging, removal and journal writes.
    ///
    /// Entries whose names this installer never creates are left alone. An
    /// entry that refuses removal is reported in
    /// [`UnitCollection::refused`] and collection continues.
    ///
    /// # Errors
    /// Returns an error for a foreign lock, unreadable records, or a
    /// directory that cannot be listed. What was removed before it stays
    /// removed, and a later collection finishes the rest.
    pub fn collect_units(
        &self,
        lock: &InstallLock,
        retain: &[UnitId],
    ) -> Result<UnitCollection, InstallStoreError> {
        let referenced = self.referenced_units(lock)?;
        let keep = |unit: &UnitId| referenced.contains(unit) || retain.contains(unit);
        let mut collection = UnitCollection::default();
        if let Some(units) = self.units_directory(lock)? {
            let names = units
                .child_names()
                .map_err(InstallStoreError::InspectUnits)?;
            self.sweep_per_unit(&units, UNITS_DIRECTORY, names, keep, &mut collection, true);
        }
        // Rendered files of a unit go even when the unit itself refused:
        // nothing that names the unit is left to reach them.
        if let Some(rendered) = self.desktop_entries_directory(lock)? {
            let names = rendered
                .child_names()
                .map_err(InstallStoreError::InspectDesktopEntries)?;
            self.sweep_per_unit(
                &rendered,
                DESKTOP_ENTRIES_DIRECTORY,
                names,
                keep,
                &mut collection,
                false,
            );
        }
        let state = self.state_authority(lock)?;
        let journal_stage_prefix = format!(".{INSTALL_JOURNAL_FILE}.");
        let names = state
            .child_names()
            .map_err(InstallStoreError::InspectState)?;
        for name in names {
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(&journal_stage_prefix) {
                continue;
            }
            let path = self.state_root.join(name);
            match state.durable_remove_file(Path::new(name)) {
                Ok(false) => {}
                Ok(true) => collection.removed_leftovers.push(path),
                Err(error) => collection.refused.push((path, error.to_string())),
            }
        }
        Ok(collection)
    }

    /// Remove each of `names` in the release root's `directory` that names
    /// a unit `keep` refuses, or is a leftover of interrupted staging or
    /// removal. A removed unit is reported as one when `units` says these
    /// are the units themselves, and as a leftover path otherwise.
    fn sweep_per_unit(
        &self,
        parent: &PublicDirectoryAuthority,
        directory: &str,
        names: Vec<std::ffi::OsString>,
        keep: impl Fn(&UnitId) -> bool,
        collection: &mut UnitCollection,
        units: bool,
    ) {
        for name in names {
            let Some(name) = name.to_str() else {
                continue;
            };
            let path = self.root.join(directory).join(name);
            let unit = UnitId::new(name).ok();
            let leftover = LEFTOVER_UNIT_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix));
            if unit.as_ref().is_some_and(&keep) || unit.is_none() && !leftover {
                continue;
            }
            match parent.durable_remove_child_tree(Path::new(name)) {
                Ok(false) => {}
                Ok(true) => match unit.filter(|_| units) {
                    Some(unit) => collection.removed_units.push(unit),
                    None => collection.removed_leftovers.push(path),
                },
                Err(error) => collection.refused.push((path, error.to_string())),
            }
        }
    }

    /// The units directory through a removal-capable handle, or `None`
    /// when no unit was ever installed.
    ///
    /// It is opened beneath the store root the lock retained (proven to be
    /// the same inode as the canonical path), never by reresolving the
    /// units path, so a root replaced under the path cannot redirect a
    /// removal.
    fn units_directory(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<PublicDirectoryAuthority>, InstallStoreError> {
        self.release_child(lock, UNITS_DIRECTORY, InstallStoreError::OpenUnits)
    }

    /// The rendered desktop entries directory, opened as
    /// [`Self::units_directory`] opens `units`, or `None` when nothing was
    /// ever rendered.
    fn desktop_entries_directory(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<PublicDirectoryAuthority>, InstallStoreError> {
        self.release_child(
            lock,
            DESKTOP_ENTRIES_DIRECTORY,
            InstallStoreError::OpenDesktopEntries,
        )
    }

    fn release_child(
        &self,
        lock: &InstallLock,
        name: &str,
        open_failed: fn(io::Error) -> InstallStoreError,
    ) -> Result<Option<PublicDirectoryAuthority>, InstallStoreError> {
        self.authority(lock)?;
        let root = lock.open_store_public_directory()?;
        match root.open_child_directory(Path::new(name)) {
            Ok(child) => Ok(Some(child)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(open_failed(error)),
        }
    }
}
