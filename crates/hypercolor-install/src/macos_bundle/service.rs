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

    /// Persistently allow or forbid the agent to start on its own.
    ///
    /// # Errors
    /// Returns an error when the service manager refuses.
    fn set_enabled(&mut self, enabled: bool) -> Result<(), InstallPlatformError>;

    /// Start the agent, which runs the installed bundle's daemon.
    ///
    /// # Errors
    /// Returns an error when the agent cannot be started.
    fn start(&mut self) -> Result<(), InstallPlatformError>;

    /// Stop exactly `daemon`, and nothing that replaced it.
    ///
    /// # Errors
    /// Returns an error when it cannot be stopped.
    fn stop(&mut self, daemon: &MacosBundleRunningDaemon) -> Result<(), InstallPlatformError>;

    /// Wait until no daemon of this installation still holds its
    /// single-instance guard.
    ///
    /// # Errors
    /// Returns an error when the guard is not released in time.
    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError>;

    /// Prove `daemon` healthy: ready, answering as `version` on its local
    /// API, and still running when its stability window ends.
    ///
    /// # Errors
    /// Returns an error when any part of the proof fails.
    fn prove(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        version: &str,
    ) -> Result<(), InstallPlatformError>;
}
