use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};

use super::super::{ComposedFrameSet, PreviewSurfaceRequest};
use super::frame_set::{gpu_composed_with_preview_surface, gpu_composed_without_surfaces};
use super::readback::{
    CachedReadbackKey, CachedReadbackSurface, copy_mapped_readback_buffer_into_surface,
};
use super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuCompositorOutputSurface,
    GpuCompositorPipeline, GpuSparkleFlinger, MAX_CACHED_PREVIEW_SURFACES,
    PREVIEW_SCALE_PARAM_BYTES, texture_extent,
};
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
    pub(super) fn cached_preview_surface(
        &self,
        key: &CachedPreviewSurfaceKey,
    ) -> Option<PublishedSurface> {
        self.cached_preview_surfaces
            .iter()
            .find(|cached| &cached.key == key)
            .map(|cached| cached.surface.clone())
    }

    fn store_cached_preview_surface(
        &mut self,
        key: CachedPreviewSurfaceKey,
        surface: PublishedSurface,
    ) {
        if let Some(index) = self
            .cached_preview_surfaces
            .iter()
            .position(|cached| cached.key == key)
        {
            self.cached_preview_surfaces.remove(index);
        }
        self.cached_preview_surfaces
            .insert(0, CachedPreviewSurface { key, surface });
        if self.cached_preview_surfaces.len() > MAX_CACHED_PREVIEW_SURFACES {
            self.cached_preview_surfaces
                .truncate(MAX_CACHED_PREVIEW_SURFACES);
        }
    }

    pub(crate) fn resolve_preview_surface(&mut self) -> Result<Option<PublishedSurface>> {
        self.submit_pending_preview_work()?;

        if self.pending_preview_map.is_some() {
            if let Some(surface) = self.try_finish_pending_preview_map()? {
                return Ok(Some(surface));
            }
            return Ok(None);
        }

        if self.pending_preview_readback().is_some() {
            let Some(submission_index) = self.pending_preview_submission() else {
                return Ok(None);
            };
            if !self.preview_submission_ready(submission_index.clone())? {
                return Ok(None);
            }
            let mut frame = self
                .frame_in_flight
                .take()
                .expect("submitted preview frame should remain staged until mapping begins");
            let Some(pending_preview_readback) = frame.take_preview_readback() else {
                return Ok(None);
            };
            if self.pending_preview_map.is_some() {
                self.discard_pending_preview_map();
            }
            self.begin_pending_preview_map(pending_preview_readback, Some(submission_index))?;
            return self.try_finish_pending_preview_map();
        }

        if let Some(surface) = self.ready_preview_surface.take() {
            return Ok(Some(surface));
        }
        self.try_finish_pending_preview_map()
    }

    pub(crate) fn submit_pending_preview_work(&mut self) -> Result<()> {
        if self.pending_preview_readback().is_none() {
            return Ok(());
        }
        let (frame_in_flight, queue) = (&mut self.frame_in_flight, &self.queue);
        let submitted = frame_in_flight
            .as_mut()
            .and_then(|frame| frame.submit(queue))
            .is_some();
        if submitted {
            self.clear_pending_upload_buffers();
            self.release_retired_uniform_slots();
        }
        if self.pending_preview_map.is_some() {
            return Ok(());
        }
        let mut frame = self
            .frame_in_flight
            .take()
            .expect("pending preview readback should have a frame owner");
        let submission_index = frame
            .submission_index()
            .context("pending preview frame should be submitted before mapping")?;
        let pending_preview_readback = frame
            .take_preview_readback()
            .expect("pending preview readback should exist before GPU preview submit");
        self.begin_pending_preview_map(pending_preview_readback, Some(submission_index))?;
        Ok(())
    }

    pub(super) fn clear_superseded_preview_outputs(&mut self) {
        drop(self.supersede_frame_in_flight("preview outputs superseded"));
        self.ready_preview_surface = None;
        self.clear_pending_upload_buffers();
    }

    pub(super) fn discard_superseded_preview_work(&mut self) {
        self.clear_superseded_preview_outputs();
        self.discard_pending_preview_map();
    }

    pub(crate) fn discard_preview_work(&mut self) {
        self.discard_superseded_preview_work();
    }

    fn preview_submission_ready(
        &mut self,
        submission_index: wgpu::SubmissionIndex,
    ) -> Result<bool> {
        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_resolve_once) {
            return Ok(false);
        }

        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: Some(Duration::ZERO),
        }) {
            Ok(_) => Ok(true),
            Err(wgpu::PollError::Timeout) => Ok(false),
            Err(error) => Err(error).context("GPU preview readiness poll failed"),
        }
    }

    #[cfg(test)]
    pub(super) fn defer_next_preview_map_resolve(&mut self) {
        self.defer_preview_map_resolve_once = true;
    }

    pub(super) fn begin_pending_preview_map(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
        submission_index: Option<wgpu::SubmissionIndex>,
    ) -> Result<()> {
        let PendingPreviewReadback::PreviewBuffer { request, slot, .. } = &pending_preview_readback;
        let preview_surfaces = self
            .preview_surfaces
            .as_ref()
            .context("GPU scaled preview map requested before preview surfaces existed")?;
        let used_bytes =
            u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height);
        let slice = preview_surfaces.readback(*slot).slice(..used_bytes);
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.pending_preview_map = Some(PendingPreviewMap {
            readback: pending_preview_readback,
            submission_index,
            used_bytes,
            receiver,
        });
        Ok(())
    }

    fn try_finish_pending_preview_map(&mut self) -> Result<Option<PublishedSurface>> {
        let Some(pending_preview_map) = self.pending_preview_map.as_ref() else {
            return Ok(None);
        };

        let poll_result =
            if let Some(submission_index) = pending_preview_map.submission_index.clone() {
                self.device.poll(wgpu::PollType::Wait {
                    submission_index: Some(submission_index),
                    timeout: Some(Duration::from_millis(1)),
                })
            } else {
                self.device.poll(wgpu::PollType::Poll)
            };
        match poll_result {
            Ok(_) | Err(wgpu::PollError::Timeout) => {}
            Err(error) => return Err(error).context("GPU preview map poll failed"),
        }

        #[cfg(test)]
        if std::mem::take(&mut self.defer_preview_map_resolve_once) {
            return Ok(None);
        }

        match pending_preview_map.receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.discard_pending_preview_map();
                return Err(error).context("GPU preview buffer mapping failed");
            }
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.discard_pending_preview_map();
                anyhow::bail!("GPU preview channel closed before map completion");
            }
        }

        let pending_preview_map = self
            .pending_preview_map
            .take()
            .expect("GPU preview map should remain pending until completion");
        self.finish_mapped_preview_surface(
            pending_preview_map.readback,
            pending_preview_map.used_bytes,
        )
        .map(Some)
    }

    pub(super) fn has_pending_or_ready_preview_for(&self, request: PreviewSurfaceRequest) -> bool {
        self.ready_preview_surface.as_ref().is_some_and(|surface| {
            surface.width() == request.width && surface.height() == request.height
        }) || self
            .pending_preview_readback()
            .is_some_and(|pending| pending.matches_request(request))
            || self
                .pending_preview_map
                .as_ref()
                .is_some_and(|pending| pending.readback.matches_request(request))
    }

    pub(super) fn discard_pending_preview_map(&mut self) {
        let Some(pending_preview_map) = self.pending_preview_map.take() else {
            return;
        };

        let PendingPreviewReadback::PreviewBuffer { slot, .. } = pending_preview_map.readback;
        if let Some(preview_surfaces) = self.preview_surfaces.as_ref() {
            preview_surfaces.readback(slot).unmap();
        }
    }

    fn ensure_preview_surface_size(&mut self, width: u32, height: u32) -> Result<()> {
        if self
            .preview_surfaces
            .as_ref()
            .is_some_and(|preview_surfaces| {
                preview_surfaces.width == width && preview_surfaces.height == height
            })
        {
            return Ok(());
        }
        if self.preview_surfaces.is_some() {
            self.discard_pending_preview_map();
        }
        if let Some(preview_surfaces) = self.preview_surfaces.as_mut()
            && preview_surfaces.fits_request(width, height)
        {
            preview_surfaces.reconfigure(width, height);
            return Ok(());
        }

        let (front_view, back_view) = {
            let surfaces = self.surfaces.as_ref().context(
                "GPU preview surfaces requested before compositor surfaces were allocated",
            )?;
            (surfaces.front.view.clone(), surfaces.back.view.clone())
        };

        self.preview_surfaces = Some(GpuPreviewSurfaceSet::new(
            &self.device,
            &self.pipeline,
            &front_view,
            &back_view,
            width,
            height,
        ));
        #[cfg(test)]
        {
            self.preview_surface_allocation_count =
                self.preview_surface_allocation_count.saturating_add(1);
        }
        Ok(())
    }

    pub(super) fn stage_preview_surface_readback(
        &mut self,
        current_output: GpuCompositorOutputSurface,
        source_width: u32,
        source_height: u32,
        readback_key: Option<CachedReadbackKey>,
        request: PreviewSurfaceRequest,
        cache_as_full_size: bool,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        if !cache_as_full_size
            && let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                composition: key.clone(),
                request,
            })
        {
            if let Some(encoder) = encoder {
                self.stage_frame_in_flight(encoder, None);
            }
            drop(self.supersede_frame_in_flight("cached preview served instead"));
            self.clear_pending_upload_buffers();
            return Ok(gpu_composed_with_preview_surface(cached));
        }

        let request_bytes_per_row = request.width.saturating_mul(BYTES_PER_PIXEL as u32);
        let direct_source_texture = if request.width == source_width
            && request.height == source_height
            && request_bytes_per_row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        {
            let surfaces = self
                .surfaces
                .as_ref()
                .context("GPU preview readback requested before compositor surfaces existed")?;
            Some(match current_output {
                GpuCompositorOutputSurface::Front => surfaces.front.texture.clone(),
                GpuCompositorOutputSurface::Back => surfaces.back.texture.clone(),
            })
        } else {
            None
        };

        self.ensure_preview_surface_size(request.width, request.height)?;
        let mapped_readback_slot = self
            .pending_preview_map
            .as_ref()
            .map(|pending| match &pending.readback {
                PendingPreviewReadback::PreviewBuffer { slot, .. } => *slot,
            });
        let preview_surfaces = self
            .preview_surfaces
            .as_mut()
            .expect("preview surfaces should exist after allocation");
        let readback_slot = preview_surfaces.select_readback_slot(mapped_readback_slot);
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU preview scale"),
                })
        });
        if let Some(source_texture) = direct_source_texture {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &source_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: preview_surfaces.readback(readback_slot),
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(preview_surfaces.padded_bytes_per_row),
                        rows_per_image: Some(request.height),
                    },
                },
                texture_extent(request.width, request.height),
            );
        } else {
            let params = encode_preview_scale_params(
                source_width,
                source_height,
                request.width,
                request.height,
            );
            let params_offset = if preview_surfaces.cached_scale_params == Some(params)
                && let Some(offset) = preview_surfaces.cached_scale_params_offset
            {
                self.pipeline.preview_scale_params.pin_last_slot();
                offset
            } else {
                let pending_upload_buffers = &mut self
                    .surfaces
                    .as_mut()
                    .expect("compositor surfaces should exist before preview staging")
                    .pending_upload_buffers;
                let write = self.pipeline.preview_scale_params.write(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    pending_upload_buffers,
                    &params,
                );
                preview_surfaces.cached_scale_params = Some(params);
                preview_surfaces.cached_scale_params_offset =
                    write.reusable.then_some(write.offset);
                #[cfg(test)]
                {
                    preview_surfaces.scale_param_write_count =
                        preview_surfaces.scale_param_write_count.saturating_add(1);
                }
                write.offset
            };
            let bind_group = match current_output {
                GpuCompositorOutputSurface::Front => &preview_surfaces.bind_groups.front_to_preview,
                GpuCompositorOutputSurface::Back => &preview_surfaces.bind_groups.back_to_preview,
            };
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU preview scale pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline.preview_scale_pipeline);
            pass.set_bind_group(0, bind_group, &[params_offset]);
            pass.dispatch_workgroups(
                request.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                request.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                1,
            );
            drop(pass);
            encoder.copy_buffer_to_buffer(
                &preview_surfaces.output_buffer,
                0,
                preview_surfaces.readback(readback_slot),
                0,
                u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height),
            );
        }
        self.stage_frame_in_flight(
            encoder,
            Some(PendingPreviewReadback::PreviewBuffer {
                request,
                readback_key,
                cache_as_full_size,
                slot: readback_slot,
            }),
        );
        Ok(gpu_composed_without_surfaces())
    }

    fn finish_mapped_preview_surface(
        &mut self,
        pending_preview_readback: PendingPreviewReadback,
        used_bytes: u64,
    ) -> Result<PublishedSurface> {
        let PendingPreviewReadback::PreviewBuffer {
            request,
            readback_key,
            cache_as_full_size,
            slot,
        } = pending_preview_readback;
        let preview_surfaces = self
            .preview_surfaces
            .as_mut()
            .context("GPU scaled preview finalize requested before preview surfaces existed")?;
        let readback = preview_surfaces.readback(slot).clone();
        let preview_surface = copy_mapped_readback_buffer_into_surface(
            &readback,
            used_bytes,
            request.width,
            request.height,
            preview_surfaces.padded_bytes_per_row,
            &mut preview_surfaces.readback_surfaces,
            #[cfg(test)]
            &mut preview_surfaces.last_readback_bytes,
        )?;
        if let Some(key) = readback_key {
            if cache_as_full_size {
                self.cached_readback_surface = Some(CachedReadbackSurface {
                    key: Some(key),
                    surface: preview_surface.clone(),
                });
            } else {
                self.store_cached_preview_surface(
                    CachedPreviewSurfaceKey {
                        composition: key,
                        request,
                    },
                    preview_surface.clone(),
                );
            }
        }
        Ok(preview_surface)
    }
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
            cached_scale_params_offset: None,
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
