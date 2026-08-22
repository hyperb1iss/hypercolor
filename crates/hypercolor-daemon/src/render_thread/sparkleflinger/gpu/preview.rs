use hypercolor_core::types::canvas::{PublishedSurface, RenderSurfacePool};

use super::super::PreviewSurfaceRequest;
use super::readback::CachedReadbackKey;
use super::{GpuSparkleFlinger, MAX_CACHED_PREVIEW_SURFACES, PREVIEW_SCALE_PARAM_BYTES};

mod preparation;
mod readback_runtime;
mod runtime;
mod surfaces;

use surfaces::encode_preview_scale_params;
pub(super) use surfaces::{
    bypass_preview_surface, preview_request_matches_plan, preview_requires_scale,
};

const MAX_CACHED_PREVIEW_READBACK_POOLS: usize = 3;
const PREVIEW_READBACK_SLOT_COUNT: usize = 2;

pub(super) struct GpuPreviewSurfaceSet {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) capacity_width: u32,
    pub(super) capacity_height: u32,
    pub(super) padded_bytes_per_row: u32,
    scale_output: Option<GpuPreviewScaleOutput>,
    readbacks: [wgpu::Buffer; PREVIEW_READBACK_SLOT_COUNT],
    next_readback_slot: usize,
    pub(super) readback_surfaces: RenderSurfacePool,
    cached_readback_surfaces: Vec<CachedPreviewReadbackSurfaces>,
    pub(super) cached_scale_params: Option<[u8; PREVIEW_SCALE_PARAM_BYTES]>,
    cached_scale_params_offset: Option<u32>,
    #[cfg(test)]
    pub(super) scale_param_write_count: usize,
    #[cfg(test)]
    pub(super) preview_bind_group_count: usize,
    #[cfg(test)]
    pub(super) last_readback_bytes: u64,
    #[cfg(test)]
    pub(super) readback_surface_pool_allocation_count: usize,
}

pub(super) struct GpuPreviewScaleBindGroups {
    pub(super) front_to_preview: wgpu::BindGroup,
    pub(super) back_to_preview: wgpu::BindGroup,
}

struct GpuPreviewScaleOutput {
    buffer: wgpu::Buffer,
    bind_groups: GpuPreviewScaleBindGroups,
}

pub(super) struct PreparedPreviewSurfaceChange(PreparedPreviewSurfaceChangeKind);

enum PreparedPreviewSurfaceChangeKind {
    Unchanged {
        scale_output: Option<GpuPreviewScaleOutput>,
    },
    Reconfigure {
        width: u32,
        height: u32,
        readback_surfaces: PreparedPreviewReadbackSurfaces,
        scale_output: Option<GpuPreviewScaleOutput>,
    },
    Replace(GpuPreviewSurfaceSet),
}

enum PreparedPreviewReadbackSurfaces {
    Cached(usize),
    Fresh(RenderSurfacePool),
}

#[derive(Debug, Clone)]
pub(super) struct CachedPreviewSurface {
    pub(super) key: CachedPreviewSurfaceKey,
    pub(super) surface: PublishedSurface,
}

struct CachedPreviewReadbackSurfaces {
    request: PreviewSurfaceRequest,
    surfaces: RenderSurfacePool,
}

#[derive(Debug, Clone)]
pub(super) enum PendingPreviewReadback {
    PreviewBuffer {
        request: PreviewSurfaceRequest,
        readback_key: Option<CachedReadbackKey>,
        cache_as_full_size: bool,
        slot: usize,
    },
}

pub(super) struct PendingPreviewMap {
    pub(super) readback: PendingPreviewReadback,
    pub(super) submission_index: Option<wgpu::SubmissionIndex>,
    pub(super) used_bytes: u64,
    pub(super) receiver: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

impl PendingPreviewReadback {
    pub(super) fn matches_request(&self, request: PreviewSurfaceRequest) -> bool {
        matches!(
            self,
            Self::PreviewBuffer {
                request: pending_request,
                ..
            } if *pending_request == request
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CachedPreviewSurfaceKey {
    pub(super) composition: CachedReadbackKey,
    pub(super) request: PreviewSurfaceRequest,
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn discard_pending_preview_map(&mut self) {
        let Some(pending_preview_map) = self.pending_preview_map.take() else {
            return;
        };

        let PendingPreviewReadback::PreviewBuffer { slot, .. } = pending_preview_map.readback;
        if let Some(preview_surfaces) = self.preview_surfaces.as_ref() {
            preview_surfaces.readback(slot).unmap();
        }
    }
}
