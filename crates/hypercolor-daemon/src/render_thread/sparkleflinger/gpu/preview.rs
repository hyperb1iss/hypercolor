use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};

use super::super::PreviewSurfaceRequest;
use super::{CachedReadbackKey, GpuCompositorPipeline, PREVIEW_SCALE_PARAM_BYTES};
use crate::render_thread::producer_queue::ProducerFrame;

const MAX_CACHED_PREVIEW_READBACK_POOLS: usize = 3;
const PREVIEW_READBACK_SLOT_COUNT: usize = 2;

pub(super) struct GpuPreviewSurfaceSet {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) capacity_width: u32,
    pub(super) capacity_height: u32,
    pub(super) padded_bytes_per_row: u32,
    pub(super) output_buffer: wgpu::Buffer,
    readbacks: [wgpu::Buffer; PREVIEW_READBACK_SLOT_COUNT],
    next_readback_slot: usize,
    pub(super) bind_groups: GpuPreviewScaleBindGroups,
    pub(super) readback_surfaces: RenderSurfacePool,
    cached_readback_surfaces: Vec<CachedPreviewReadbackSurfaces>,
    pub(super) cached_scale_params: Option<[u8; PREVIEW_SCALE_PARAM_BYTES]>,
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

impl GpuPreviewSurfaceSet {
    pub(super) fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Self {
        let padded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU preview output"),
            size: u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(BYTES_PER_PIXEL as u64),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = std::array::from_fn(|slot| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(match slot {
                    0 => "SparkleFlinger GPU preview readback A",
                    1 => "SparkleFlinger GPU preview readback B",
                    _ => "SparkleFlinger GPU preview readback",
                }),
                size: u64::from(width)
                    .saturating_mul(u64::from(height))
                    .saturating_mul(BYTES_PER_PIXEL as u64),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        Self {
            width,
            height,
            capacity_width: width,
            capacity_height: height,
            padded_bytes_per_row,
            readbacks,
            next_readback_slot: 0,
            bind_groups: GpuPreviewScaleBindGroups::new(
                device,
                pipeline,
                front_view,
                back_view,
                &output_buffer,
            ),
            output_buffer,
            readback_surfaces: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height)),
            cached_readback_surfaces: Vec::with_capacity(MAX_CACHED_PREVIEW_READBACK_POOLS),
            cached_scale_params: None,
            #[cfg(test)]
            scale_param_write_count: 0,
            #[cfg(test)]
            preview_bind_group_count: 2,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            readback_surface_pool_allocation_count: 1,
        }
    }

    pub(super) fn fits_request(&self, width: u32, height: u32) -> bool {
        width <= self.capacity_width && height <= self.capacity_height
    }

    pub(super) fn reconfigure(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        let next_request = PreviewSurfaceRequest { width, height };
        let next_surfaces = self
            .take_cached_readback_surfaces(next_request)
            .unwrap_or_else(|| {
                #[cfg(test)]
                {
                    self.readback_surface_pool_allocation_count = self
                        .readback_surface_pool_allocation_count
                        .saturating_add(1);
                }
                RenderSurfacePool::new(SurfaceDescriptor::rgba8888(width, height))
            });
        let current_request = PreviewSurfaceRequest {
            width: self.width,
            height: self.height,
        };
        let current_surfaces = std::mem::replace(&mut self.readback_surfaces, next_surfaces);
        self.store_cached_readback_surfaces(current_request, current_surfaces);
        self.width = width;
        self.height = height;
        self.padded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
    }

    pub(super) fn select_readback_slot(&mut self, mapped_slot: Option<usize>) -> usize {
        for offset in 0..PREVIEW_READBACK_SLOT_COUNT {
            let slot = (self.next_readback_slot + offset) % PREVIEW_READBACK_SLOT_COUNT;
            if Some(slot) != mapped_slot {
                self.next_readback_slot = (slot + 1) % PREVIEW_READBACK_SLOT_COUNT;
                return slot;
            }
        }
        mapped_slot
            .map(|slot| (slot + 1) % PREVIEW_READBACK_SLOT_COUNT)
            .unwrap_or(0)
    }

    pub(super) fn readback(&self, slot: usize) -> &wgpu::Buffer {
        &self.readbacks[slot]
    }

    fn take_cached_readback_surfaces(
        &mut self,
        request: PreviewSurfaceRequest,
    ) -> Option<RenderSurfacePool> {
        self.cached_readback_surfaces
            .iter()
            .position(|cached| cached.request == request)
            .map(|index| self.cached_readback_surfaces.remove(index).surfaces)
    }

    fn store_cached_readback_surfaces(
        &mut self,
        request: PreviewSurfaceRequest,
        surfaces: RenderSurfacePool,
    ) {
        if let Some(index) = self
            .cached_readback_surfaces
            .iter()
            .position(|cached| cached.request == request)
        {
            self.cached_readback_surfaces.remove(index);
        }
        self.cached_readback_surfaces
            .insert(0, CachedPreviewReadbackSurfaces { request, surfaces });
        if self.cached_readback_surfaces.len() > MAX_CACHED_PREVIEW_READBACK_POOLS {
            self.cached_readback_surfaces
                .truncate(MAX_CACHED_PREVIEW_READBACK_POOLS);
        }
    }
}

impl GpuPreviewScaleBindGroups {
    fn new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        preview_buffer: &wgpu::Buffer,
    ) -> Self {
        Self {
            front_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                front_view,
                preview_buffer,
                "SparkleFlinger GPU preview scale bind group front->preview",
            ),
            back_to_preview: create_preview_scale_bind_group(
                device,
                pipeline,
                back_view,
                preview_buffer,
                "SparkleFlinger GPU preview scale bind group back->preview",
            ),
        }
    }
}

pub(super) fn bypass_preview_surface(frame: &ProducerFrame) -> Option<PublishedSurface> {
    match frame {
        ProducerFrame::Surface(surface) => Some(surface.clone()),
        ProducerFrame::Canvas(_) => None,
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
        ProducerFrame::GpuTexture(_) => None,
    }
}

pub(super) fn preview_request_matches_plan(
    request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
) -> bool {
    request.is_none_or(|request| request.width == width && request.height == height)
}

fn create_preview_scale_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    source: &wgpu::TextureView,
    output: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.preview_scale_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pipeline.preview_scale_params_buffer.as_entire_binding(),
            },
        ],
    })
}

pub(super) fn encode_preview_scale_params(
    source_width: u32,
    source_height: u32,
    preview_width: u32,
    preview_height: u32,
) -> [u8; PREVIEW_SCALE_PARAM_BYTES] {
    let mut bytes = [0u8; PREVIEW_SCALE_PARAM_BYTES];
    bytes[0..4].copy_from_slice(&source_width.to_le_bytes());
    bytes[4..8].copy_from_slice(&source_height.to_le_bytes());
    bytes[8..12].copy_from_slice(&preview_width.to_le_bytes());
    bytes[12..16].copy_from_slice(&preview_height.to_le_bytes());
    bytes
}
