#![deny(missing_docs)]

//! macOS GPU interop helpers for IOSurface-backed frames.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
mod native_reduction;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
mod screen_capture;
#[cfg(all(target_os = "macos", feature = "servo-context"))]
mod servo_context;
#[cfg(not(target_os = "macos"))]
mod stubs;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub use native_reduction::*;
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
pub use screen_capture::*;
#[cfg(all(target_os = "macos", feature = "servo-context"))]
pub use servo_context::*;
#[cfg(not(target_os = "macos"))]
pub use stubs::*;
