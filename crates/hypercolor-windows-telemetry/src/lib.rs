#![deny(missing_docs)]

//! Windows host telemetry for Hypercolor.
//!
//! Three probes live here: motherboard identity via WMI `Win32_BaseBoard`,
//! the running-software inventory via `Win32_Process` and `Win32_Service`,
//! and the sensor cascade (PawnIO MSR/SMN CPU temperature,
//! LibreHardwareMonitor / OpenHardwareMonitor, ACPI thermal zones) that
//! backfills the neutral [`SystemSnapshot`]. The crate compiles on every
//! target: off Windows the probes report nothing, so neutral callers never
//! branch on the operating system.

pub use hypercolor_types::host_software::HostSoftwareSnapshot;
pub use hypercolor_types::motherboard::MotherboardInfo;
pub use hypercolor_types::sensor::SystemSnapshot;

#[cfg(target_os = "windows")]
mod board;
#[cfg(target_os = "windows")]
mod sensors;
#[cfg(target_os = "windows")]
mod software;
#[cfg(not(target_os = "windows"))]
mod stubs;

#[cfg(target_os = "windows")]
pub use board::motherboard_info;
#[cfg(target_os = "windows")]
pub use sensors::SensorExtras;
#[cfg(target_os = "windows")]
pub use software::running_software;
#[cfg(not(target_os = "windows"))]
pub use stubs::{SensorExtras, motherboard_info, running_software};
