use std::sync::mpsc;

use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::types::canvas::{PublishedSurface, RenderSurfacePool};

use super::super::DisplayFinalizeCacheKey;
use super::GpuCompositorTexture;
#[allow(unused_imports)]
pub(super) use super::source::GpuDisplaySourceTexture;

mod readback;
mod runtime;
mod shader;
mod surfaces;

pub(super) const DISPLAY_FINALIZE_READBACK_SLOT_COUNT: usize = 3;

pub(super) struct GpuDisplayFinalizeSurfaceSet {
    pub(super) generation: u64,
    pub(super) padded_bytes_per_row: u32,
    pub(super) yuv_layout: DisplayYuv420Layout,
    pub(super) output: GpuCompositorTexture,
    pub(super) yuv_output: wgpu::Buffer,
    readbacks: [wgpu::Buffer; DISPLAY_FINALIZE_READBACK_SLOT_COUNT],
    yuv_readbacks: [wgpu::Buffer; DISPLAY_FINALIZE_READBACK_SLOT_COUNT],
    readback_slots_in_use: [bool; DISPLAY_FINALIZE_READBACK_SLOT_COUNT],
    next_readback_slot: usize,
    pub(super) readback_surfaces: RenderSurfacePool,
    pub(super) scene_source: Option<GpuDisplaySourceTexture>,
    pub(super) face_source: Option<GpuDisplaySourceTexture>,
    pub(super) pending_upload_buffers: super::PendingUploadBuffers,
    #[cfg(test)]
    pub(super) scene_upload_count: usize,
    #[cfg(test)]
    pub(super) face_upload_count: usize,
    #[cfg(test)]
    pub(super) last_readback_bytes: u64,
    #[cfg(test)]
    pub(super) last_yuv_readback_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisplayYuv420Layout {
    pub(super) y_stride: u32,
    pub(super) uv_stride: u32,
    pub(super) y_plane_len: u32,
    pub(super) u_plane_len: u32,
    pub(super) total_len: u32,
    pub(super) word_len: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GpuDisplayFinalizeFormat {
    Rgba,
    Yuv420,
}

pub(crate) enum GpuDisplayFinalizeFrame {
    Rgba(PublishedSurface),
    Yuv420(DisplayYuv420Frame),
}

pub(crate) enum GpuDisplayFinalizeDispatch {
    Unsupported,
    Saturated,
    Pending(PendingGpuDisplayFinalize),
}

pub(crate) struct PendingGpuDisplayFinalize {
    pub(super) cache_key: DisplayFinalizeCacheKey,
    pub(super) surface_generation: u64,
    pub(super) format: GpuDisplayFinalizeFormat,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) padded_bytes_per_row: u32,
    pub(super) yuv_layout: DisplayYuv420Layout,
    pub(super) used_bytes: u64,
    pub(super) mapped_bytes: u64,
    pub(super) submission_index: wgpu::SubmissionIndex,
    pub(super) buffer: wgpu::Buffer,
    receiver: Option<mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>>,
    map_ready: bool,
    pub(super) slot: usize,
}
