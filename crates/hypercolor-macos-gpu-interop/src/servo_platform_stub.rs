//! Off-macOS stand-in for the Servo platform constructor.

use hypercolor_gpu_frame::servo::ServoRenderPlatform;

/// The macOS Servo platform never exists off macOS.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    None
}
