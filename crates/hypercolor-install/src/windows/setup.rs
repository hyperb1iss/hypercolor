//! What an installer does with the release unit it carries.
//!
//! NSIS only unpacks a unit and runs that unit's own CLI, which takes the
//! machine to one place and is safe to run again from wherever the last run
//! stopped. Every step observes what exists and does what is missing, and a
//! killed installer is finished by running the installer again:
//!
//! 1. finish a transaction a killed installer left open;
//! 2. publish the unit in the store;
//! 3. switch to it in one coordinated transaction, which moves the broker's
//!    `ImagePath`, the firewall rules and `active.json`, and starts nothing;
//! 4. record the installation;
//! 5. register the update service and give the recorded user its records'
//!    directory, or take the service away from a build that has no updater
//!    to run behind it;
//! 6. drop every unit nothing refers to any more.
//!
//! A flat layout goes through exactly this. It has no unit, so the
//! transaction is a first install whose journal retained the flat broker
//! path and firewall rules, and a rollback puts those back. The flat files
//! stay on disk until the installer has laid down what replaces them, so a
//! run killed anywhere leaves a Hypercolor that still starts.
//!
//! Everything here that decides what happens sits behind [`WindowsSetupHost`],
//! so it runs against fakes on any platform.

use super::machine_layout::BootstrapSlot;
use crate::{InstallPlatformError, UnitId};

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::{WindowsInstallRequest, install_windows_release};

/// Who can finish a transaction someone left open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTransaction {
    /// It starts no release on either side, as an installer's never does,
    /// so any elevated process can finish it.
    StartsNothing,
    /// It needs the recorded user's session, which only the update service
    /// can reach.
    StartsARelease,
}

/// What an installer asks of the machine, one step at a time. Each step
/// is durable on return.
pub trait WindowsSetupHost {
    /// The transaction a journal left open, if any.
    ///
    /// # Errors
    /// Returns an error for an unreadable journal.
    fn pending_transaction(&mut self) -> Result<Option<PendingTransaction>, InstallPlatformError>;

    /// Drive the open transaction to its end, whichever end that is.
    ///
    /// # Errors
    /// Returns an error when it cannot be driven further.
    fn finish_transaction(&mut self) -> Result<(), InstallPlatformError>;

    /// Publish the unit this installer carries in the store.
    ///
    /// # Errors
    /// Returns an error when the unit is not exactly what the installer was
    /// built with.
    fn stage_candidate(&mut self) -> Result<(), InstallPlatformError>;

    /// The unit `active.json` names.
    ///
    /// # Errors
    /// Returns an error for an unreadable pointer.
    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError>;

    /// Switch to the staged unit in one transaction that starts nothing.
    ///
    /// # Errors
    /// Returns why it did not commit. A rollback has already restored what
    /// the transaction changed.
    fn install_candidate(&mut self) -> Result<(), InstallPlatformError>;

    /// Whether `installation.json` exists.
    ///
    /// # Errors
    /// Returns an error for a record that cannot be trusted or read.
    fn installation_recorded(&mut self) -> Result<bool, InstallPlatformError>;

    /// Write `installation.json` for a new installation.
    ///
    /// # Errors
    /// Returns an error when the record cannot be written.
    fn record_installation(&mut self) -> Result<(), InstallPlatformError>;

    /// The slot the update service's `ImagePath` names, or `None` when the
    /// service is not registered.
    ///
    /// # Errors
    /// Returns an error when the service manager cannot be asked.
    fn update_service_slot(&mut self) -> Result<Option<BootstrapSlot>, InstallPlatformError>;

    /// Give the recorded user the directory its daemon keeps update records
    /// in. The state root is the machine's, so that daemon cannot make it.
    ///
    /// # Errors
    /// Returns an error when the directory cannot be created.
    fn prepare_coordinator_directory(&mut self) -> Result<(), InstallPlatformError>;

    /// Write the active unit's bootstrap into both slots.
    ///
    /// # Errors
    /// Returns an error when a slot cannot be written.
    fn write_bootstrap_slots(&mut self) -> Result<(), InstallPlatformError>;

    /// Prove the bootstrap in `slot` loads.
    ///
    /// # Errors
    /// Returns an error when it does not.
    fn probe_bootstrap(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError>;

    /// Run the bootstrap in `slot` with its register command.
    ///
    /// # Errors
    /// Returns an error when it does not exit zero.
    fn register_update_service(&mut self, slot: BootstrapSlot) -> Result<(), InstallPlatformError>;

    /// Delete the update service and its slots.
    ///
    /// # Errors
    /// Returns an error when the service stays.
    fn remove_update_service(&mut self) -> Result<(), InstallPlatformError>;

    /// Remove every unit but the active one and every leftover of a killed
    /// run. Returns a note for each entry that stayed behind.
    ///
    /// # Errors
    /// Returns an error when the records cannot be read.
    fn collect_units(&mut self) -> Result<Vec<String>, InstallPlatformError>;
}

/// Take the machine to `candidate` as its active, recorded installation,
/// and return a note for anything that stayed behind.
///
/// `managed_update` says this build ships the updater, so the update
/// service belongs on the machine. Without it the service does not.
///
/// # Errors
///
/// Returns the first step that failed. Running this again resumes there.
pub fn run_windows_setup<H: WindowsSetupHost>(
    host: &mut H,
    candidate: &UnitId,
    managed_update: bool,
) -> Result<Vec<String>, InstallPlatformError> {
    match host.pending_transaction()? {
        Some(PendingTransaction::StartsARelease) => {
            return Err(InstallPlatformError::new(
                "an update of this installation has not finished; sign in and let it finish, \
                 then run this installer again",
            ));
        }
        Some(PendingTransaction::StartsNothing) => host.finish_transaction()?,
        None => {}
    }
    host.stage_candidate()?;
    if host.active_unit()?.as_ref() != Some(candidate) {
        host.install_candidate()?;
    }
    if !host.installation_recorded()? {
        host.record_installation()?;
    }
    if managed_update {
        host.prepare_coordinator_directory()?;
    }
    match (managed_update, host.update_service_slot()?) {
        // A killed run may have left the registration half done, and an
        // update may have moved the service to the other slot, so the slot
        // it names repeats the whole registration.
        (true, Some(slot)) => host.register_update_service(slot)?,
        (true, None) => {
            host.write_bootstrap_slots()?;
            for slot in [BootstrapSlot::A, BootstrapSlot::B] {
                host.probe_bootstrap(slot)?;
            }
            host.register_update_service(BootstrapSlot::A)?;
        }
        (false, Some(_)) => host.remove_update_service()?,
        (false, None) => {}
    }
    host.collect_units()
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
