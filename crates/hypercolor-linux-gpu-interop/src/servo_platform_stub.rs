//! Off-Linux stand-in for the Servo platform constructor.

use hypercolor_gpu_frame::servo::ServoRenderPlatform;

/// The Linux Servo platform never exists off Linux.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    None
}
