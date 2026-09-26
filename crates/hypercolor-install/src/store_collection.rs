//! Removal of installed units nothing references, and of what interrupted
//! staging and removal left behind.
//!
//! Every release lands in its own immutable unit and stays there after a
//! newer one commits, so without collection a store grows by a release per
//! update. Collection runs under the installation lock and never removes a
//! referenced unit (see [`InstallStore::referenced_units`]), whatever the
//! caller asks.

use std::io;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::PublicDirectoryAuthority;

use super::super::model::{InstallDisposition, InstallJournalV1, UnitId};
use super::{INSTALL_JOURNAL_FILE, InstallLock, InstallStore, InstallStoreError, UNITS_DIRECTORY};

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
    /// Leftovers of interrupted staging, removal and journal writes.
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
            for unit in journal_units(&journal) {
                if !referenced.contains(&unit) {
                    referenced.push(unit);
                }
            }
        }
        Ok(referenced)
    }

    /// Durably remove one installed unit that nothing references.
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
        let Some(units) = self.units_directory(lock)? else {
            return Ok(false);
        };
        units
            .durable_remove_child_tree(Path::new(unit.as_str()))
            .map_err(|source| InstallStoreError::RemoveUnit {
                name: unit.as_str().to_owned(),
                source,
            })
    }

    /// Remove every installed unit except the referenced ones and `retain`,
    /// then every leftover of interrupted staging, removal and journal
    /// writes.
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
        let mut collection = UnitCollection::default();
        if let Some(units) = self.units_directory(lock)? {
            let names = units
                .child_names()
                .map_err(InstallStoreError::InspectUnits)?;
            for name in names {
                let Some(name) = name.to_str() else {
                    continue;
                };
                let path = self.root.join(UNITS_DIRECTORY).join(name);
                let unit = UnitId::new(name).ok();
                let leftover = LEFTOVER_UNIT_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix));
                if unit
                    .as_ref()
                    .is_some_and(|unit| referenced.contains(unit) || retain.contains(unit))
                    || unit.is_none() && !leftover
                {
                    continue;
                }
                match units.durable_remove_child_tree(Path::new(name)) {
                    Ok(false) => {}
                    Ok(true) => match unit {
                        Some(unit) => collection.removed_units.push(unit),
                        None => collection.removed_leftovers.push(path),
                    },
                    Err(error) => collection.refused.push((path, error.to_string())),
                }
            }
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

    /// The units directory through a removal-capable handle, or `None`
    /// when no unit was ever installed.
    fn units_directory(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<PublicDirectoryAuthority>, InstallStoreError> {
        self.authority(lock)?;
        match lock.open_public_directory(&self.root.join(UNITS_DIRECTORY)) {
            Ok(units) => Ok(Some(units)),
            Err(InstallStoreError::OpenPublicDirectory(error))
                if error.kind() == io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

/// Every unit a journal names: both sides and every platform state.
fn journal_units(journal: &InstallJournalV1) -> Vec<UnitId> {
    let mut units = vec![journal.candidate_unit.clone()];
    units.extend(journal.prior_active_unit.clone());
    for state in [&journal.prior_platform, &journal.target_platform] {
        units.extend(
            [
                state.layout_unit.clone(),
                state.launcher_unit.clone(),
                state.running_unit.clone(),
            ]
            .into_iter()
            .flatten(),
        );
    }
    units
}
