#![deny(missing_docs)]

//! Linux GPU interop helpers for Servo effect frames.

pub use hypercolor_gpu_frame::{
    FrameOrigin, GpuFrameImportError, GpuFrameImportFallbackReason, ImportedEffectFrame,
    ImportedFrameAllocationId, ImportedFrameFormat, ImportedFrameLease, ImportedFrameTimings,
};

/// Linux-native format projections for a neutral imported frame format.
pub trait LinuxImportedFrameFormatExt {
    /// Returns the matching GL internal format.
    fn gl_internal_format(self) -> Option<u32>;

    /// Returns the matching Vulkan image format.
    fn vk_format(self) -> Option<ash::vk::Format>;
}

impl LinuxImportedFrameFormatExt for ImportedFrameFormat {
    fn gl_internal_format(self) -> Option<u32> {
        match self {
            ImportedFrameFormat::Rgba8Unorm => Some(glow::RGBA8),
            _ => None,
        }
    }

    fn vk_format(self) -> Option<ash::vk::Format> {
        match self {
            ImportedFrameFormat::Rgba8Unorm => Some(ash::vk::Format::R8G8B8A8_UNORM),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
mod servo_context;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
mod servo_platform;
#[cfg(all(not(target_os = "linux"), feature = "servo-context"))]
mod servo_platform_stub;
#[cfg(not(target_os = "linux"))]
mod stubs;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
pub use servo_context::*;
#[cfg(all(target_os = "linux", feature = "servo-context"))]
pub use servo_platform::{LinuxServoPlatform, servo_render_platform};
#[cfg(all(not(target_os = "linux"), feature = "servo-context"))]
pub use servo_platform_stub::servo_render_platform;
#[cfg(not(target_os = "linux"))]
pub use stubs::*;
