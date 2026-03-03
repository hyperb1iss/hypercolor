//! Servo feature bootstrap helpers.
//!
//! This module is intentionally minimal for Phase 6.1:
//! - verify that `libservo` is wired correctly behind a crate feature
//! - provide a tiny API to create a headless software rendering context
//! - keep all Servo-specific types out of non-Servo builds

use anyhow::{Result, anyhow};
use dpi::PhysicalSize;
use servo::SoftwareRenderingContext;

/// Create a headless Servo software rendering context.
///
/// This is the first integration seam for HTML effect rendering. Later phases
/// will layer `ServoBuilder`, `WebView`, and runtime JS/audio injection on top.
///
/// # Errors
///
/// Returns an error if the software OpenGL adapter/context cannot be created.
pub fn bootstrap_software_rendering_context(
    width: u32,
    height: u32,
) -> Result<SoftwareRenderingContext> {
    SoftwareRenderingContext::new(PhysicalSize::new(width, height)).map_err(|error| {
        anyhow!("failed to create Servo SoftwareRenderingContext ({width}x{height}): {error:?}")
    })
}
