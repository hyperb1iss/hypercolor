use hypercolor_types::layer::BlendMode;
use hypercolor_types::spatial::EdgeBehavior;

use super::super::super::DisplayFinalizeParams;
use super::super::{DISPLAY_FINALIZE_PARAM_BYTES, GpuCompositorPipeline};
use super::DisplayYuv420Layout;
use crate::render_thread::producer_queue::ProducerFrame;

pub(super) fn create_display_finalize_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    scene: &wgpu::TextureView,
    face: &wgpu::TextureView,
    output: &wgpu::TextureView,
    output_yuv: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SparkleFlinger GPU display finalize bind group"),
        layout: &pipeline.display_finalize_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(face),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.display_finalize_params.binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: output_yuv.as_entire_binding(),
            },
        ],
    })
}

pub(super) fn encode_display_finalize_params(
    params: &DisplayFinalizeParams,
    scene: &ProducerFrame,
    face: &ProducerFrame,
) -> [u8; DISPLAY_FINALIZE_PARAM_BYTES] {
    let mut bytes = [0u8; DISPLAY_FINALIZE_PARAM_BYTES];
    let circular = u32::from(params.circular);
    let yuv_layout = DisplayYuv420Layout::new(params.width, params.height);
    bytes[0..4].copy_from_slice(&params.width.to_le_bytes());
    bytes[4..8].copy_from_slice(&params.height.to_le_bytes());
    bytes[8..12].copy_from_slice(&circular.to_le_bytes());
    bytes[12..16].copy_from_slice(&(display_finalize_mode(params.blend_mode) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&scene.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&scene.height().to_le_bytes());
    bytes[24..28].copy_from_slice(&face.width().to_le_bytes());
    bytes[28..32].copy_from_slice(&face.height().to_le_bytes());
    bytes[32..36].copy_from_slice(&display_brightness_factor(params.brightness).to_le_bytes());
    bytes[36..40]
        .copy_from_slice(&display_edge_behavior(params.viewport_edge_behavior).to_le_bytes());
    bytes[48..52].copy_from_slice(&params.viewport_position.x.to_le_bytes());
    bytes[52..56].copy_from_slice(&params.viewport_position.y.to_le_bytes());
    bytes[56..60].copy_from_slice(&params.viewport_size.x.to_le_bytes());
    bytes[60..64].copy_from_slice(&params.viewport_size.y.to_le_bytes());
    bytes[64..68].copy_from_slice(&params.viewport_rotation.cos().to_le_bytes());
    bytes[68..72].copy_from_slice(&params.viewport_rotation.sin().to_le_bytes());
    bytes[72..76].copy_from_slice(&params.viewport_scale.to_le_bytes());
    bytes[76..80].copy_from_slice(&params.opacity.clamp(0.0, 1.0).to_le_bytes());
    bytes[80..84].copy_from_slice(&yuv_layout.y_stride.to_le_bytes());
    bytes[84..88].copy_from_slice(&yuv_layout.uv_stride.to_le_bytes());
    bytes[88..92].copy_from_slice(&yuv_layout.y_plane_len.to_le_bytes());
    bytes[92..96].copy_from_slice(&yuv_layout.u_plane_len.to_le_bytes());
    bytes[40..44]
        .copy_from_slice(&display_fade_falloff(params.viewport_edge_behavior).to_le_bytes());
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum DisplayFinalizeShaderMode {
    Replace = 0,
    Alpha = 1,
    Tint = 2,
    LumaReveal = 3,
    Add = 4,
    Screen = 5,
    Multiply = 6,
    Overlay = 7,
    SoftLight = 8,
    ColorDodge = 9,
    Difference = 10,
}

fn display_finalize_mode(mode: BlendMode) -> DisplayFinalizeShaderMode {
    match mode {
        BlendMode::Replace => DisplayFinalizeShaderMode::Replace,
        BlendMode::Alpha => DisplayFinalizeShaderMode::Alpha,
        BlendMode::Tint => DisplayFinalizeShaderMode::Tint,
        BlendMode::LumaReveal => DisplayFinalizeShaderMode::LumaReveal,
        BlendMode::Add => DisplayFinalizeShaderMode::Add,
        BlendMode::Screen => DisplayFinalizeShaderMode::Screen,
        BlendMode::Multiply => DisplayFinalizeShaderMode::Multiply,
        BlendMode::Overlay => DisplayFinalizeShaderMode::Overlay,
        BlendMode::SoftLight => DisplayFinalizeShaderMode::SoftLight,
        BlendMode::ColorDodge => DisplayFinalizeShaderMode::ColorDodge,
        BlendMode::Difference => DisplayFinalizeShaderMode::Difference,
    }
}

fn display_edge_behavior(edge_behavior: EdgeBehavior) -> u32 {
    match edge_behavior {
        EdgeBehavior::Clamp => 0,
        EdgeBehavior::Wrap => 1,
        EdgeBehavior::Mirror => 2,
        EdgeBehavior::FadeToBlack { .. } => 3,
    }
}

fn display_fade_falloff(edge_behavior: EdgeBehavior) -> f32 {
    match edge_behavior {
        EdgeBehavior::FadeToBlack { falloff } => falloff,
        EdgeBehavior::Clamp | EdgeBehavior::Wrap | EdgeBehavior::Mirror => 0.0,
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper mirrors display byte brightness policy before encoding GPU uniforms"
)]
fn display_brightness_factor(brightness: f32) -> u32 {
    let value = brightness.clamp(0.0, 1.0);
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 1.0 {
        return u32::from(u8::MAX);
    }
    (value.mul_add(f32::from(u8::MAX), 0.5)) as u32
}
