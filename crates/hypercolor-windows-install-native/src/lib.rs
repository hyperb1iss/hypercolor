//! Audited Win32 calls behind the Windows install platform.
//!
//! `hypercolor-install` forbids unsafe code, so the two families of calls
//! it cannot reach through a safe binding live here: naming, waiting on and
//! ending processes through retained handles, and reading and writing
//! Windows Firewall rules through `INetFwPolicy2`. Every decision about
//! what those observations mean stays in `hypercolor-install`. The crate is
//! empty on other platforms.

#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod firewall;
mod process;

pub use firewall::{FirewallRule, firewall_rule, set_firewall_rule_program};
pub use process::{
    ProcessId, RunningProcess, process, process_with_pid, processes_named, terminate_processes,
    wait_for_any_exit, wait_for_every_exit,
};
