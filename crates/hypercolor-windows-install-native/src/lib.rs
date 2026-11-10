//! Audited Win32 calls behind the Windows install platform.
//!
//! `hypercolor-install` forbids unsafe code, so the calls it cannot reach
//! through a safe binding live here: naming, waiting on and ending processes
//! through retained handles, waiting for a service to start, reading and
//! writing Windows Firewall rules through `INetFwPolicy2`, writing the
//! registry's `SYSTEM` hive to disk, and the two token questions an
//! installer has.
//! Every decision about what those observations mean stays in
//! `hypercolor-install`. The crate is empty on other platforms.

#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod firewall;
mod process;
mod registry;
mod service;
mod token;

pub use firewall::{FirewallRule, firewall_rule, set_firewall_rule_program};
pub use process::{
    ProcessId, RunningProcess, process, process_with_pid, processes_named, terminate_processes,
    wait_for_any_exit, wait_for_every_exit,
};
pub use registry::flush_system_hive;
pub use service::ServiceStarts;
pub use token::{desktop_user_sid, own_new_objects_as_administrators};
