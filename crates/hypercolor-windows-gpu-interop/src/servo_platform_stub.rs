//! Off-Windows stand-in for the Servo platform constructor.

use hypercolor_gpu_frame::servo::ServoRenderPlatform;

/// The Windows Servo platform never exists off Windows.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    None
}
