use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{
    BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool, SurfaceDescriptor, SurfaceStateCounts,
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
    pub(super) fn prepare_preview_surfaces_for_canvas_resize(
        &mut self,
        source_width: u32,
        source_height: u32,
        request: PreviewSurfaceRequest,
        scale_views: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
    ) -> Result<GpuPreviewSurfaceSet> {
        let prepared =
            self.prepare_preview_surface_change(request.width, request.height, scale_views, true)?;
        let PreparedPreviewSurfaceChangeKind::Replace(replacement) = prepared.0 else {
            unreachable!("forced preview preparation must create a replacement")
        };
        debug_assert_eq!(
            scale_views.is_some(),
            preview_requires_scale(request, source_width, source_height)
        );
        Ok(replacement)
    }

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
        let submission_index = frame_in_flight
            .as_mut()
            .and_then(|frame| frame.submit(queue));
        if let Some(submission_index) = submission_index {
            self.finish_pending_uploads(submission_index);
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
        self.discard_pending_uploads();
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

    fn prepare_preview_surface_change(
        &mut self,
        width: u32,
        height: u32,
        scale_views: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
        force_replace: bool,
    ) -> Result<PreparedPreviewSurfaceChange> {
        let needs_scale_output = scale_views.is_some()
            && (force_replace
                || self.preview_surfaces.as_ref().is_none_or(|surfaces| {
                    !surfaces.fits_request(width, height) || surfaces.scale_output.is_none()
                }));
        let fail_scale_output =
            needs_scale_output && self.take_preview_scale_output_failure_injection();

        if !force_replace
            && let Some(surfaces) = self.preview_surfaces.as_ref()
            && surfaces.width == width
            && surfaces.height == height
        {
            let scale_output = scale_views
                .filter(|_| surfaces.scale_output.is_none())
                .map(|(front_view, back_view)| {
                    GpuPreviewScaleOutput::try_new(
                        &self.device,
                        &self.pipeline,
                        front_view,
                        back_view,
                        surfaces.capacity_width,
                        surfaces.capacity_height,
                        self.max_storage_buffer_binding_size,
                        fail_scale_output,
                    )
                })
                .transpose()?;
            return Ok(PreparedPreviewSurfaceChange(
                PreparedPreviewSurfaceChangeKind::Unchanged { scale_output },
            ));
        }

        if !force_replace
            && let Some(surfaces) = self.preview_surfaces.as_ref()
            && surfaces.fits_request(width, height)
        {
            let readback_surfaces = surfaces.prepare_reconfiguration(width, height)?;
            let scale_output = scale_views
                .filter(|_| surfaces.scale_output.is_none())
                .map(|(front_view, back_view)| {
                    GpuPreviewScaleOutput::try_new(
                        &self.device,
                        &self.pipeline,
                        front_view,
                        back_view,
                        surfaces.capacity_width,
                        surfaces.capacity_height,
                        self.max_storage_buffer_binding_size,
                        fail_scale_output,
                    )
                })
                .transpose()?;
            return Ok(PreparedPreviewSurfaceChange(
                PreparedPreviewSurfaceChangeKind::Reconfigure {
                    width,
                    height,
                    readback_surfaces,
                    scale_output,
                },
            ));
        }

        let mut replacement = GpuPreviewSurfaceSet::try_new(&self.device, width, height)?;
        if let Some((front_view, back_view)) = scale_views {
            replacement.scale_output = Some(GpuPreviewScaleOutput::try_new(
                &self.device,
                &self.pipeline,
                front_view,
                back_view,
                replacement.capacity_width,
                replacement.capacity_height,
                self.max_storage_buffer_binding_size,
                fail_scale_output,
            )?);
            #[cfg(test)]
            {
                replacement.preview_bind_group_count = 2;
            }
        }
        Ok(PreparedPreviewSurfaceChange(
            PreparedPreviewSurfaceChangeKind::Replace(replacement),
        ))
    }

    pub(super) fn prepare_preview_surface_readback(
        &mut self,
        source_width: u32,
        source_height: u32,
        request: PreviewSurfaceRequest,
        scale_views: Option<(wgpu::TextureView, wgpu::TextureView)>,
        force_replace: bool,
    ) -> Result<PreparedPreviewSurfaceChange> {
        super::ensure_readback_buffer_capacity(
            self.max_buffer_size,
            request.width,
            request.height,
            false,
        )?;
        let requires_scale = preview_requires_scale(request, source_width, source_height);
        if requires_scale {
            super::ensure_storage_buffer_capacity(
                self.max_storage_buffer_binding_size,
                request.width,
                request.height,
            )?;
        }
        let scale_views = if requires_scale {
            Some(
                scale_views
                    .context("GPU preview scale requested without compositor surface views")?,
            )
        } else {
            None
        };
        self.prepare_preview_surface_change(
            request.width,
            request.height,
            scale_views
                .as_ref()
                .map(|(front_view, back_view)| (front_view, back_view)),
            force_replace,
        )
    }

    fn commit_preview_surface_change(&mut self, prepared: PreparedPreviewSurfaceChange) {
        match prepared.0 {
            PreparedPreviewSurfaceChangeKind::Unchanged { scale_output } => {
                if let Some(scale_output) = scale_output {
                    let surfaces = self
                        .preview_surfaces
                        .as_mut()
                        .expect("unchanged preview preparation requires existing surfaces");
                    surfaces.scale_output = Some(scale_output);
                    #[cfg(test)]
                    {
                        surfaces.preview_bind_group_count =
                            surfaces.preview_bind_group_count.saturating_add(2);
                    }
                }
            }
            PreparedPreviewSurfaceChangeKind::Reconfigure {
                width,
                height,
                readback_surfaces,
                scale_output,
            } => {
                self.discard_pending_preview_map();
                let surfaces = self
                    .preview_surfaces
                    .as_mut()
                    .expect("preview reconfiguration requires existing surfaces");
                surfaces.commit_reconfiguration(width, height, readback_surfaces);
                if let Some(scale_output) = scale_output {
                    surfaces.scale_output = Some(scale_output);
                    #[cfg(test)]
                    {
                        surfaces.preview_bind_group_count =
                            surfaces.preview_bind_group_count.saturating_add(2);
                    }
                }
            }
            PreparedPreviewSurfaceChangeKind::Replace(replacement) => {
                self.discard_pending_preview_map();
                self.preview_surfaces = Some(replacement);
                #[cfg(test)]
                {
                    self.preview_surface_allocation_count =
                        self.preview_surface_allocation_count.saturating_add(1);
                }
            }
        }
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
        if let Some(encoder) =
            self.supersede_frame_in_flight("preview restaged over retained frame")
        {
            drop(encoder);
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
    pub(super) fn surface_pool_counts(&mut self) -> SurfaceStateCounts {
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
        let readbacks = super::try_create_gpu_resources(
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

    fn prepare_reconfiguration(
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

    fn commit_reconfiguration(
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
    pub(super) fn has_scale_output(&self) -> bool {
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
    fn try_new(
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        capacity_width: u32,
        capacity_height: u32,
        max_storage_buffer_binding_size: u64,
        fail_after_prepare: bool,
    ) -> Result<Self> {
        let size = super::ensure_storage_buffer_capacity(
            max_storage_buffer_binding_size,
            capacity_width,
            capacity_height,
        )?;
        let prepared = super::try_create_gpu_resources(
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
        super::reject_injected_gpu_preparation(fail_after_prepare, "preview scale output")?;
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

pub(super) fn bypass_preview_surface(frame: &ProducerFrame) -> Option<PublishedSurface> {
    match frame {
        ProducerFrame::Surface(surface) => Some(surface.clone()),
        ProducerFrame::Canvas(_) | ProducerFrame::ScreenPublication(_) => None,
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

pub(super) fn preview_requires_scale(
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
