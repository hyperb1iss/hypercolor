#![deny(missing_docs)]

//! Windows host telemetry for Hypercolor.
//!
//! Two probes live here: motherboard identity via WMI `Win32_BaseBoard`, and
//! the sensor cascade (PawnIO MSR/SMN CPU temperature, LibreHardwareMonitor /
//! OpenHardwareMonitor, ACPI thermal zones) that backfills the neutral
//! [`SystemSnapshot`]. The crate compiles on every target: off Windows the
//! probes report nothing, so neutral callers never branch on the operating
//! system.

pub use hypercolor_types::motherboard::MotherboardInfo;
pub use hypercolor_types::sensor::SystemSnapshot;

#[cfg(target_os = "windows")]
mod board;
#[cfg(target_os = "windows")]
mod sensors;
#[cfg(not(target_os = "windows"))]
mod stubs;

#[cfg(target_os = "windows")]
pub use board::motherboard_info;
#[cfg(target_os = "windows")]
pub use sensors::SensorExtras;
#[cfg(not(target_os = "windows"))]
pub use stubs::{SensorExtras, motherboard_info};
