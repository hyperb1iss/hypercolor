use std::time::Duration;

use crate::coordinator_error::MacosOwnerExecutionError;
use crate::model::{MacosDaemonOwner, MacosOwnerIncarnation};

/// Closed local process and launcher operations used by the coordinator.
pub trait MacosOwnerExecutor {
    /// Return whether one managed topology is configured for login startup.
    fn autostart_enabled(
        &mut self,
        owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Idempotently set one managed topology's login-start state.
    fn set_autostart(
        &mut self,
        owner: MacosDaemonOwner,
        enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Verify that this executor can stop one exact managed owner acquisition.
    fn preflight_stop_authority(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Flush and stop one exact managed owner acquisition.
    fn flush_and_stop(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError>;

    /// Idempotently start one managed owner.
    fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError>;

    /// Wait until the canonical daemon guard can be acquired.
    fn wait_for_guard_release(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Wait for the requested owner to publish an epoch newer than `after_epoch`.
    fn wait_for_owner(
        &mut self,
        owner: MacosDaemonOwner,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError>;
}
