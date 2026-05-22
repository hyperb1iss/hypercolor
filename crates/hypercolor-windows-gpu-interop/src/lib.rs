#![deny(missing_docs)]

//! Windows GPU interop helpers for Servo effect frames.

#[cfg(all(target_os = "windows", feature = "servo-context"))]
mod servo_context;
#[cfg(not(target_os = "windows"))]
mod stubs;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(target_os = "windows", feature = "servo-context"))]
pub use servo_context::*;
#[cfg(not(target_os = "windows"))]
pub use stubs::*;
#[cfg(target_os = "windows")]
pub use windows::*;
