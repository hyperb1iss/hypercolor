//! Physical-GPU agreement between Servo's GL context and wgpu's Vulkan device.
//!
//! `GL_EXT_memory_object` only defines importing Vulkan memory into GL when
//! both APIs report the same device and driver UUIDs. surfman does not let
//! Hypercolor choose the GPU behind Servo's EGL display: it follows the
//! process environment at display initialization, so a GL context can land
//! on another GPU or on Mesa's llvmpipe beside a hardware Vulkan device. Such
//! a context can accept the import calls and still render garbage, so the
//! importer checks agreement before it allocates shared memory.

use ash::vk;
use glow::HasContext;

use super::gl_external_memory::{GlExternalMemoryFunctions, clear_gl_errors};
use super::{LinuxGpuInteropError, Result};

const GL_NUM_DEVICE_UUIDS_EXT: u32 = 0x9596;
const GL_DEVICE_UUID_EXT: u32 = 0x9597;
const GL_DRIVER_UUID_EXT: u32 = 0x9598;
/// Upper bound on GL device UUIDs read from one context, guarding against a
/// driver that reports a nonsensical count.
const MAX_GL_DEVICE_UUIDS: u32 = 16;
const UNREPORTED_UUID: [u8; 16] = [0; 16];

/// Device and driver UUIDs an API reports for the GPU it runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GpuDeviceIdentity {
    /// Device UUIDs; GL may report several for a multi-device context.
    device_uuids: Vec<[u8; 16]>,
    driver_uuid: [u8; 16],
}

impl GpuDeviceIdentity {
    /// Loaders that emulate the properties query without the ID extension
    /// leave the UUIDs zeroed; treat that as not reporting.
    fn is_reported(&self) -> bool {
        self.driver_uuid != UNREPORTED_UUID
            && self
                .device_uuids
                .iter()
                .any(|uuid| *uuid != UNREPORTED_UUID)
    }
}

/// Returns `true` when both identities name the same driver and share a
/// device, the condition `GL_EXT_memory_object` places on memory import.
fn identities_share_device(gl: &GpuDeviceIdentity, vulkan: &GpuDeviceIdentity) -> bool {
    gl.driver_uuid == vulkan.driver_uuid
        && vulkan
            .device_uuids
            .iter()
            .any(|uuid| gl.device_uuids.contains(uuid))
}

/// Fails when the current GL context and the wgpu Vulkan device run on
/// different GPUs or drivers. Contexts or devices that cannot report UUIDs,
/// or report all-zero ones, pass, since there is nothing to compare.
pub(super) fn verify_shared_physical_device(
    gl: &glow::Context,
    gl_external_memory: &GlExternalMemoryFunctions,
    hal_device: &wgpu_hal::vulkan::Device,
) -> Result<()> {
    let (Some(gl_identity), Some(vulkan_identity)) = (
        gl_device_identity(gl, gl_external_memory),
        vulkan_device_identity(hal_device),
    ) else {
        return Ok(());
    };
    if identities_share_device(&gl_identity, &vulkan_identity) {
        Ok(())
    } else {
        Err(LinuxGpuInteropError::DeviceUuidMismatch {
            gl: format_identity(&gl_identity),
            vulkan: format_identity(&vulkan_identity),
        })
    }
}

fn gl_device_identity(
    gl: &glow::Context,
    functions: &GlExternalMemoryFunctions,
) -> Option<GpuDeviceIdentity> {
    // SAFETY: the caller holds the GL context current; GL_NUM_DEVICE_UUIDS_EXT
    // is a plain integer query that drivers without the enum reject with
    // GL_INVALID_ENUM, which is drained below.
    let count = unsafe { gl.get_parameter_i32(GL_NUM_DEVICE_UUIDS_EXT) };
    let identity = u32::try_from(count)
        .ok()
        .filter(|count| *count > 0)
        .map(|count| {
            let count = count.min(MAX_GL_DEVICE_UUIDS);
            let device_uuids = (0..count)
                .map(|index| {
                    let mut uuid = [0_u8; 16];
                    // SAFETY: GL_DEVICE_UUID_EXT writes GL_UUID_SIZE_EXT (16)
                    // bytes for an index below GL_NUM_DEVICE_UUIDS_EXT.
                    unsafe {
                        (functions.get_unsigned_bytei_v_ext)(
                            GL_DEVICE_UUID_EXT,
                            index,
                            uuid.as_mut_ptr(),
                        );
                    }
                    uuid
                })
                .collect();
            let mut driver_uuid = [0_u8; 16];
            // SAFETY: GL_DRIVER_UUID_EXT writes GL_UUID_SIZE_EXT (16) bytes.
            unsafe {
                (functions.get_unsigned_bytev_ext)(GL_DRIVER_UUID_EXT, driver_uuid.as_mut_ptr());
            }
            GpuDeviceIdentity {
                device_uuids,
                driver_uuid,
            }
        });
    clear_gl_errors(gl);
    identity.filter(GpuDeviceIdentity::is_reported)
}

fn vulkan_device_identity(hal_device: &wgpu_hal::vulkan::Device) -> Option<GpuDeviceIdentity> {
    let instance = hal_device.shared_instance();
    if instance.instance_api_version() < vk::API_VERSION_1_1 {
        return None;
    }
    let mut id_properties = vk::PhysicalDeviceIDProperties::default();
    {
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id_properties);
        // SAFETY: the physical device belongs to this instance through the
        // active wgpu HAL, and the instance is Vulkan 1.1 or newer.
        unsafe {
            instance
                .raw_instance()
                .get_physical_device_properties2(hal_device.raw_physical_device(), &mut properties);
        }
    }
    Some(GpuDeviceIdentity {
        device_uuids: vec![id_properties.device_uuid],
        driver_uuid: id_properties.driver_uuid,
    })
    .filter(GpuDeviceIdentity::is_reported)
}

fn format_identity(identity: &GpuDeviceIdentity) -> String {
    let devices = identity
        .device_uuids
        .iter()
        .map(|uuid| hex(uuid))
        .collect::<Vec<_>>()
        .join(",");
    format!("device=[{devices}] driver={}", hex(&identity.driver_uuid))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::{GpuDeviceIdentity, identities_share_device};

    fn identity(device_uuids: &[[u8; 16]], driver_uuid: [u8; 16]) -> GpuDeviceIdentity {
        GpuDeviceIdentity {
            device_uuids: device_uuids.to_vec(),
            driver_uuid,
        }
    }

    #[test]
    fn matching_device_and_driver_share_a_device() {
        let vulkan = identity(&[[1; 16]], [9; 16]);
        let gl = identity(&[[2; 16], [1; 16]], [9; 16]);

        assert!(identities_share_device(&gl, &vulkan));
    }

    #[test]
    fn a_different_device_does_not_share() {
        let vulkan = identity(&[[1; 16]], [9; 16]);
        let gl = identity(&[[2; 16]], [9; 16]);

        assert!(!identities_share_device(&gl, &vulkan));
    }

    #[test]
    fn zeroed_uuids_count_as_unreported() {
        assert!(!identity(&[[0; 16]], [0; 16]).is_reported());
        assert!(!identity(&[[1; 16]], [0; 16]).is_reported());
        assert!(!identity(&[[0; 16]], [9; 16]).is_reported());
        assert!(identity(&[[0; 16], [1; 16]], [9; 16]).is_reported());
    }

    #[test]
    fn the_same_device_under_a_different_driver_does_not_share() {
        let vulkan = identity(&[[1; 16]], [9; 16]);
        let gl = identity(&[[1; 16]], [8; 16]);

        assert!(!identities_share_device(&gl, &vulkan));
    }
}
