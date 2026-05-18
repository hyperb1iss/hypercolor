#![deny(missing_docs)]

//! macOS GPU interop helpers for Servo effect frames.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(not(target_os = "macos"))]
pub use stubs::*;
