#![deny(missing_docs)]

//! Linux GPU interop helpers for Servo effect frames.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod stubs;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(not(target_os = "linux"))]
pub use stubs::*;
