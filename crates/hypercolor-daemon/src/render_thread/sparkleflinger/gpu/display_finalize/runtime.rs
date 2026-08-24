#[cfg(test)]
use std::time::Instant;

use anyhow::{Context, Result};
#[cfg(test)]
use hypercolor_core::bus::DisplayYuv420Frame;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::scene::ZoneId;

use super::super::super::{DisplayFinalizeCacheKey, DisplayFinalizeParams};
use super::super::readback::copy_mapped_readback_buffer_into_surface;
use super::super::source::{GpuSourceFrame, gpu_source_frame, prepare_display_source_texture};
#[cfg(test)]
use super::super::telemetry::record_gpu_display_finalize_blocking_wait;
use super::super::telemetry::{
    record_gpu_display_finalize_attempt, record_gpu_display_finalize_result,
    record_gpu_display_finalize_surface_realloc,
};
use super::super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuSparkleFlinger, texture_extent,
};
#[cfg(test)]
use super::readback::wait_for_display_finalize_readback;
use super::readback::{
    begin_display_finalize_readback, finish_yuv420_display_readback,
    poll_display_finalize_readback_ready,
};
use super::shader::{create_display_finalize_bind_group, encode_display_finalize_params};
use super::{
    GpuDisplayFinalizeDispatch, GpuDisplayFinalizeFormat, GpuDisplayFinalizeFrame,
    GpuDisplayFinalizeSurfaceSet, PendingGpuDisplayFinalize,
};
use crate::render_thread::producer_queue::ProducerFrame;

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
        let mut native_screen_leases = Vec::new();
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
            &mut native_screen_leases,
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
            &mut native_screen_leases,
            #[cfg(test)]
            &mut surfaces.face_upload_count,
        );

        for frame in [&scene_gpu, &face_gpu]
            .into_iter()
            .flatten()
            .filter(|frame| !frame.needs_display_source_copy())
        {
            if let Some(lease) = frame.native_screen_lease() {
                native_screen_leases.push(lease);
            }
        }

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
            submission_index.clone(),
            readback_buffer,
            readback_slot,
        ));
        self.retire_native_screen_leases(submission_index, native_screen_leases);
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
    pub(in crate::render_thread::sparkleflinger::gpu) fn finish_pending_display_finalization_blocking(
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
            .retain(|cached_key, _| cached_key.zone_id != key.zone_id || *cached_key == key);
    }

    pub(crate) fn retain_display_finalize_zones(&mut self, active_zone_ids: &[ZoneId]) {
        self.display_finalize_surfaces
            .retain(|key, _| active_zone_ids.contains(&key.zone_id));
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
