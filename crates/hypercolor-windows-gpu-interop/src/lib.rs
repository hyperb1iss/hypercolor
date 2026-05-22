#![deny(missing_docs)]

//! Windows GPU interop helpers for Servo effect frames.

#[cfg(not(target_os = "windows"))]
mod stubs;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(not(target_os = "windows"))]
pub use stubs::*;
#[cfg(target_os = "windows")]
pub use windows::*;
