//! Windows launcher authority: who positively owns this daemon process.
//!
//! Running under the Service Control Manager is proven by the dispatcher
//! itself (a process that was not started by SCM cannot attach to it), the
//! desktop app by its supervised-parent claim matching the live parent,
//! and everything else is the standalone residual.

use hypercolor_types::service::ServiceIdentity;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

use crate::startup::SUPERVISED_PARENT_PID_ENV;

/// Measure which launchers own the current process.
///
/// `service_dispatch` is true when the process was asked to attach to the
/// Service Control Manager; the attach itself fails for any process SCM
/// did not start, so the flag plus a successful dispatch is the proof.
#[must_use]
pub fn attested_windows_launchers(service_dispatch: bool) -> Vec<ServiceIdentity> {
    if service_dispatch {
        return vec![ServiceIdentity::windows_scm()];
    }
    let supervised_child = std::env::var(SUPERVISED_PARENT_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|claimed| current_parent_pid() == Some(claimed));
    if supervised_child {
        vec![ServiceIdentity::APP_SIDECAR]
    } else {
        Vec::new()
    }
}

fn current_parent_pid() -> Option<u32> {
    let current = Pid::from_u32(std::process::id());
    let mut system = System::new_with_specifics(RefreshKind::nothing());
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[current]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(current)?.parent().map(Pid::as_u32)
}
