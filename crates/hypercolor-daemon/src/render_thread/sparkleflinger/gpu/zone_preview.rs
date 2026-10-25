//! Transport-sized zone previews scaled from GPU zone frames.
//!
//! Each previewed zone owns one lane: a params uniform, a scale output, one
//! readback buffer, and a CPU surface pool, all at the preview size. A lane
//! carries at most one readback in flight and is polled without blocking;
//! its newest finished surface stays latched until a fresher one lands.
//!
//! Lanes keep their own params uniform rather than writing the shared
//! preview-scale ring, because the scene preview reuses a cached offset into
//! that ring across frames and extra writers would eventually overwrite it.

use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_types::canvas::{
    BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool, SurfaceDescriptor,
};
use hypercolor_types::scene::ZoneId;

use super::preview::encode_preview_scale_params;
use super::readback::copy_mapped_readback_buffer_into_surface;
use super::source::{
    CachedGpuSourceCopy, GpuDisplaySourceTexture, gpu_source_frame, prepare_display_source_texture,
};
use super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuCompositorPipeline, GpuSparkleFlinger,
    PREVIEW_SCALE_PARAM_BYTES, PendingUploadBuffers, ensure_readback_buffer_capacity,
    ensure_storage_buffer_capacity, try_create_gpu_resources,
};
use crate::render_thread::producer_queue::{GpuTextureFrameOrigin, ProducerFrame};
use crate::render_thread::sparkleflinger::ZonePreviewStage;

const ZONE_PREVIEW_BIND_GROUP_CACHE_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZonePreviewContentKey {
    source: CachedGpuSourceCopy,
    width: u32,
    height: u32,
}

pub(super) struct GpuZonePreviewLane {
    width: u32,
    height: u32,
    bytes_per_row: u32,
    used_bytes: u64,
    params: wgpu::Buffer,
    written_params: Option<[u8; PREVIEW_SCALE_PARAM_BYTES]>,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    surfaces: RenderSurfacePool,
    source: Option<GpuDisplaySourceTexture>,
    pending_upload_buffers: PendingUploadBuffers,
    bind_groups: Vec<CachedZonePreviewBindGroup>,
    pending: Option<PendingZonePreviewMap>,
    latest: Option<PublishedSurface>,
    latest_content: Option<ZonePreviewContentKey>,
    #[cfg(test)]
    last_readback_bytes: u64,
}

struct CachedZonePreviewBindGroup {
    source_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct PendingZonePreviewMap {
    content: ZonePreviewContentKey,
    submission_index: wgpu::SubmissionIndex,
    receiver: Option<mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>>,
    map_ready: bool,
}

impl GpuSparkleFlinger {
    /// Scale `frame` to `width` x `height` and start reading the result
    /// back for `zone_id`. Never waits on the GPU.
    pub(crate) fn stage_zone_preview(
        &mut self,
        zone_id: ZoneId,
        frame: &ProducerFrame,
        width: u32,
        height: u32,
    ) -> Result<ZonePreviewStage> {
        let Some(source) = gpu_source_frame(frame) else {
            return Ok(ZonePreviewStage::Unsupported);
        };
        if width == 0 || height == 0 || source.width() == 0 || source.height() == 0 {
            return Ok(ZonePreviewStage::Unsupported);
        }
        let content = ZonePreviewContentKey {
            source: source.cached_display_source_copy(),
            width,
            height,
        };
        self.poll_zone_preview_lane(zone_id)?;
        self.ensure_zone_preview_lane(zone_id, width, height)?;
        if let Some(lane) = self.zone_preview_lanes.get(&zone_id) {
            if lane.pending.is_some() {
                return Ok(ZonePreviewStage::Busy);
            }
            if lane.latest_content == Some(content) {
                return Ok(ZonePreviewStage::Current);
            }
        }
        if matches!(
            frame,
            ProducerFrame::GpuTexture(texture)
                if texture.origin == GpuTextureFrameOrigin::CompositorOutput
        ) {
            self.flush_pending_output_submission()?;
        }

        let device = &self.device;
        let queue = &self.queue;
        let pipeline = &mut self.pipeline;
        let lane = self
            .zone_preview_lanes
            .get_mut(&zone_id)
            .context("GPU zone preview lane should exist after allocation")?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SparkleFlinger GPU zone preview"),
        });
        let mut native_screen_leases = Vec::new();
        #[cfg(test)]
        let mut source_upload_count = 0;
        prepare_display_source_texture(
            device,
            queue,
            pipeline,
            &mut encoder,
            &mut lane.source,
            &mut lane.pending_upload_buffers,
            frame,
            Some(&source),
            "SparkleFlinger Zone Preview Source",
            &mut native_screen_leases,
            #[cfg(test)]
            &mut source_upload_count,
        )?;
        let source_view = if source.needs_display_source_copy() {
            lane.source
                .as_ref()
                .map(|copied| copied.texture.view.clone())
                .context("GPU zone preview source copy should exist after preparation")?
        } else {
            if let Some(lease) = source.native_screen_lease() {
                native_screen_leases.push(lease);
            }
            source.view().clone()
        };
        // A native screen texture must not outlive its lease inside a cached
        // bind group, so those frames bind afresh every time.
        let cacheable = source.native_screen_lease().is_none();
        let bind_group = lane.bind_group_for(device, pipeline, &source_view, cacheable);
        let params = encode_preview_scale_params(source.width(), source.height(), width, height);
        if lane.written_params != Some(params) {
            queue.write_buffer(&lane.params, 0, &params);
            lane.written_params = Some(params);
        }
        {
            pipeline.timings.compute(
                &mut encoder,
                crate::render_thread::sparkleflinger::gpu::timing::GpuStage::PreviewScale,
                "SparkleFlinger GPU zone preview scale pass",
                |pass| {
                    pass.set_pipeline(&pipeline.preview_scale_pipeline);
                    pass.set_bind_group(0, &bind_group, &[0]);
                    pass.dispatch_workgroups(
                        width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                        height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                        1,
                    );
                },
            );
        }
        encoder.copy_buffer_to_buffer(&lane.output, 0, &lane.readback, 0, lane.used_bytes);
        let submission_index = queue.submit(Some(encoder.finish()));
        lane.pending_upload_buffers.clear();
        lane.pending = Some(begin_zone_preview_map(
            &lane.readback,
            lane.used_bytes,
            content,
            submission_index.clone(),
        ));
        #[cfg(test)]
        {
            self.zone_preview_stage_count = self.zone_preview_stage_count.saturating_add(1);
        }
        self.retire_native_screen_leases(submission_index, native_screen_leases);
        self.release_retired_uniform_slots();
        Ok(ZonePreviewStage::Staged)
    }

    /// The newest finished preview for `zone_id`, after a non-blocking poll
    /// of its in-flight readback.
    pub(crate) fn zone_preview_surface(
        &mut self,
        zone_id: ZoneId,
    ) -> Result<Option<PublishedSurface>> {
        self.poll_zone_preview_lane(zone_id)?;
        Ok(self
            .zone_preview_lanes
            .get(&zone_id)
            .and_then(|lane| lane.latest.clone()))
    }

    /// Drop every lane whose zone `keep` rejects. Dropping a lane's
    /// readback buffer cancels any map still in flight.
    pub(crate) fn retain_zone_previews(&mut self, mut keep: impl FnMut(ZoneId) -> bool) {
        self.zone_preview_lanes.retain(|zone_id, _| keep(*zone_id));
    }

    #[cfg(test)]
    pub(crate) fn zone_preview_lane_count(&self) -> usize {
        self.zone_preview_lanes.len()
    }

    #[cfg(test)]
    pub(crate) const fn zone_preview_stage_count(&self) -> usize {
        self.zone_preview_stage_count
    }

    fn ensure_zone_preview_lane(&mut self, zone_id: ZoneId, width: u32, height: u32) -> Result<()> {
        if self
            .zone_preview_lanes
            .get(&zone_id)
            .is_some_and(|lane| lane.width == width && lane.height == height)
        {
            return Ok(());
        }
        let mut lane = GpuZonePreviewLane::try_new(
            &self.device,
            self.max_buffer_size,
            self.max_storage_buffer_binding_size,
            width,
            height,
        )?;
        if let Some(mut previous) = self.zone_preview_lanes.remove(&zone_id) {
            // Keep publishing the old size until the new one lands.
            lane.latest = previous.latest.take();
        }
        self.zone_preview_lanes.insert(zone_id, lane);
        Ok(())
    }

    fn poll_zone_preview_lane(&mut self, zone_id: ZoneId) -> Result<()> {
        let device = &self.device;
        let Some(lane) = self.zone_preview_lanes.get_mut(&zone_id) else {
            return Ok(());
        };
        let Some(pending) = lane.pending.as_mut() else {
            return Ok(());
        };
        match poll_zone_preview_map(device, pending) {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(error) => {
                // A failed poll or map leaves the readback buffer's map state
                // unknown, so the lane is rebuilt rather than reused.
                self.zone_preview_lanes.remove(&zone_id);
                return Err(error);
            }
        }
        let content = pending.content;
        lane.pending = None;
        let surface = copy_mapped_readback_buffer_into_surface(
            &lane.readback,
            lane.used_bytes,
            lane.width,
            lane.height,
            lane.bytes_per_row,
            &mut lane.surfaces,
            #[cfg(test)]
            &mut lane.last_readback_bytes,
        )?;
        super::record_gpu_readback(
            super::GpuReadbackKind::Preview,
            u64::from(lane.width) * u64::from(lane.height) * 4,
        );
        lane.latest = Some(surface);
        lane.latest_content = Some(content);
        Ok(())
    }
}

impl GpuZonePreviewLane {
    fn try_new(
        device: &wgpu::Device,
        max_buffer_size: u64,
        max_storage_buffer_binding_size: u64,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let used_bytes = ensure_readback_buffer_capacity(max_buffer_size, width, height, false)?;
        ensure_storage_buffer_capacity(max_storage_buffer_binding_size, width, height)?;
        let bytes_per_row = width
            .checked_mul(u32::try_from(BYTES_PER_PIXEL).context("pixel size should fit u32")?)
            .context("GPU zone preview row byte size overflowed")?;
        let surfaces = RenderSurfacePool::try_new(SurfaceDescriptor::rgba8888(width, height))
            .context("GPU zone preview CPU surface allocation failed")?;
        let (params, output, readback) = try_create_gpu_resources(
            device,
            "GPU zone preview resource allocation failed",
            || {
                let params = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU zone preview params"),
                    size: PREVIEW_SCALE_PARAM_BYTES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let output = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU zone preview output"),
                    size: used_bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU zone preview readback"),
                    size: used_bytes,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                (params, output, readback)
            },
        )?;
        Ok(Self {
            width,
            height,
            bytes_per_row,
            used_bytes,
            params,
            written_params: None,
            output,
            readback,
            surfaces,
            source: None,
            pending_upload_buffers: PendingUploadBuffers::default(),
            bind_groups: Vec::new(),
            pending: None,
            latest: None,
            latest_content: None,
            #[cfg(test)]
            last_readback_bytes: 0,
        })
    }

    fn bind_group_for(
        &mut self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        source_view: &wgpu::TextureView,
        cacheable: bool,
    ) -> wgpu::BindGroup {
        if let Some(cached) = self
            .bind_groups
            .iter()
            .find(|cached| cached.source_view == *source_view)
        {
            return cached.bind_group.clone();
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SparkleFlinger GPU zone preview scale bind group"),
            layout: &pipeline.preview_scale_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        if cacheable {
            if self.bind_groups.len() >= ZONE_PREVIEW_BIND_GROUP_CACHE_CAP {
                self.bind_groups.remove(0);
            }
            self.bind_groups.push(CachedZonePreviewBindGroup {
                source_view: source_view.clone(),
                bind_group: bind_group.clone(),
            });
        }
        bind_group
    }
}

fn begin_zone_preview_map(
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    content: ZonePreviewContentKey,
    submission_index: wgpu::SubmissionIndex,
) -> PendingZonePreviewMap {
    let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    buffer
        .slice(..used_bytes)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    PendingZonePreviewMap {
        content,
        submission_index,
        receiver: Some(receiver),
        map_ready: false,
    }
}

fn poll_zone_preview_map(
    device: &wgpu::Device,
    pending: &mut PendingZonePreviewMap,
) -> Result<bool> {
    if pending.map_ready {
        return Ok(true);
    }
    device
        .poll(wgpu::PollType::Poll)
        .context("GPU zone preview callback poll failed")?;
    if take_zone_preview_map_ready(pending)? {
        return Ok(true);
    }
    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending.submission_index.clone()),
        timeout: Some(Duration::ZERO),
    }) {
        Ok(_) | Err(wgpu::PollError::Timeout) => {}
        Err(error) => return Err(error).context("GPU zone preview readiness poll failed"),
    }
    device
        .poll(wgpu::PollType::Poll)
        .context("GPU zone preview callback poll failed")?;
    take_zone_preview_map_ready(pending)
}

fn take_zone_preview_map_ready(pending: &mut PendingZonePreviewMap) -> Result<bool> {
    let Some(receiver) = pending.receiver.take() else {
        return Ok(pending.map_ready);
    };
    match receiver.try_recv() {
        Ok(Ok(())) => {
            pending.map_ready = true;
            Ok(true)
        }
        Ok(Err(error)) => Err(error).context("GPU zone preview buffer mapping failed"),
        Err(TryRecvError::Disconnected) => {
            anyhow::bail!("GPU zone preview map channel closed before completion")
        }
        Err(TryRecvError::Empty) => {
            pending.receiver = Some(receiver);
            Ok(false)
        }
    }
}
