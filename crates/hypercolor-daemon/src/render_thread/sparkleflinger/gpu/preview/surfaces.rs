use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool, SurfaceDescriptor, SurfaceStateCounts,
};

use super::super::{GpuCompositorPipeline, PREVIEW_SCALE_PARAM_BYTES};
use super::{
    CachedPreviewReadbackSurfaces, GpuPreviewScaleBindGroups, GpuPreviewScaleOutput,
    GpuPreviewSurfaceSet, MAX_CACHED_PREVIEW_READBACK_POOLS, PREVIEW_READBACK_SLOT_COUNT,
    PreparedPreviewReadbackSurfaces,
};
use crate::render_thread::producer_queue::ProducerFrame;
use crate::render_thread::sparkleflinger::PreviewSurfaceRequest;

impl GpuPreviewSurfaceSet {
    pub(in crate::render_thread::sparkleflinger::gpu) fn surface_pool_counts(
        &mut self,
    ) -> SurfaceStateCounts {
        let mut counts = self.readback_surfaces.slot_counts();
        for cached in &mut self.cached_readback_surfaces {
            let cached_counts = cached.surfaces.slot_counts();
            counts.free = counts.free.saturating_add(cached_counts.free);
            counts.dequeued = counts.dequeued.saturating_add(cached_counts.dequeued);
            counts.published = counts.published.saturating_add(cached_counts.published);
        }
        counts
    }

    pub(super) fn try_new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        let padded_bytes_per_row = width
            .checked_mul(BYTES_PER_PIXEL as u32)
            .context("GPU preview row byte size overflowed")?;
        let size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("GPU preview readback byte size overflowed")?;
        let readback_surfaces =
            RenderSurfacePool::try_new(SurfaceDescriptor::rgba8888(width, height))
                .context("GPU preview CPU surface allocation failed")?;
        let readbacks = super::super::try_create_gpu_resources(
            device,
            "GPU preview readback buffer allocation failed",
            || {
                std::array::from_fn(|slot| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(match slot {
                            0 => "SparkleFlinger GPU preview readback A",
                            1 => "SparkleFlinger GPU preview readback B",
                            _ => "SparkleFlinger GPU preview readback",
                        }),
                        size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    })
                })
            },
        )?;
        let mut cached_readback_surfaces = Vec::new();
        cached_readback_surfaces
            .try_reserve_exact(MAX_CACHED_PREVIEW_READBACK_POOLS)
            .context("GPU preview readback cache allocation failed")?;
        Ok(Self {
            width,
            height,
            capacity_width: width,
            capacity_height: height,
            padded_bytes_per_row,
            scale_output: None,
            readbacks,
            next_readback_slot: 0,
            readback_surfaces,
            cached_readback_surfaces,
            cached_scale_params: None,
            cached_scale_params_offset: None,
            #[cfg(test)]
            scale_param_write_count: 0,
            #[cfg(test)]
            preview_bind_group_count: 0,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            readback_surface_pool_allocation_count: 1,
        })
    }

    pub(super) fn fits_request(&self, width: u32, height: u32) -> bool {
        width <= self.capacity_width && height <= self.capacity_height
    }

    pub(super) fn prepare_reconfiguration(
        &self,
        width: u32,
        height: u32,
    ) -> Result<PreparedPreviewReadbackSurfaces> {
        let next_request = PreviewSurfaceRequest { width, height };
        if let Some(index) = self
            .cached_readback_surfaces
            .iter()
            .position(|cached| cached.request == next_request)
        {
            return Ok(PreparedPreviewReadbackSurfaces::Cached(index));
        }
        RenderSurfacePool::try_new(SurfaceDescriptor::rgba8888(width, height))
            .map(PreparedPreviewReadbackSurfaces::Fresh)
            .context("GPU preview CPU surface reconfiguration failed")
    }

    pub(super) fn commit_reconfiguration(
        &mut self,
        width: u32,
        height: u32,
        prepared: PreparedPreviewReadbackSurfaces,
    ) {
        let next_surfaces = match prepared {
            PreparedPreviewReadbackSurfaces::Cached(index) => {
                self.cached_readback_surfaces.remove(index).surfaces
            }
            PreparedPreviewReadbackSurfaces::Fresh(surfaces) => {
                #[cfg(test)]
                {
                    self.readback_surface_pool_allocation_count = self
                        .readback_surface_pool_allocation_count
                        .saturating_add(1);
                }
                surfaces
            }
        };
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

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn has_scale_output(&self) -> bool {
        self.scale_output.is_some()
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
        if self.cached_readback_surfaces.len() >= MAX_CACHED_PREVIEW_READBACK_POOLS {
            self.cached_readback_surfaces.pop();
        }
        self.cached_readback_surfaces
            .insert(0, CachedPreviewReadbackSurfaces { request, surfaces });
    }
}

impl GpuPreviewScaleOutput {
    pub(super) fn try_new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        capacity_width: u32,
        capacity_height: u32,
        max_storage_buffer_binding_size: u64,
        fail_after_prepare: bool,
    ) -> Result<Self> {
        let size = super::super::ensure_storage_buffer_capacity(
            max_storage_buffer_binding_size,
            capacity_width,
            capacity_height,
        )?;
        let prepared = super::super::try_create_gpu_resources(
            device,
            "GPU preview scale resource allocation failed",
            || {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU preview output"),
                    size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let bind_groups = GpuPreviewScaleBindGroups::new(
                    device, pipeline, front_view, back_view, &buffer,
                );
                Self {
                    buffer,
                    bind_groups,
                }
            },
        )?;
        super::super::reject_injected_gpu_preparation(fail_after_prepare, "preview scale output")?;
        Ok(prepared)
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

pub(in crate::render_thread::sparkleflinger::gpu) fn bypass_preview_surface(
    frame: &ProducerFrame,
) -> Option<PublishedSurface> {
    match frame {
        ProducerFrame::Surface(surface) => Some(surface.clone()),
        ProducerFrame::Canvas(_) | ProducerFrame::ScreenPublication(_) => None,
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
        ProducerFrame::GpuTexture(_) => None,
    }
}

pub(in crate::render_thread::sparkleflinger::gpu) fn preview_request_matches_plan(
    request: Option<PreviewSurfaceRequest>,
    width: u32,
    height: u32,
) -> bool {
    request.is_none_or(|request| request.width == width && request.height == height)
}

pub(in crate::render_thread::sparkleflinger::gpu) fn preview_requires_scale(
    request: PreviewSurfaceRequest,
    source_width: u32,
    source_height: u32,
) -> bool {
    request.width != source_width
        || request.height != source_height
        || !request
            .width
            .saturating_mul(BYTES_PER_PIXEL as u32)
            .is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
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
                resource: pipeline.preview_scale_params.binding(),
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
