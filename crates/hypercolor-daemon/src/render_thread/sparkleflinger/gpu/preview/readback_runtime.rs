use anyhow::{Context, Result};
use hypercolor_types::canvas::{BYTES_PER_PIXEL, PublishedSurface};

use super::super::frame_set::{gpu_composed_with_preview_surface, gpu_composed_without_surfaces};
use super::super::readback::{
    CachedReadbackKey, CachedReadbackSurface, copy_mapped_readback_buffer_into_surface,
};
use super::super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuCompositorOutputSurface,
    GpuSparkleFlinger, texture_extent,
};
use super::{
    CachedPreviewSurfaceKey, PendingPreviewReadback, PreparedPreviewSurfaceChange,
    encode_preview_scale_params,
};
use crate::render_thread::producer_queue::NativeScreenTextureLease;
use crate::render_thread::sparkleflinger::{ComposedFrameSet, PreviewSurfaceRequest};

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn stage_preview_surface_readback(
        &mut self,
        current_output: GpuCompositorOutputSurface,
        source_width: u32,
        source_height: u32,
        readback_key: Option<CachedReadbackKey>,
        request: PreviewSurfaceRequest,
        cache_as_full_size: bool,
        encoder: Option<wgpu::CommandEncoder>,
        native_screen_leases: Vec<NativeScreenTextureLease>,
        prepared_surface_change: Option<PreparedPreviewSurfaceChange>,
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
            self.discard_pending_uploads();
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
        let scale_views = direct_source_texture
            .is_none()
            .then(|| {
                let surfaces = self.surfaces.as_ref().context(
                    "GPU preview scale requested before compositor surfaces were allocated",
                )?;
                Ok::<_, anyhow::Error>((surfaces.front.view.clone(), surfaces.back.view.clone()))
            })
            .transpose()?;
        let prepared = match prepared_surface_change {
            Some(prepared) => prepared,
            None => self.prepare_preview_surface_readback(
                source_width,
                source_height,
                request,
                scale_views,
                false,
            )?,
        };
        self.commit_preview_surface_change(prepared);
        let mapped_readback_slot = self
            .pending_preview_map
            .as_ref()
            .map(|pending| match &pending.readback {
                PendingPreviewReadback::PreviewBuffer { slot, .. } => *slot,
            });
        if let Some(stashed) =
            self.supersede_frame_in_flight("preview restaged over retained frame")
        {
            drop(stashed);
            self.discard_pending_uploads();
        }
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
            let scale_output = preview_surfaces
                .scale_output
                .as_ref()
                .expect("scaled preview output should exist after preparation");
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
                GpuCompositorOutputSurface::Front => &scale_output.bind_groups.front_to_preview,
                GpuCompositorOutputSurface::Back => &scale_output.bind_groups.back_to_preview,
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
                &scale_output.buffer,
                0,
                preview_surfaces.readback(readback_slot),
                0,
                u64::from(preview_surfaces.padded_bytes_per_row) * u64::from(request.height),
            );
        }
        self.stage_frame_in_flight_with_native_screen_leases(
            encoder,
            Some(PendingPreviewReadback::PreviewBuffer {
                request,
                readback_key,
                cache_as_full_size,
                slot: readback_slot,
            }),
            native_screen_leases,
        );
        Ok(gpu_composed_without_surfaces())
    }

    pub(in crate::render_thread::sparkleflinger::gpu::preview) fn finish_mapped_preview_surface(
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
