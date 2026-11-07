use std::path::Path;

use super::super::{InstallPlatformError, UnitRecord};
use super::model::{MacosBundleMaterialization, MacosBundleObservation, MacosBundleUnit};

/// The filesystem effects of an app bundle installation.
///
/// Every effect is one exact step on the installed bundle and the staging
/// root beside it, durable on return, and idempotent from the observation
/// the platform matched first. `name` is always the transaction's staging
/// name, the candidate's digest: its staged copy lives at
/// `<staging root>/<name>/Hypercolor.app`.
pub trait MacosBundleExecutor {
    /// Observe the installed bundle, every staged copy and every leftover of
    /// an interrupted step.
    ///
    /// # Errors
    /// Returns an error when the locations cannot be inspected at all. A
    /// location that holds something other than a readable bundle is
    /// reported as unrecognized instead.
    fn observe(&mut self) -> Result<MacosBundleObservation, InstallPlatformError>;

    /// Bytes available on the filesystem holding the installed bundle.
    ///
    /// # Errors
    /// Returns an error when the filesystem cannot be queried.
    fn available_bytes(&mut self) -> Result<u64, InstallPlatformError>;

    /// Put `unit`'s bundle at the staged copy named `name`.
    ///
    /// Leftovers of an earlier attempt are removed first. The copy is built
    /// under a scratch name, cloned from `source` where the filesystem can
    /// and durably copied otherwise, verified against every member of
    /// `unit`, synced, and only then renamed to `name`. The staged copy is
    /// therefore either absent or complete.
    ///
    /// # Errors
    /// Returns an error when the copy cannot be made, verified or published;
    /// the scratch copy is removed where possible.
    fn materialize(
        &mut self,
        source: &UnitRecord,
        unit: &MacosBundleUnit,
        name: &str,
    ) -> Result<MacosBundleMaterialization, InstallPlatformError>;

    /// Copy the installed bundle to `Hypercolor.app` inside the existing
    /// directory `destination`, cloned where the filesystem can, with no
    /// member writable by anyone but its owner. The installed bundle is only
    /// read.
    ///
    /// # Errors
    /// Returns an error when the bundle holds anything but directories and
    /// regular files, or the copy cannot be made.
    fn snapshot_installed(&mut self, destination: &Path) -> Result<(), InstallPlatformError>;

    /// Exchange the staged copy with the installed bundle in one atomic
    /// rename (`renamex_np` with `RENAME_SWAP`).
    ///
    /// # Errors
    /// Returns an error when either side is missing or the exchange fails.
    fn exchange(&mut self, name: &str) -> Result<(), InstallPlatformError>;

    /// Move the staged copy into place where nothing is installed, refusing
    /// to replace anything.
    ///
    /// # Errors
    /// Returns an error when something is installed or the move fails.
    fn install_staged(&mut self, name: &str) -> Result<(), InstallPlatformError>;

    /// Move the installed bundle back to the staged copy's name, refusing to
    /// replace anything.
    ///
    /// # Errors
    /// Returns an error when a staged copy exists or the move fails.
    fn withdraw_installed(&mut self, name: &str) -> Result<(), InstallPlatformError>;

    /// Move the staged copy named `name` to its discard name in one durable
    /// rename, leaving its removal to [`Self::discard`].
    ///
    /// # Errors
    /// Returns an error when the rename fails.
    fn retire_staged(&mut self, name: &str) -> Result<(), InstallPlatformError>;

    /// Remove the staged copy named `name` and every leftover of it.
    ///
    /// The copy is renamed to its discard name first, so a removal that
    /// is interrupted leaves only a leftover, never a torn staged copy.
    ///
    /// # Errors
    /// Returns an error when the removal fails.
    fn discard(&mut self, name: &str) -> Result<(), InstallPlatformError>;
}
