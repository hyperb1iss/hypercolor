#![deny(missing_docs)]

//! macOS GPU interop helpers for Servo effect frames.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(target_os = "macos", feature = "servo-context"))]
mod servo_context;
#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(all(target_os = "macos", feature = "servo-context"))]
pub use servo_context::*;
#[cfg(not(target_os = "macos"))]
pub use stubs::*;
