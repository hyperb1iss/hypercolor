use std::time::Duration;

use super::super::InstallPlatformError;
use super::model::{MacosBundleRunningDaemon, MacosBundleServiceObservation};

/// The daemon's service manager, as an app bundle installation drives it.
///
/// The service always runs the daemon inside the installed bundle, so a
/// swap changes which release it starts without touching its definition.
/// The platform decides which daemon is which from its cdhash and path; an
/// implementation only observes and acts.
pub trait MacosBundleService {
    /// Observe the daemon agent and the daemon it runs.
    ///
    /// # Errors
    /// Returns an error when the service manager cannot be queried.
    fn observe(&mut self) -> Result<MacosBundleServiceObservation, InstallPlatformError>;

    /// Persistently allow or forbid the service manager to load the agent,
    /// at login or when asked. An agent already loaded is unaffected.
    ///
    /// # Errors
    /// Returns an error when the service manager refuses.
    fn set_enabled(&mut self, enabled: bool) -> Result<(), InstallPlatformError>;

    /// Load the agent and wait until the daemon with `cdhash` runs from the
    /// installed bundle and has claimed the installation.
    ///
    /// # Errors
    /// Returns an error when the agent cannot be loaded, or that daemon does
    /// not get there before the start deadline. A daemon that crashes on the
    /// way is such a failure, so the transaction rolls back rather than
    /// starting it again.
    fn start(&mut self, cdhash: &str) -> Result<(), InstallPlatformError>;

    /// Unload the agent, stopping exactly `daemon` and nothing that replaced
    /// it. With `None` no daemon may be running, and this removes a job the
    /// service manager still holds to restart a daemon that exited.
    ///
    /// # Errors
    /// Returns an error when it cannot be stopped or unloaded.
    fn stop(
        &mut self,
        daemon: Option<&MacosBundleRunningDaemon>,
    ) -> Result<(), InstallPlatformError>;

    /// Wait until no daemon of this installation still holds its
    /// single-instance guard.
    ///
    /// # Errors
    /// Returns an error when the guard is not released in time.
    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError>;

    /// Prove `daemon` healthy: ready, answering as `version` on its local
    /// API, and, with a nonzero `probation`, still running when that window
    /// ends and still proving all of it.
    ///
    /// # Errors
    /// Returns an error when any part of the proof fails. A daemon that
    /// exits inside the window fails at once.
    fn prove(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        version: &str,
        probation: Duration,
    ) -> Result<(), InstallPlatformError>;
}
