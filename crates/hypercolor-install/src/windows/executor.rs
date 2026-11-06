use std::time::Duration;

use super::model::{
    WindowsBrokerObservation, WindowsFirewallObservation, WindowsFirewallRule, WindowsProcessId,
    WindowsProcessImage, WindowsRuntimeObservation, WindowsUnitBinding,
};
use crate::{InstallPlatformError, LocalHttpResponse, UnitId, UnitRecord};

/// What a Windows installation does to the machine and asks of it.
///
/// Every effect is one exact step, durable on return, and idempotent from
/// the observation the platform matched first. The platform decides which
/// step to take and what an observation means.
pub trait WindowsInstallExecutor {
    /// The unit `active.json` names.
    ///
    /// # Errors
    /// Returns an error for an unreadable or invalid pointer.
    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError>;

    /// Read what `unit`'s manifest and files state.
    ///
    /// # Errors
    /// Returns an error for a unit that is not an exact Windows unit.
    fn bind_unit(&mut self, unit: &UnitRecord) -> Result<WindowsUnitBinding, InstallPlatformError>;

    /// The SMBus broker's registration and state, or `None` when no broker
    /// is registered.
    ///
    /// # Errors
    /// Returns an error when the service control manager cannot be asked.
    fn observe_broker(&mut self) -> Result<Option<WindowsBrokerObservation>, InstallPlatformError>;

    /// Stop the broker and wait until it stopped, then register `program`
    /// as its image, keeping the rest of its registration.
    ///
    /// # Errors
    /// Returns an error when it does not stop in time or the service
    /// control manager refuses the change.
    fn switch_broker(&mut self, program: &str) -> Result<(), InstallPlatformError>;

    /// Start the broker unless it runs, wait until it answers on its pipe,
    /// and return the process it runs in.
    ///
    /// # Errors
    /// Returns an error when it does not reach running or does not answer
    /// in time.
    fn start_broker(&mut self) -> Result<WindowsProcessId, InstallPlatformError>;

    /// The programs the firewall rules allow, or `None` when the firewall
    /// service does not run.
    ///
    /// # Errors
    /// Returns an error when a running firewall cannot be asked.
    fn observe_firewall(
        &mut self,
    ) -> Result<Option<WindowsFirewallObservation>, InstallPlatformError>;

    /// Make `rule` allow `program` inbound, or remove it when `None`.
    ///
    /// # Errors
    /// Returns an error when the firewall refuses.
    fn set_firewall_rule(
        &mut self,
        rule: WindowsFirewallRule,
        program: Option<&str>,
    ) -> Result<(), InstallPlatformError>;

    /// Every process running under the app's or the daemon's file name,
    /// from wherever it runs.
    ///
    /// # Errors
    /// Returns an error when the process list cannot be read.
    fn observe_runtime(&mut self) -> Result<WindowsRuntimeObservation, InstallPlatformError>;

    /// The image `process` runs, read through the file system, or `None`
    /// when it has exited.
    ///
    /// # Errors
    /// Returns an error when the image cannot be opened or hashed.
    fn process_image(
        &mut self,
        process: WindowsProcessId,
    ) -> Result<Option<WindowsProcessImage>, InstallPlatformError>;

    /// Wait on the process handles until every process in `processes` has
    /// exited. Returns `false` when `timeout` passes first.
    ///
    /// # Errors
    /// Returns an error when the wait fails.
    fn wait_for_every_exit(
        &mut self,
        processes: &[WindowsProcessId],
        timeout: Duration,
    ) -> Result<bool, InstallPlatformError>;

    /// Wait on the process handles for `timeout`, ending at once when any
    /// process in `processes` exits, and return that process.
    ///
    /// # Errors
    /// Returns an error when the wait fails.
    fn wait_for_any_exit(
        &mut self,
        processes: &[WindowsProcessId],
        timeout: Duration,
    ) -> Result<Option<WindowsProcessId>, InstallPlatformError>;

    /// End `processes` by force, without waiting for them.
    ///
    /// # Errors
    /// Returns an error when one that still runs cannot be ended.
    fn terminate(&mut self, processes: &[WindowsProcessId]) -> Result<(), InstallPlatformError>;

    /// Wait until the daemon `app` supervises answers on its local API,
    /// which it does only once it finished starting.
    ///
    /// # Errors
    /// Returns an error when `app` exits or `timeout` passes first.
    fn wait_for_daemon(
        &mut self,
        app: WindowsProcessId,
        timeout: Duration,
    ) -> Result<(), InstallPlatformError>;

    /// One bounded request to the daemon's local API.
    ///
    /// # Errors
    /// Returns an error when the request fails or exceeds `max_bytes`.
    fn http_get(
        &mut self,
        path: &str,
        max_bytes: usize,
    ) -> Result<LocalHttpResponse, InstallPlatformError>;
}
