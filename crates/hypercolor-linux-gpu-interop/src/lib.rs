#![deny(missing_docs)]

//! Linux GPU interop helpers for Servo effect frames.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
mod servo_context;
#[cfg(not(target_os = "linux"))]
mod stubs;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
pub use servo_context::*;
#[cfg(not(target_os = "linux"))]
pub use stubs::*;
