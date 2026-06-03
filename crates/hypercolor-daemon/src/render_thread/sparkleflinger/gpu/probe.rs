use anyhow::Result;

use super::COMPOSITOR_TEXTURE_FORMAT;
use crate::render_thread::gpu_device::{
    GpuBackendPreference, GpuRenderDevice, backend_name, device_type_name, texture_format_name,
};

#[derive(Debug, Clone)]
pub(crate) struct GpuCompositorProbe {
    pub(crate) adapter_name: String,
    pub(crate) adapter_device_type: &'static str,
    pub(crate) backend: &'static str,
    pub(crate) texture_format: &'static str,
    pub(crate) max_texture_dimension_2d: u32,
    pub(crate) max_storage_textures_per_shader_stage: u32,
    pub(crate) software_adapter_reason: Option<&'static str>,
    pub(crate) servo_gpu_import_backend_compatible: bool,
    pub(crate) servo_gpu_import_backend_reason: Option<&'static str>,
    pub(crate) linux_servo_gpu_import_backend_compatible: bool,
    pub(crate) linux_servo_gpu_import_backend_reason: Option<&'static str>,
}

pub(crate) fn probe_render_device(render_device: &GpuRenderDevice) -> Result<GpuCompositorProbe> {
    render_device.require_texture_usage(
        COMPOSITOR_TEXTURE_FORMAT,
        wgpu::TextureUsages::STORAGE_BINDING,
    )?;

    let info = render_device.info();
    let servo_gpu_import_backend_compatible = info.servo_gpu_import_backend_compatible();
    let servo_gpu_import_backend_reason = info.servo_gpu_import_backend_reason();
    let linux_servo_gpu_import_backend_compatible =
        info.linux_servo_gpu_import_backend_compatible();
    let linux_servo_gpu_import_backend_reason = info.linux_servo_gpu_import_backend_reason();
    let software_adapter_reason = info.software_adapter_reason();
    Ok(GpuCompositorProbe {
        adapter_name: info.adapter_name,
        adapter_device_type: device_type_name(info.adapter_device_type),
        backend: backend_name(info.backend),
        texture_format: texture_format_name(COMPOSITOR_TEXTURE_FORMAT),
        max_texture_dimension_2d: info.max_texture_dimension_2d,
        max_storage_textures_per_shader_stage: info.max_storage_textures_per_shader_stage,
        software_adapter_reason,
        servo_gpu_import_backend_compatible,
        servo_gpu_import_backend_reason,
        linux_servo_gpu_import_backend_compatible,
        linux_servo_gpu_import_backend_reason,
    })
}

pub(super) fn servo_import_backend_preference() -> GpuBackendPreference {
    #[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
    {
        if matches!(
            hypercolor_core::effect::servo_gpu_import_mode(),
            hypercolor_types::config::ServoGpuImportMode::On
        ) {
            return GpuBackendPreference::VulkanRequiredForServoImport;
        }
    }

    GpuBackendPreference::Default
}
