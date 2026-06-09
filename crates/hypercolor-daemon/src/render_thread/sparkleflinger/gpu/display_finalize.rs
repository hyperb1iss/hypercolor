use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

use anyhow::{Context, Result};
use hypercolor_core::bus::DisplayYuv420Frame;
use hypercolor_core::types::canvas::{PublishedSurface, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::scene::DisplayFaceBlendMode;
use hypercolor_types::scene::ZoneId;
use hypercolor_types::spatial::EdgeBehavior;

use super::super::{DisplayFinalizeCacheKey, DisplayFinalizeParams};
use super::readback::copy_mapped_readback_buffer_into_surface;
use super::source::{GpuSourceFrame, gpu_source_frame, prepare_display_source_texture};
#[cfg(test)]
use super::telemetry::record_gpu_display_finalize_blocking_wait;
use super::telemetry::{
    record_gpu_display_finalize_attempt, record_gpu_display_finalize_result,
    record_gpu_display_finalize_surface_realloc,
};
use super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, DISPLAY_FINALIZE_PARAM_BYTES,
    GpuCompositorPipeline, GpuCompositorTexture, GpuSparkleFlinger, texture_extent,
};
use crate::render_thread::producer_queue::ProducerFrame;

pub(super) const DISPLAY_FINALIZE_READBACK_SLOT_COUNT: usize = 3;
#[cfg(test)]
const GPU_READBACK_WAIT_TIMEOUT: Duration = Duration::from_millis(8);

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

pub(super) struct GpuDisplaySourceTexture {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) texture: GpuCompositorTexture,
    pub(super) cached_upload: Option<super::CachedSourceUpload>,
    pub(super) cached_gpu_copy: Option<super::CachedGpuSourceCopy>,
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

impl PendingGpuDisplayFinalize {
    pub(super) fn new(
        cache_key: DisplayFinalizeCacheKey,
        surface_generation: u64,
        format: GpuDisplayFinalizeFormat,
        width: u32,
        height: u32,
        padded_bytes_per_row: u32,
        yuv_layout: DisplayYuv420Layout,
        used_bytes: u64,
        mapped_bytes: u64,
        submission_index: wgpu::SubmissionIndex,
        buffer: wgpu::Buffer,
        slot: usize,
    ) -> Self {
        Self {
            cache_key,
            surface_generation,
            format,
            width,
            height,
            padded_bytes_per_row,
            yuv_layout,
            used_bytes,
            mapped_bytes,
            submission_index,
            buffer,
            receiver: None,
            map_ready: false,
            slot,
        }
    }

    pub(super) fn unmap_after_failed_map(&mut self) {
        self.receiver = None;
        self.map_ready = false;
        self.buffer.unmap();
    }

    pub(super) fn map_ready(&self) -> bool {
        self.map_ready
    }
}

impl DisplayYuv420Layout {
    pub(super) fn new(width: u32, height: u32) -> Self {
        let y_stride = width;
        let uv_stride = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_plane_len = y_stride
            .checked_mul(height)
            .expect("display Y plane size should fit in u32");
        let u_plane_len = uv_stride
            .checked_mul(uv_height)
            .expect("display U/V plane size should fit in u32");
        let total_len = y_plane_len
            .checked_add(
                u_plane_len
                    .checked_mul(2)
                    .expect("display chroma plane size should fit in u32"),
            )
            .expect("display YUV buffer size should fit in u32");
        let word_len = total_len
            .div_ceil(4)
            .checked_mul(4)
            .expect("display YUV word-aligned buffer size should fit in u32");

        Self {
            y_stride,
            uv_stride,
            y_plane_len,
            u_plane_len,
            total_len,
            word_len,
        }
    }
}

impl GpuDisplayFinalizeSurfaceSet {
    pub(super) fn new(device: &wgpu::Device, generation: u64, width: u32, height: u32) -> Self {
        let padded_bytes_per_row = super::padded_bytes_per_row(width);
        let yuv_layout = DisplayYuv420Layout::new(width, height);
        let yuv_output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize YUV output"),
            size: u64::from(yuv_layout.word_len),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback_size = u64::from(padded_bytes_per_row) * u64::from(height);
        let yuv_readback_size = u64::from(yuv_layout.word_len);
        Self {
            generation,
            padded_bytes_per_row,
            yuv_layout,
            output: GpuCompositorTexture::new(
                device,
                width,
                height,
                "SparkleFlinger Display Finalize Output",
            ),
            yuv_output,
            readbacks: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU display finalize readback"),
                    size: readback_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            yuv_readbacks: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU display finalize YUV readback"),
                    size: yuv_readback_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            readback_slots_in_use: [false; DISPLAY_FINALIZE_READBACK_SLOT_COUNT],
            next_readback_slot: 0,
            readback_surfaces: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(width, height),
                3,
            ),
            scene_source: None,
            face_source: None,
            pending_upload_buffers: super::PendingUploadBuffers::default(),
            #[cfg(test)]
            scene_upload_count: 0,
            #[cfg(test)]
            face_upload_count: 0,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            last_yuv_readback_bytes: 0,
        }
    }

    pub(super) fn next_readback_buffer(
        &mut self,
        format: GpuDisplayFinalizeFormat,
    ) -> Option<(usize, wgpu::Buffer)> {
        for offset in 0..DISPLAY_FINALIZE_READBACK_SLOT_COUNT {
            let slot = (self.next_readback_slot + offset) % DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
            if !self.readback_slots_in_use[slot] {
                self.readback_slots_in_use[slot] = true;
                self.next_readback_slot = (slot + 1) % DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
                let buffer = match format {
                    GpuDisplayFinalizeFormat::Rgba => self.readbacks[slot].clone(),
                    GpuDisplayFinalizeFormat::Yuv420 => self.yuv_readbacks[slot].clone(),
                };
                return Some((slot, buffer));
            }
        }

        None
    }

    pub(super) fn release_readback_slot(&mut self, slot: usize) {
        if slot < DISPLAY_FINALIZE_READBACK_SLOT_COUNT {
            self.readback_slots_in_use[slot] = false;
        }
    }
}

impl GpuDisplaySourceTexture {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32, label: &'static str) -> Self {
        Self {
            width,
            height,
            texture: GpuCompositorTexture::new(device, width, height, label),
            cached_upload: None,
            cached_gpu_copy: None,
        }
    }
}

impl GpuSparkleFlinger {
    #[cfg(test)]
    pub(crate) fn finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<PublishedSurface>> {
        let pending = match self.begin_finalize_display_face(scene, face, params)? {
            GpuDisplayFinalizeDispatch::Pending(pending) => pending,
            GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
                return Ok(None);
            }
        };
        match self.finish_pending_display_finalization_blocking(pending)? {
            Some(GpuDisplayFinalizeFrame::Rgba(surface)) => Ok(Some(surface)),
            Some(GpuDisplayFinalizeFrame::Yuv420(_)) | None => Ok(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<Option<DisplayYuv420Frame>> {
        let pending = match self.begin_finalize_display_face_yuv420(scene, face, params)? {
            GpuDisplayFinalizeDispatch::Pending(pending) => pending,
            GpuDisplayFinalizeDispatch::Unsupported | GpuDisplayFinalizeDispatch::Saturated => {
                return Ok(None);
            }
        };
        match self.finish_pending_display_finalization_blocking(pending)? {
            Some(GpuDisplayFinalizeFrame::Yuv420(frame)) => Ok(Some(frame)),
            Some(GpuDisplayFinalizeFrame::Rgba(_)) | None => Ok(None),
        }
    }

    pub(crate) fn begin_finalize_display_face(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        self.begin_display_finalize(scene, face, params, GpuDisplayFinalizeFormat::Rgba)
    }

    pub(crate) fn begin_finalize_display_face_yuv420(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        self.begin_display_finalize(scene, face, params, GpuDisplayFinalizeFormat::Yuv420)
    }

    fn begin_display_finalize(
        &mut self,
        scene: &ProducerFrame,
        face: &ProducerFrame,
        params: DisplayFinalizeParams,
        format: GpuDisplayFinalizeFormat,
    ) -> Result<GpuDisplayFinalizeDispatch> {
        if params.width == 0
            || params.height == 0
            || scene.width() == 0
            || scene.height() == 0
            || face.width() == 0
            || face.height() == 0
        {
            return Ok(GpuDisplayFinalizeDispatch::Unsupported);
        }

        record_gpu_display_finalize_attempt(format == GpuDisplayFinalizeFormat::Yuv420);
        self.flush_pending_output_submission()?;
        self.retain_current_display_finalize_route(params.cache_key);
        self.ensure_display_finalize_surfaces(params.cache_key);
        let device = &self.device;
        let queue = &self.queue;
        let pipeline = &mut self.pipeline;
        let surfaces = self
            .display_finalize_surfaces
            .get_mut(&params.cache_key)
            .expect("display finalize surfaces should exist after allocation");
        let surface_generation = surfaces.generation;
        let Some((readback_slot, readback_buffer)) = surfaces.next_readback_buffer(format) else {
            record_gpu_display_finalize_result(false);
            return Ok(GpuDisplayFinalizeDispatch::Saturated);
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SparkleFlinger GPU display finalize"),
        });

        let scene_gpu = gpu_source_frame(scene);
        prepare_display_source_texture(
            device,
            queue,
            pipeline,
            &mut encoder,
            &mut surfaces.scene_source,
            &mut surfaces.pending_upload_buffers,
            scene,
            scene_gpu.as_ref(),
            "SparkleFlinger Display Scene Source",
            #[cfg(test)]
            &mut surfaces.scene_upload_count,
        );
        let face_gpu = gpu_source_frame(face);
        prepare_display_source_texture(
            device,
            queue,
            pipeline,
            &mut encoder,
            &mut surfaces.face_source,
            &mut surfaces.pending_upload_buffers,
            face,
            face_gpu.as_ref(),
            "SparkleFlinger Display Face Source",
            #[cfg(test)]
            &mut surfaces.face_upload_count,
        );

        let scene_view = scene_gpu
            .as_ref()
            .filter(|frame| !frame.needs_display_source_copy())
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .scene_source
                    .as_ref()
                    .expect("uploaded scene source should exist")
                    .texture
                    .view
            });
        let face_view = face_gpu
            .as_ref()
            .filter(|frame| !frame.needs_display_source_copy())
            .map(GpuSourceFrame::view)
            .unwrap_or_else(|| {
                &surfaces
                    .face_source
                    .as_ref()
                    .expect("uploaded face source should exist")
                    .texture
                    .view
            });
        let bind_group = create_display_finalize_bind_group(
            device,
            pipeline,
            scene_view,
            face_view,
            &surfaces.output.view,
            &surfaces.yuv_output,
        );
        let params_offset = pipeline
            .display_finalize_params
            .write(
                device,
                queue,
                &mut encoder,
                &mut surfaces.pending_upload_buffers,
                &encode_display_finalize_params(&params, scene, face),
            )
            .offset;

        let (used_bytes, mapped_bytes) = match format {
            GpuDisplayFinalizeFormat::Rgba => {
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("SparkleFlinger GPU display finalize pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline.display_finalize_pipeline);
                    pass.set_bind_group(0, &bind_group, &[params_offset]);
                    pass.dispatch_workgroups(
                        params.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                        params.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                        1,
                    );
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &surfaces.output.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback_buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(surfaces.padded_bytes_per_row),
                            rows_per_image: Some(params.height),
                        },
                    },
                    texture_extent(params.width, params.height),
                );
                let bytes = u64::from(surfaces.padded_bytes_per_row) * u64::from(params.height);
                #[cfg(test)]
                {
                    surfaces.last_readback_bytes = bytes;
                }
                (bytes, bytes)
            }
            GpuDisplayFinalizeFormat::Yuv420 => {
                encoder.clear_buffer(&surfaces.yuv_output, 0, None);
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("SparkleFlinger GPU display finalize YUV420 pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline.display_finalize_yuv_pipeline);
                    pass.set_bind_group(0, &bind_group, &[params_offset]);
                    pass.dispatch_workgroups(
                        params.width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
                        params.height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
                        1,
                    );
                }
                encoder.copy_buffer_to_buffer(
                    &surfaces.yuv_output,
                    0,
                    &readback_buffer,
                    0,
                    u64::from(surfaces.yuv_layout.word_len),
                );
                #[cfg(test)]
                {
                    surfaces.last_yuv_readback_bytes = u64::from(surfaces.yuv_layout.total_len);
                }
                (
                    u64::from(surfaces.yuv_layout.total_len),
                    u64::from(surfaces.yuv_layout.word_len),
                )
            }
        };
        let submission_index = queue.submit(Some(encoder.finish()));
        surfaces.pending_upload_buffers.clear();
        let pending = begin_display_finalize_readback(PendingGpuDisplayFinalize::new(
            params.cache_key,
            surface_generation,
            format,
            params.width,
            params.height,
            surfaces.padded_bytes_per_row,
            surfaces.yuv_layout,
            used_bytes,
            mapped_bytes,
            submission_index,
            readback_buffer,
            readback_slot,
        ));
        self.release_retired_uniform_slots();
        Ok(GpuDisplayFinalizeDispatch::Pending(pending))
    }

    pub(crate) fn try_finish_pending_display_finalization(
        &mut self,
        pending: &mut PendingGpuDisplayFinalize,
    ) -> Result<Option<GpuDisplayFinalizeFrame>> {
        if let Err(error) = poll_display_finalize_readback_ready(&self.device, pending) {
            self.discard_pending_display_finalization_slot(pending);
            return Err(error);
        }
        if !pending.map_ready() {
            return Ok(None);
        }
        let frame = match self.finish_display_finalize_readback(pending) {
            Ok(frame) => frame,
            Err(error) => {
                pending.buffer.unmap();
                self.release_display_finalize_slot(
                    pending.cache_key,
                    pending.surface_generation,
                    pending.slot,
                );
                return Err(error);
            }
        };
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
        record_gpu_display_finalize_result(true);
        Ok(Some(frame))
    }

    pub(crate) fn discard_pending_display_finalization(
        &mut self,
        pending: PendingGpuDisplayFinalize,
    ) {
        pending.buffer.unmap();
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
    }

    fn discard_pending_display_finalization_slot(
        &mut self,
        pending: &mut PendingGpuDisplayFinalize,
    ) {
        pending.unmap_after_failed_map();
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
    }

    #[cfg(test)]
    pub(super) fn finish_pending_display_finalization_blocking(
        &mut self,
        mut pending: PendingGpuDisplayFinalize,
    ) -> Result<Option<GpuDisplayFinalizeFrame>> {
        let wait_start = Instant::now();
        let ready = match wait_for_display_finalize_readback(&self.device, &mut pending) {
            Ok(ready) => ready,
            Err(error) => {
                self.discard_pending_display_finalization_slot(&mut pending);
                return Err(error);
            }
        };
        record_gpu_display_finalize_blocking_wait(wait_start.elapsed());
        if !ready {
            self.discard_pending_display_finalization(pending);
            record_gpu_display_finalize_result(false);
            return Ok(None);
        }
        let frame = match self.finish_display_finalize_readback(&pending) {
            Ok(frame) => frame,
            Err(error) => {
                pending.buffer.unmap();
                self.release_display_finalize_slot(
                    pending.cache_key,
                    pending.surface_generation,
                    pending.slot,
                );
                return Err(error);
            }
        };
        self.release_display_finalize_slot(
            pending.cache_key,
            pending.surface_generation,
            pending.slot,
        );
        record_gpu_display_finalize_result(true);
        Ok(Some(frame))
    }

    fn finish_display_finalize_readback(
        &mut self,
        pending: &PendingGpuDisplayFinalize,
    ) -> Result<GpuDisplayFinalizeFrame> {
        match pending.format {
            GpuDisplayFinalizeFormat::Rgba => {
                let surfaces = self
                    .display_finalize_surfaces
                    .get_mut(&pending.cache_key)
                    .filter(|surfaces| surfaces.generation == pending.surface_generation)
                    .context("GPU display finalize surfaces changed before RGBA readback")?;
                copy_mapped_readback_buffer_into_surface(
                    &pending.buffer,
                    pending.used_bytes,
                    pending.width,
                    pending.height,
                    pending.padded_bytes_per_row,
                    &mut surfaces.readback_surfaces,
                    #[cfg(test)]
                    &mut surfaces.last_readback_bytes,
                )
                .map(GpuDisplayFinalizeFrame::Rgba)
            }
            GpuDisplayFinalizeFormat::Yuv420 => Ok(GpuDisplayFinalizeFrame::Yuv420(
                finish_yuv420_display_readback(pending),
            )),
        }
    }

    fn ensure_display_finalize_surfaces(&mut self, key: DisplayFinalizeCacheKey) {
        if !self.display_finalize_surfaces.contains_key(&key) {
            record_gpu_display_finalize_surface_realloc();
            self.display_finalize_generation = self.display_finalize_generation.saturating_add(1);
            self.display_finalize_surfaces.insert(
                key,
                GpuDisplayFinalizeSurfaceSet::new(
                    &self.device,
                    self.display_finalize_generation,
                    key.width,
                    key.height,
                ),
            );
        }
    }

    fn retain_current_display_finalize_route(&mut self, key: DisplayFinalizeCacheKey) {
        self.display_finalize_surfaces
            .retain(|cached_key, _| cached_key.group_id != key.group_id || *cached_key == key);
    }

    pub(crate) fn retain_display_finalize_groups(&mut self, active_group_ids: &[ZoneId]) {
        self.display_finalize_surfaces
            .retain(|key, _| active_group_ids.contains(&key.group_id));
    }

    fn release_display_finalize_slot(
        &mut self,
        key: DisplayFinalizeCacheKey,
        surface_generation: u64,
        slot: usize,
    ) {
        if let Some(surfaces) = self.display_finalize_surfaces.get_mut(&key)
            && surfaces.generation == surface_generation
        {
            surfaces.release_readback_slot(slot);
        }
    }
}

pub(super) fn begin_display_finalize_readback(
    mut pending: PendingGpuDisplayFinalize,
) -> PendingGpuDisplayFinalize {
    let slice = pending.buffer.slice(..pending.mapped_bytes);
    let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    pending.receiver = Some(receiver);
    pending
}

pub(super) fn poll_display_finalize_readback_ready(
    device: &wgpu::Device,
    pending: &mut PendingGpuDisplayFinalize,
) -> Result<bool> {
    if pending.map_ready {
        return Ok(true);
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    if take_display_finalize_readback_ready(pending)? {
        return Ok(true);
    }

    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending.submission_index.clone()),
        timeout: Some(Duration::ZERO),
    }) {
        Ok(_) | Err(wgpu::PollError::Timeout) => {}
        Err(error) => {
            return Err(error).context("GPU display finalize readiness poll failed");
        }
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    take_display_finalize_readback_ready(pending)
}

#[cfg(test)]
pub(super) fn wait_for_display_finalize_readback(
    device: &wgpu::Device,
    pending: &mut PendingGpuDisplayFinalize,
) -> Result<bool> {
    if pending.map_ready {
        return Ok(true);
    }

    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending.submission_index.clone()),
        timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
    }) {
        Ok(_) => {}
        Err(wgpu::PollError::Timeout) => return Ok(false),
        Err(error) => return Err(error).context("GPU display finalize wait failed"),
    }

    if take_display_finalize_readback_ready(pending)? {
        return Ok(true);
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    take_display_finalize_readback_ready(pending)
}

fn take_display_finalize_readback_ready(pending: &mut PendingGpuDisplayFinalize) -> Result<bool> {
    let Some(receiver) = pending.receiver.take() else {
        return Ok(pending.map_ready);
    };
    match receiver.try_recv() {
        Ok(Ok(())) => {
            pending.map_ready = true;
            Ok(true)
        }
        Ok(Err(error)) => {
            pending.buffer.unmap();
            Err(error).context("GPU display finalize buffer mapping failed")
        }
        Err(TryRecvError::Disconnected) => {
            pending.buffer.unmap();
            anyhow::bail!("GPU display finalize channel closed before map completion");
        }
        Err(TryRecvError::Empty) => {
            pending.receiver = Some(receiver);
            Ok(false)
        }
    }
}

pub(super) fn finish_yuv420_display_readback(
    pending: &PendingGpuDisplayFinalize,
) -> DisplayYuv420Frame {
    let slice = pending.buffer.slice(..pending.mapped_bytes);
    let mapped = slice.get_mapped_range();
    let used_len = usize::try_from(pending.used_bytes).expect("YUV readback should fit usize");
    let mut data = Vec::with_capacity(used_len);
    data.extend_from_slice(&mapped[..used_len]);
    drop(mapped);
    pending.buffer.unmap();
    let layout = pending.yuv_layout;

    DisplayYuv420Frame::from_vec(
        data,
        pending.width,
        pending.height,
        layout.y_stride,
        layout.uv_stride,
        usize::try_from(layout.y_plane_len).expect("Y plane length should fit usize"),
        usize::try_from(layout.u_plane_len).expect("U plane length should fit usize"),
        0,
        0,
    )
}

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

fn display_finalize_mode(mode: DisplayFaceBlendMode) -> DisplayFinalizeShaderMode {
    match mode {
        DisplayFaceBlendMode::Replace => DisplayFinalizeShaderMode::Replace,
        DisplayFaceBlendMode::Alpha => DisplayFinalizeShaderMode::Alpha,
        DisplayFaceBlendMode::Tint => DisplayFinalizeShaderMode::Tint,
        DisplayFaceBlendMode::LumaReveal => DisplayFinalizeShaderMode::LumaReveal,
        DisplayFaceBlendMode::Add => DisplayFinalizeShaderMode::Add,
        DisplayFaceBlendMode::Screen => DisplayFinalizeShaderMode::Screen,
        DisplayFaceBlendMode::Multiply => DisplayFinalizeShaderMode::Multiply,
        DisplayFaceBlendMode::Overlay => DisplayFinalizeShaderMode::Overlay,
        DisplayFaceBlendMode::SoftLight => DisplayFinalizeShaderMode::SoftLight,
        DisplayFaceBlendMode::ColorDodge => DisplayFinalizeShaderMode::ColorDodge,
        DisplayFaceBlendMode::Difference => DisplayFinalizeShaderMode::Difference,
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
