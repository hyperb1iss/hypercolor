#![deny(missing_docs)]

//! Windows GPU interop helpers for Servo effect frames.

/// Operation label used when creating the ANGLE client-buffer surface fails.
pub const WINDOWS_ANGLE_CLIENT_BUFFER_SURFACE_OPERATION: &str =
    "create ANGLE client-buffer surface";

/// Phase 1 Windows Servo GPU import synchronization mode.
pub const WINDOWS_SERVO_GPU_IMPORT_SYNC_MODE: &str = "gl_finish";

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
