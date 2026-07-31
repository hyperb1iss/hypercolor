use std::collections::{HashMap, VecDeque};
use std::sync::Weak;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::ScreenResourceLifetime;
use hypercolor_core::types::canvas::{
    PublishedSurface, RenderSurfacePool, SurfaceDescriptor, SurfaceStateCounts,
};

use super::super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
};
use super::frame_set::{
    gpu_bypassed_canvas_frame, gpu_bypassed_surface_frame, gpu_bypassed_without_surfaces,
    gpu_composed_from_surface, gpu_composed_with_preview_surface, gpu_composed_without_surfaces,
};
use super::preview::{
    CachedPreviewSurfaceKey, PreparedPreviewSurfaceChange, bypass_preview_surface,
    preview_request_matches_plan, preview_requires_scale,
};
use super::readback::{
    CachedReadbackKey, CachedReadbackSurface, copy_mapped_readback_buffer_into_surface,
};
use super::screen_upload::ScreenUploadPoolSaturated;
use super::source::{
    CachedSourceUpload, cached_readback_key, cached_source_upload, copy_frame_into_output_texture,
    copy_gpu_source_frame_into_texture, gpu_source_frame, upload_frame_into_cached_texture,
    upload_frame_into_source_texture,
};
use super::telemetry::record_gpu_source_upload_skipped;
use super::{
    COMPOSE_PARAM_BYTES, COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH,
    GpuCompositorOutputSurface, GpuCompositorPipeline, GpuCompositorSurfaceSet, GpuSparkleFlinger,
    ScreenUploadContentKey, padded_bytes_per_row, texture_extent,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameLease, GpuTextureFrameOrigin, ProducerFrame,
};

const SAMPLING_READBACK_SLOT_COUNT: usize = 2;
const SAMPLING_READBACK_SURFACE_SLOTS: usize = 3;

/// One-frame readback latch that lets CPU spatial sampling follow GPU-only
/// composition plans (Servo imports and media producer textures) one frame
/// behind. Plans containing GPU producer frames have no `CachedReadbackKey`,
/// so the keyed readback cache can never service them; instead every compose
/// stages a full-canvas copy of the freshly composed output and the next
/// compose returns the previously staged surface.
#[derive(Default)]
pub(super) struct SamplingReadbackLatch {
    buffers: Option<SamplingReadbackBuffers>,
    pending: Option<PendingSamplingReadback>,
    latched: Option<LatchedSamplingSurface>,
    #[cfg(test)]
    pub(super) last_readback_bytes: u64,
}

pub(super) struct SamplingReadbackBuffers {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    readbacks: [wgpu::Buffer; SAMPLING_READBACK_SLOT_COUNT],
    next_slot: usize,
    surfaces: RenderSurfacePool,
}

impl SamplingReadbackBuffers {
    fn try_new(
        device: &wgpu::Device,
        max_buffer_size: u64,
        width: u32,
        height: u32,
        fail_after_prepare: bool,
    ) -> Result<Self> {
        super::ensure_readback_buffer_capacity(max_buffer_size, width, height, true)?;
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("GPU sampling latch readback byte size overflowed")?;
        let surfaces = RenderSurfacePool::try_with_slot_count(
            SurfaceDescriptor::rgba8888(width, height),
            SAMPLING_READBACK_SURFACE_SLOTS,
        )
        .context("GPU sampling latch CPU surface allocation failed")?;
        let readbacks = super::try_create_gpu_resources(
            device,
            "GPU sampling latch buffer allocation failed",
            || {
                std::array::from_fn(|_| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("SparkleFlinger GPU sampling latch readback"),
                        size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    })
                })
            },
        )?;
        let prepared = Self {
            width,
            height,
            padded_bytes_per_row,
            readbacks,
            next_slot: 0,
            surfaces,
        };
        super::reject_injected_gpu_preparation(fail_after_prepare, "sampling readback")?;
        Ok(prepared)
    }
}

impl SamplingReadbackLatch {
    pub(super) fn install_buffers(&mut self, buffers: Option<SamplingReadbackBuffers>) {
        self.buffers = buffers;
    }

    #[cfg(test)]
    pub(super) fn buffer_extent(&self) -> Option<(u32, u32)> {
        self.buffers
            .as_ref()
            .map(|buffers| (buffers.width, buffers.height))
    }

    pub(super) fn surface_pool_counts(&mut self) -> SurfaceStateCounts {
        self.buffers
            .as_mut()
            .map_or_else(SurfaceStateCounts::default, |buffers| {
                buffers.surfaces.slot_counts()
            })
    }
}

struct PendingSamplingReadback {
    width: u32,
    height: u32,
    output_generation: u64,
    slot: usize,
    used_bytes: u64,
    submission_index: wgpu::SubmissionIndex,
    receiver: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

struct LatchedSamplingSurface {
    width: u32,
    height: u32,
    output_generation: u64,
    surface: PublishedSurface,
}

impl GpuSparkleFlinger {
    pub(crate) fn compose_attempt(
        &mut self,
        plan: &CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> super::GpuComposeOutcome {
        match self.compose(plan, requires_cpu_sampling_canvas, preview_surface_request) {
            Ok(composed) => super::GpuComposeOutcome::Produced(composed),
            Err(error) if error.is::<ScreenUploadPoolSaturated>() => {
                tracing::debug!(%error, "retaining GPU output while screen uploads are saturated");
                super::GpuComposeOutcome::Retained(gpu_composed_without_surfaces())
            }
            Err(error) => super::GpuComposeOutcome::Failed(error),
        }
    }

    #[allow(
        clippy::drop_non_drop,
        reason = "the screen-frame iterator borrow must end before its backing scratch returns"
    )]
    pub(crate) fn compose(
        &mut self,
        plan: &CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
        #[cfg(test)]
        if self.take_screen_upload_pool_saturation_injection() {
            return Err(ScreenUploadPoolSaturated::for_test().into());
        }
        let requires_preview_surface = preview_surface_request.is_some();
        let readback_key = cached_readback_key(plan);
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && self.layer_reuses_current_output_texture(layer, plan.width, plan.height)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                readback_key,
                requires_cpu_sampling_canvas,
                preview_surface_request,
                None,
                None,
            );
        }
        anyhow::ensure!(
            !self.plan_samples_compositor_storage(plan),
            "GPU composition source aliases compositor storage after its generation changed"
        );
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && layer.is_bypass_candidate()
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            return self.compose_bypass_layer(
                plan,
                readback_key,
                layer,
                requires_cpu_sampling_canvas,
                preview_surface_request,
            );
        }

        if requires_cpu_sampling_canvas && readback_key.is_none() {
            super::ensure_readback_buffer_capacity(
                self.max_buffer_size,
                plan.width,
                plan.height,
                true,
            )?;
        } else if !requires_cpu_sampling_canvas && let Some(request) = preview_surface_request {
            super::ensure_readback_buffer_capacity(
                self.max_buffer_size,
                request.width,
                request.height,
                false,
            )?;
            if super::preview::preview_requires_scale(request, plan.width, plan.height) {
                super::ensure_storage_buffer_capacity(
                    self.max_storage_buffer_binding_size,
                    request.width,
                    request.height,
                )?;
            }
        }
        let mut prepared_surface_replacement =
            self.prepare_surface_size(plan.width, plan.height)?;
        let prepared_sampling_readback = if requires_cpu_sampling_canvas && readback_key.is_none() {
            self.prepare_sampling_readback_buffers(plan.width, plan.height)?
        } else {
            None
        };
        if prepared_surface_replacement.is_none()
            && let Some(key) = readback_key.as_ref()
            && self.current_output.is_some()
            && self.cached_composition_key.as_ref() == Some(key)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|cached| {
                    cached.key.as_ref() == Some(key)
                        && preview_request_matches_plan(
                            preview_surface_request,
                            plan.width,
                            plan.height,
                        )
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_from_surface(
                    surface,
                    requires_cpu_sampling_canvas,
                ));
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && self.has_pending_or_ready_preview_for(request)
            {
                return Ok(gpu_composed_without_surfaces());
            }
            let prepared_preview_surface = self.prepare_preview_for_surfaces(
                plan.width,
                plan.height,
                preview_surface_request,
                requires_cpu_sampling_canvas,
                None,
                false,
            )?;
            let pending_output_submission =
                self.supersede_frame_in_flight("current output readback restaged");
            if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
                self.ready_preview_surface = None;
            } else {
                self.discard_pending_preview_map();
                self.ready_preview_surface = None;
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                Some(key.clone()),
                requires_cpu_sampling_canvas,
                preview_surface_request,
                pending_output_submission,
                prepared_preview_surface,
            );
        }
        let reuse_cached_readback = readback_key.as_ref().is_some_and(|key| {
            self.cached_readback_surface.as_ref().is_some_and(|cached| {
                cached.key.as_ref() == Some(key)
                    && preview_request_matches_plan(
                        preview_surface_request,
                        plan.width,
                        plan.height,
                    )
            })
        });
        let prepared_preview_surface = if reuse_cached_readback {
            None
        } else {
            self.prepare_preview_for_surfaces(
                plan.width,
                plan.height,
                preview_surface_request,
                requires_cpu_sampling_canvas,
                prepared_surface_replacement.as_ref(),
                prepared_surface_replacement.is_some(),
            )?
        };
        let first_layer = plan
            .layers
            .first()
            .context("GPU composition requires at least one layer")?;
        let has_screen_uploads = has_screen_upload_layers(&plan.layers);
        if has_screen_uploads {
            self.flush_pending_output_submission()?;
        }
        let mut uploaded_screen_frame_scratch = if has_screen_uploads {
            let surfaces = prepared_surface_replacement
                .as_mut()
                .or(self.surfaces.as_mut())
                .expect("surface preparation should succeed before screen upload admission");
            if let Err(error) = surfaces
                .screen_upload_pool
                .preflight_uploads(&self.device, screen_upload_content_keys(&plan.layers))
            {
                return Err(error);
            }
            match upload_screen_layers(&self.device, &self.queue, surfaces, &plan.layers) {
                Ok(()) => Some(std::mem::take(&mut surfaces.uploaded_screen_frame_scratch)),
                Err(error) => {
                    surfaces.discard_pending_uploads();
                    return Err(error);
                }
            }
        } else {
            None
        };
        self.commit_surface_size(prepared_surface_replacement);
        self.commit_sampling_readback_buffers(prepared_sampling_readback);
        if has_screen_uploads {
            // The newly encoded uploads must survive preview cleanup. Any
            // older deferred frame was submitted before preparation, so only
            // its preview bookkeeping may be retired here.
            drop(self.supersede_frame_in_flight("screen upload outputs superseded"));
            self.ready_preview_surface = None;
            if preview_surface_request.is_none() || requires_cpu_sampling_canvas {
                self.discard_pending_preview_map();
            }
        } else if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
            self.clear_superseded_preview_outputs();
        } else {
            self.discard_superseded_preview_work();
        }
        // The stashed encoder (if any) was just submitted or dropped and no
        // local encoder exists yet, so retired uniform ring slots are safe to
        // reuse from here on.
        self.release_retired_uniform_slots();

        let surfaces = self
            .surfaces
            .as_mut()
            .expect("surface allocation should succeed before composition");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU compose"),
            });

        let mut use_front_as_current = true;
        let mut uploaded_screen_frames = uploaded_screen_frame_scratch
            .as_ref()
            .map(|frames| frames.iter());
        let first_uploaded_screen_frame = uploaded_screen_frames
            .as_mut()
            .and_then(Iterator::next)
            .and_then(Option::as_ref);

        if first_layer.can_bypass_for_size(plan.width, plan.height) {
            copy_frame_into_output_texture(
                &self.device,
                &self.queue,
                &mut self.pipeline,
                &surfaces.front,
                &mut surfaces.front_contents,
                &mut surfaces.pending_upload_buffers,
                &mut surfaces.source_copy_bind_groups,
                &mut encoder,
                &first_layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
        } else {
            // The first layer only blends against FRONT when it cannot take
            // the direct-copy path inside `compose_layer_into_gpu`; skip the
            // clear (and keep `front_contents` accurate) when FRONT is never
            // read.
            if !first_layer.replaces_output_directly(plan.width, plan.height) {
                let full_range = wgpu::ImageSubresourceRange::default();
                encoder.clear_texture(&surfaces.front.texture, &full_range);
                surfaces.front_contents = None;
            }
            let compose_result = compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &mut self.pipeline,
                surfaces,
                &mut encoder,
                first_layer,
                first_uploaded_screen_frame,
                true,
            );
            if let Err(error) = compose_result {
                drop(uploaded_screen_frames);
                return_screen_frame_scratch(surfaces, &mut uploaded_screen_frame_scratch);
                return Err(error);
            }
            use_front_as_current = false;
        }

        for layer in plan.layers.iter().skip(1) {
            let uploaded_screen_frame = uploaded_screen_frames
                .as_mut()
                .and_then(Iterator::next)
                .and_then(Option::as_ref);
            let compose_result = compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &mut self.pipeline,
                surfaces,
                &mut encoder,
                layer,
                uploaded_screen_frame,
                use_front_as_current,
            );
            if let Err(error) = compose_result {
                drop(uploaded_screen_frames);
                return_screen_frame_scratch(surfaces, &mut uploaded_screen_frame_scratch);
                return Err(error);
            }
            use_front_as_current = !use_front_as_current;
        }
        debug_assert!(
            uploaded_screen_frames
                .as_mut()
                .is_none_or(|frames| frames.next().is_none())
        );
        drop(uploaded_screen_frames);
        return_screen_frame_scratch(surfaces, &mut uploaded_screen_frame_scratch);

        let current_output = if use_front_as_current {
            GpuCompositorOutputSurface::Front
        } else {
            GpuCompositorOutputSurface::Back
        };
        self.current_output = Some(current_output);
        self.cached_composition_key.clone_from(&readback_key);
        self.output_generation = self.output_generation.saturating_add(1);
        self.cached_sample_result = None;
        if !requires_cpu_sampling_canvas && !requires_preview_surface {
            self.stage_frame_in_flight(encoder, None);
            return Ok(gpu_composed_without_surfaces());
        }

        if let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_readback_surface.as_ref()
            && cached.key.as_ref() == Some(key)
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            let cached_surface = cached.surface.clone();
            let submission_index = self.queue.submit(Some(encoder.finish()));
            self.finish_pending_uploads(submission_index);
            self.release_retired_uniform_slots();
            return Ok(gpu_composed_from_surface(
                cached_surface,
                requires_cpu_sampling_canvas,
            ));
        }

        self.read_back_current_output_surface(
            plan.width,
            plan.height,
            readback_key,
            requires_cpu_sampling_canvas,
            preview_surface_request,
            Some(encoder),
            prepared_preview_surface,
        )
    }

    #[cfg(test)]
    pub(crate) fn try_ensure_surface_size(&mut self, width: u32, height: u32) -> Result<()> {
        let replacement = self.prepare_surface_size(width, height)?;
        self.commit_surface_size(replacement);
        Ok(())
    }

    fn prepare_surface_size(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<GpuCompositorSurfaceSet>> {
        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return Ok(None);
        }
        let admission =
            super::gpu_canvas_admission(self.probe.max_texture_dimension_2d, width, height);
        if let super::GpuCanvasAdmission::CpuFallback(reason) = admission {
            anyhow::bail!("{}", reason.message());
        }

        let replacement = self
            .compositor_surface_cache
            .remove(&(width, height))
            .flatten()
            .map_or_else(|| self.try_create_compositor_surface_set(width, height), Ok)?;
        Ok(Some(replacement))
    }

    fn commit_surface_size(&mut self, replacement: Option<GpuCompositorSurfaceSet>) {
        let Some(replacement) = replacement else {
            return;
        };
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("compositor surfaces resized"));
        self.discard_pending_uploads();
        if let Some(previous) = self.surfaces.replace(replacement) {
            self.compositor_surface_cache
                .insert((previous.width, previous.height), Some(previous));
        }
        self.preview_surfaces = None;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
    }

    fn prepare_preview_for_surfaces(
        &mut self,
        source_width: u32,
        source_height: u32,
        request: Option<PreviewSurfaceRequest>,
        requires_cpu_sampling_canvas: bool,
        prepared_surfaces: Option<&GpuCompositorSurfaceSet>,
        force_replace: bool,
    ) -> Result<Option<PreparedPreviewSurfaceChange>> {
        if requires_cpu_sampling_canvas {
            return Ok(None);
        }
        let Some(request) = request else {
            return Ok(None);
        };
        let scale_views = if preview_requires_scale(request, source_width, source_height) {
            let surfaces = prepared_surfaces
                .or(self.surfaces.as_ref())
                .context("GPU preview preparation requires compositor surfaces")?;
            Some((surfaces.front.view.clone(), surfaces.back.view.clone()))
        } else {
            None
        };
        self.prepare_preview_surface_readback(
            source_width,
            source_height,
            request,
            scale_views,
            force_replace,
        )
        .map(Some)
    }

    pub(super) fn layer_reuses_current_output_texture(
        &self,
        layer: &CompositionLayer,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(current_storage_id) = self.current_output.and_then(|output| {
            let surfaces = self.surfaces.as_ref()?;
            Some(match output {
                GpuCompositorOutputSurface::Front => surfaces.front.storage_id,
                GpuCompositorOutputSurface::Back => surfaces.back.storage_id,
            })
        }) else {
            return false;
        };
        layer.mode == CompositionMode::Replace
            && layer.opacity >= 1.0
            && layer.transform.is_none()
            && layer.adjust.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::CompositorOutput
                        && frame.storage_id == current_storage_id
                        && frame.content_generation == self.output_generation
                        && frame.width == width
                        && frame.height == height
            )
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "sibling compose_* methods return Result; keeping this one wrapped preserves call-site uniformity"
    )]
    fn compose_bypass_layer(
        &mut self,
        plan: &CompositionPlan,
        readback_key: Option<CachedReadbackKey>,
        layer: &CompositionLayer,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
        let requires_preview_surface = preview_surface_request.is_some();
        let same_surface_canvas = match &layer.frame {
            ProducerFrame::Canvas(canvas) => {
                self.current_output == Some(GpuCompositorOutputSurface::Front)
                    && self.cached_readback_surface.as_ref().is_some_and(|cached| {
                        cached.surface.width() == plan.width
                            && cached.surface.height() == plan.height
                            && cached.surface.storage_identity() == canvas.storage_identity()
                    })
            }
            ProducerFrame::Surface(_) | ProducerFrame::ScreenPublication(_) => false,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => false,
            ProducerFrame::GpuTexture(_) => false,
        };
        let same_output = readback_key.as_ref().is_some_and(|key| {
            self.current_output == Some(GpuCompositorOutputSurface::Front)
                && self.cached_composition_key.as_ref() == Some(key)
        }) || same_surface_canvas;
        if same_output {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_bypassed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(key) = readback_key.as_ref()
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|_| {
                    preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_bypassed_surface_frame(
                    &surface,
                    requires_cpu_sampling_canvas,
                    requires_preview_surface,
                ));
            }
        }

        let prepared_surface_replacement = self.prepare_surface_size(plan.width, plan.height)?;
        if prepared_surface_replacement.is_none() {
            self.discard_superseded_preview_work();
        }
        self.commit_surface_size(prepared_surface_replacement);
        if let Some(surfaces) = self.surfaces.as_mut() {
            upload_frame_into_cached_texture(
                &self.queue,
                &surfaces.front.texture,
                &mut surfaces.front_contents,
                &layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
            surfaces.back_contents = None;
        }
        self.current_output = Some(GpuCompositorOutputSurface::Front);
        self.cached_composition_key.clone_from(&readback_key);
        if !same_output {
            self.output_generation = self.output_generation.saturating_add(1);
            self.cached_sample_result = None;
        }

        let mut composed = match &layer.frame {
            ProducerFrame::Surface(surface) => gpu_bypassed_surface_frame(
                surface,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
            ProducerFrame::Canvas(canvas) => gpu_bypassed_canvas_frame(
                canvas,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
            ProducerFrame::ScreenPublication(_) => {
                unreachable!("screen publication frames are composed instead of bypassed")
            }
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
            ProducerFrame::GpuTexture(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
        };
        let cached_surface = composed
            .preview_surface
            .as_ref()
            .or(composed.sampling_surface.as_ref())
            .cloned()
            .or_else(|| bypass_preview_surface(&layer.frame));
        self.cached_readback_surface = cached_surface.map(|surface| CachedReadbackSurface {
            key: readback_key,
            surface,
        });
        composed.backend = CompositorBackendKind::Gpu;
        self.ready_preview_surface = None;
        Ok(composed)
    }

    fn read_back_current_output_surface(
        &mut self,
        width: u32,
        height: u32,
        readback_key: Option<CachedReadbackKey>,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
        encoder: Option<wgpu::CommandEncoder>,
        prepared_preview_surface: Option<PreparedPreviewSurfaceChange>,
    ) -> Result<ComposedFrameSet> {
        if requires_cpu_sampling_canvas {
            // Keyed (all-CPU) plans keep their historical behavior: the keyed
            // readback cache and the bypass paths already service them, so
            // the compose encoder is deferred for a later flush. Keyless
            // plans carry GPU producer frames and can only reach the CPU
            // sampler through the one-frame readback latch.
            if readback_key.is_some() {
                if let Some(encoder) = encoder {
                    self.stage_frame_in_flight(encoder, None);
                }
                return Ok(gpu_composed_without_surfaces());
            }
            return self.latch_sampling_surface_readback(width, height, encoder);
        }
        let Some(current_output) = self.current_output else {
            anyhow::bail!("GPU readback requested without a composed output surface");
        };
        if let Some(request) = preview_surface_request {
            let cache_as_full_size = preview_request_matches_plan(Some(request), width, height);
            return self.stage_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                cache_as_full_size,
                encoder,
                prepared_preview_surface,
            );
        }
        if let Some(encoder) = encoder {
            self.stage_frame_in_flight(encoder, None);
        }
        Ok(gpu_composed_without_surfaces())
    }

    /// Services `requires_cpu_sampling_canvas` for keyless plans with a
    /// one-frame readback latch: each compose stages a full-canvas readback
    /// of the freshly composed output and returns the surface staged by the
    /// previous compose. The very first compose returns no canvas — callers
    /// already tolerate one `Unavailable` sampling frame.
    ///
    /// Preview coexistence: when a scaled preview is requested in the same
    /// frame, sampling wins this readback; the full-size sampling canvas is
    /// published as the scene canvas and preview consumers downscale it
    /// CPU-side, so no preview frame is lost — it just skips the GPU scale
    /// pass for that frame.
    ///
    /// Pipeline correctness: the staged copy is appended to the compose
    /// encoder (or the stashed deferred submission) and submitted eagerly,
    /// then resolved with zero-timeout polls on later composes — the render
    /// thread never blocks on this path.
    fn latch_sampling_surface_readback(
        &mut self,
        width: u32,
        height: u32,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        self.resolve_pending_sampling_readback();
        let latched = self
            .sampling_latch
            .latched
            .as_ref()
            .filter(|latched| latched.width == width && latched.height == height)
            .map(|latched| latched.surface.clone());
        self.stage_sampling_surface_readback(width, height, encoder)?;
        Ok(match latched {
            Some(surface) => gpu_composed_from_surface(surface, true),
            None => gpu_composed_without_surfaces(),
        })
    }

    /// Polls the in-flight sampling readback without blocking and moves it
    /// into the latch once its buffer map has completed.
    fn resolve_pending_sampling_readback(&mut self) {
        let Some(pending) = self.sampling_latch.pending.take() else {
            return;
        };
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(pending.submission_index.clone()),
            timeout: Some(Duration::ZERO),
        }) {
            Ok(_) | Err(wgpu::PollError::Timeout) => {}
            Err(error) => {
                tracing::debug!(%error, "GPU sampling latch readiness poll failed");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
        }
        if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
            tracing::debug!(%error, "GPU sampling latch callback poll failed");
            self.unmap_sampling_readback_slot(pending.slot);
            return;
        }
        match pending.receiver.try_recv() {
            Ok(Ok(())) => {}
            Err(TryRecvError::Empty) => {
                self.sampling_latch.pending = Some(pending);
                return;
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "GPU sampling latch buffer mapping failed");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                tracing::debug!("GPU sampling latch channel closed before map completion");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
        }
        let SamplingReadbackLatch {
            buffers,
            latched,
            #[cfg(test)]
            last_readback_bytes,
            ..
        } = &mut self.sampling_latch;
        let Some(buffers) = buffers.as_mut() else {
            return;
        };
        let readback = buffers.readbacks[pending.slot].clone();
        match copy_mapped_readback_buffer_into_surface(
            &readback,
            pending.used_bytes,
            pending.width,
            pending.height,
            buffers.padded_bytes_per_row,
            &mut buffers.surfaces,
            #[cfg(test)]
            last_readback_bytes,
        ) {
            Ok(surface) => {
                *latched = Some(LatchedSamplingSurface {
                    width: pending.width,
                    height: pending.height,
                    output_generation: pending.output_generation,
                    surface,
                });
            }
            Err(error) => {
                tracing::debug!(%error, "GPU sampling latch readback copy failed");
            }
        }
    }

    /// Encodes a full-canvas copy of the current output texture, submits it
    /// together with any pending compose work, and begins the asynchronous
    /// buffer map that the next compose resolves.
    fn stage_sampling_surface_readback(
        &mut self,
        width: u32,
        height: u32,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<()> {
        // A staged preview readback shares the deferred-submission slot.
        // Route it through the preview machinery first so its buffer map
        // still begins before this path claims the queue.
        if self.pending_preview_readback().is_some()
            && self.has_pending_output_submission()
            && let Err(error) = self.submit_pending_preview_work()
        {
            tracing::debug!(%error, "GPU preview submit ahead of sampling readback failed");
        }
        let stashed = self
            .has_pending_output_submission()
            .then(|| self.supersede_frame_in_flight("sampling readback chained"))
            .flatten();
        let encoder = match (encoder, stashed) {
            (Some(encoder), Some(stashed)) => {
                // Submit the stashed encoder first so its work stays ordered
                // ahead of the compose encoder we are extending.
                self.queue.submit(Some(stashed.finish()));
                Some(encoder)
            }
            (Some(encoder), None) | (None, Some(encoder)) => Some(encoder),
            (None, None) => None,
        };

        let generation = self.output_generation;
        let staged_already = self.sampling_latch.pending.as_ref().is_some_and(|pending| {
            pending.output_generation == generation
                && pending.width == width
                && pending.height == height
        }) || self.sampling_latch.latched.as_ref().is_some_and(|latched| {
            latched.output_generation == generation
                && latched.width == width
                && latched.height == height
        });
        let source_texture = self.surfaces.as_ref().and_then(|surfaces| {
            Some(match self.current_output? {
                GpuCompositorOutputSurface::Front => surfaces.front.texture.clone(),
                GpuCompositorOutputSurface::Back => surfaces.back.texture.clone(),
            })
        });
        // Keep at most one readback in flight: an unresolved readback keeps
        // its buffer map outstanding, so the latch keeps serving the older
        // surface until the GPU catches up instead of dropping work.
        if staged_already
            || self.sampling_latch.pending.is_some()
            || width == 0
            || height == 0
            || source_texture.is_none()
        {
            self.submit_sampling_encoder(encoder);
            return Ok(());
        }
        let Some(source_texture) = source_texture else {
            return Ok(());
        };

        self.ensure_sampling_readback_buffers(width, height)?;
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU sampling readback"),
                })
        });
        let buffers = self
            .sampling_latch
            .buffers
            .as_mut()
            .expect("sampling readback buffers should exist after allocation");
        let slot = buffers.next_slot;
        buffers.next_slot = (slot + 1) % SAMPLING_READBACK_SLOT_COUNT;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffers.readbacks[slot],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(buffers.padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            texture_extent(width, height),
        );
        let used_bytes = u64::from(buffers.padded_bytes_per_row).saturating_mul(u64::from(height));
        let readback = buffers.readbacks[slot].clone();
        let submission_index = self.queue.submit(Some(encoder.finish()));
        self.finish_pending_uploads(submission_index.clone());
        self.release_retired_uniform_slots();
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        readback
            .slice(..used_bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.sampling_latch.pending = Some(PendingSamplingReadback {
            width,
            height,
            output_generation: generation,
            slot,
            used_bytes,
            submission_index,
            receiver,
        });
        Ok(())
    }

    fn submit_sampling_encoder(&mut self, encoder: Option<wgpu::CommandEncoder>) {
        if let Some(encoder) = encoder {
            let submission_index = self.queue.submit(Some(encoder.finish()));
            self.finish_pending_uploads(submission_index);
            self.release_retired_uniform_slots();
        }
    }

    pub(super) fn prepare_sampling_readback_buffers(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<SamplingReadbackBuffers>> {
        if self
            .sampling_latch
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.width == width && buffers.height == height)
        {
            return Ok(None);
        }
        let fail_after_prepare = self.take_sampling_readback_failure_injection();
        SamplingReadbackBuffers::try_new(
            &self.device,
            self.max_buffer_size,
            width,
            height,
            fail_after_prepare,
        )
        .map(Some)
    }

    pub(super) fn prepare_sampling_readback_buffers_for_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<SamplingReadbackBuffers>> {
        if self.sampling_latch.buffers.is_none() {
            return Ok(None);
        }
        let fail_after_prepare = self.take_sampling_readback_failure_injection();
        SamplingReadbackBuffers::try_new(
            &self.device,
            self.max_buffer_size,
            width,
            height,
            fail_after_prepare,
        )
        .map(Some)
    }

    fn commit_sampling_readback_buffers(&mut self, replacement: Option<SamplingReadbackBuffers>) {
        let Some(replacement) = replacement else {
            return;
        };
        // The pending readback's mapped buffer belongs to the old set; drop
        // it before replacing the buffers.
        self.discard_pending_sampling_readback();
        self.sampling_latch.buffers = Some(replacement);
    }

    fn ensure_sampling_readback_buffers(&mut self, width: u32, height: u32) -> Result<()> {
        let replacement = self.prepare_sampling_readback_buffers(width, height)?;
        self.commit_sampling_readback_buffers(replacement);
        Ok(())
    }

    pub(super) fn discard_pending_sampling_readback(&mut self) {
        if let Some(pending) = self.sampling_latch.pending.take() {
            self.unmap_sampling_readback_slot(pending.slot);
        }
    }

    pub(super) fn clear_sampling_readback_latch(&mut self) {
        self.discard_pending_sampling_readback();
        self.sampling_latch.latched = None;
        self.sampling_latch.buffers = None;
    }

    fn unmap_sampling_readback_slot(&mut self, slot: usize) {
        if let Some(buffers) = self.sampling_latch.buffers.as_ref() {
            buffers.readbacks[slot].unmap();
        }
    }
}

fn compose_layer_into_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: &CompositionLayer,
    uploaded_screen_frame: Option<&GpuTextureFrame>,
    use_front_as_current: bool,
) -> Result<()> {
    let shader_mode = if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        ComposeShaderMode::Replace
    } else {
        match layer.mode {
            CompositionMode::Replace | CompositionMode::Alpha => ComposeShaderMode::Alpha,
            CompositionMode::Add => ComposeShaderMode::Add,
            CompositionMode::Screen => ComposeShaderMode::Screen,
            CompositionMode::Multiply => ComposeShaderMode::Multiply,
            CompositionMode::Overlay => ComposeShaderMode::Overlay,
            CompositionMode::SoftLight => ComposeShaderMode::SoftLight,
            CompositionMode::ColorDodge => ComposeShaderMode::ColorDodge,
            CompositionMode::Difference => ComposeShaderMode::Difference,
            CompositionMode::Tint => ComposeShaderMode::Tint,
            CompositionMode::LumaReveal => ComposeShaderMode::LumaReveal,
        }
    };
    let output_surface = if use_front_as_current {
        GpuCompositorOutputSurface::Back
    } else {
        GpuCompositorOutputSurface::Front
    };

    let gpu_frame = uploaded_screen_frame
        .map(super::source::GpuSourceFrame::Texture)
        .or_else(|| gpu_source_frame(&layer.frame));

    if let Some(frame) = gpu_frame.as_ref()
        && shader_mode == ComposeShaderMode::Replace
        && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
    {
        if uploaded_screen_frame.is_none() {
            record_gpu_source_upload_skipped();
        }
        let output = if use_front_as_current {
            &surfaces.back
        } else {
            &surfaces.front
        };
        copy_gpu_source_frame_into_texture(
            device,
            queue,
            pipeline,
            encoder,
            &mut surfaces.pending_upload_buffers,
            &mut surfaces.source_copy_bind_groups,
            frame,
            output,
        );
        set_texture_contents(
            surfaces,
            output_surface,
            uploaded_screen_frame
                .as_ref()
                .and_then(|_| cached_source_upload(&layer.frame)),
        );
        return Ok(());
    }

    if gpu_frame.is_none() {
        upload_frame_into_source_texture(device, encoder, surfaces, &layer.frame);
        if shader_mode == ComposeShaderMode::Replace
            && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
        {
            let output_texture = if use_front_as_current {
                &surfaces.back.texture
            } else {
                &surfaces.front.texture
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &surfaces.source.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: output_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                texture_extent(surfaces.width, surfaces.height),
            );
            set_texture_contents(surfaces, output_surface, cached_source_upload(&layer.frame));
            return Ok(());
        }
    }

    let source_flip_y = gpu_frame
        .as_ref()
        .is_some_and(super::source::GpuSourceFrame::flip_y_on_shader_copy);
    let params = encode_compose_params(
        surfaces.width,
        surfaces.height,
        shader_mode,
        layer,
        source_flip_y,
    );
    let params_offset =
        encode_compose_params_upload(device, queue, pipeline, surfaces, encoder, &params);
    #[cfg(test)]
    {
        surfaces.compose_dispatch_count = surfaces.compose_dispatch_count.saturating_add(1);
    }
    if let Some(frame) = gpu_frame.as_ref() {
        if uploaded_screen_frame.is_none() {
            record_gpu_source_upload_skipped();
        }
        let (width, height) = (surfaces.width, surfaces.height);
        let projected_source = uploaded_screen_frame.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::ProjectionSnapshot
            );
        let bind_group = {
            let GpuCompositorSurfaceSet {
                generation,
                front,
                back,
                source,
                compose_source_bind_groups,
                ..
            } = surfaces;
            let (current_view, output_view) = if use_front_as_current {
                (&front.view, &back.view)
            } else {
                (&back.view, &front.view)
            };
            let _ = source;
            let source_view = frame.view();
            let source_storage_id = frame.cached_display_source_copy().storage_id;
            if projected_source {
                compose_source_bind_groups
                    .get_projected(
                        *generation,
                        source_storage_id,
                        source_view,
                        use_front_as_current,
                    )
                    .context("projected source bind group was not admitted")?
            } else {
                compose_source_bind_groups.get_or_create_transient(
                    device,
                    pipeline,
                    *generation,
                    source_storage_id,
                    source_view,
                    use_front_as_current,
                    current_view,
                    output_view,
                    #[cfg(target_os = "windows")]
                    frame.screen_target_lifetime(),
                )
            }
        };
        dispatch_compose_pass(encoder, pipeline, &bind_group, params_offset, width, height);
        set_texture_contents(surfaces, output_surface, None);
        return Ok(());
    }

    let bind_group = if use_front_as_current {
        &surfaces.bind_groups.front_to_back
    } else {
        &surfaces.bind_groups.back_to_front
    };
    dispatch_compose_pass(
        encoder,
        pipeline,
        bind_group,
        params_offset,
        surfaces.width,
        surfaces.height,
    );
    set_texture_contents(surfaces, output_surface, None);
    Ok(())
}

fn return_screen_frame_scratch(
    surfaces: &mut GpuCompositorSurfaceSet,
    scratch: &mut Option<Vec<Option<GpuTextureFrame>>>,
) {
    if let Some(mut frames) = scratch.take() {
        frames.clear();
        surfaces.uploaded_screen_frame_scratch = frames;
    }
}

fn upload_screen_layers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surfaces: &mut GpuCompositorSurfaceSet,
    layers: &[CompositionLayer],
) -> Result<()> {
    let mut uploaded = std::mem::take(&mut surfaces.uploaded_screen_frame_scratch);
    uploaded.clear();
    let prior_capacity = uploaded.capacity();
    let result = (|| -> Result<()> {
        if prior_capacity < layers.len() {
            uploaded.try_reserve_exact(layers.len() - prior_capacity)?;
        }
        for layer in layers {
            let ProducerFrame::ScreenPublication(publication) = &layer.frame else {
                uploaded.push(None);
                continue;
            };
            let surface = publication.surface();
            let GpuCompositorSurfaceSet {
                screen_upload_pool,
                compose_source_bind_groups,
                ..
            } = surfaces;
            let content_key = ScreenUploadContentKey::new(
                publication.plan_generation(),
                publication.descriptor_identity(),
                publication.branch_sequence(),
                surface.extent().width(),
                surface.extent().height(),
            );
            let (frame, wrote_texture) = screen_upload_pool.upload_rgba(
                device,
                queue,
                surface.extent().width(),
                surface.extent().height(),
                surface.pixels(),
                content_key,
                |storage_id| compose_source_bind_groups.release_source(storage_id),
            )?;
            #[cfg(test)]
            if wrote_texture {
                surfaces.source_upload_count = surfaces.source_upload_count.saturating_add(1);
            }
            #[cfg(not(test))]
            let _ = wrote_texture;
            uploaded.push(Some(frame));
        }
        Ok(())
    })();
    #[cfg(test)]
    if uploaded.capacity() > prior_capacity {
        surfaces.screen_layer_host_allocation_count = surfaces
            .screen_layer_host_allocation_count
            .saturating_add(1);
    }
    surfaces.uploaded_screen_frame_scratch = uploaded;
    result
}

fn screen_upload_content_keys(
    layers: &[CompositionLayer],
) -> impl Iterator<Item = ScreenUploadContentKey> + '_ {
    layers.iter().filter_map(|layer| {
        let ProducerFrame::ScreenPublication(publication) = &layer.frame else {
            return None;
        };
        let extent = publication.surface().extent();
        Some(ScreenUploadContentKey::new(
            publication.plan_generation(),
            publication.descriptor_identity(),
            publication.branch_sequence(),
            extent.width(),
            extent.height(),
        ))
    })
}

fn dispatch_compose_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &GpuCompositorPipeline,
    bind_group: &wgpu::BindGroup,
    params_offset: u32,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, bind_group, &[params_offset]);
    pass.dispatch_workgroups(
        width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
}

fn set_texture_contents(
    surfaces: &mut GpuCompositorSurfaceSet,
    output: GpuCompositorOutputSurface,
    contents: Option<CachedSourceUpload>,
) {
    match output {
        GpuCompositorOutputSurface::Front => surfaces.front_contents = contents,
        GpuCompositorOutputSurface::Back => surfaces.back_contents = contents,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ComposeSourceBindGroupKey {
    target_generation: u64,
    source_storage_id: u64,
    front_as_current: bool,
}

pub(crate) struct PreparedProjectedComposeBindGroups {
    target_generation: u64,
    entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    retired_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
}

impl PreparedProjectedComposeBindGroups {
    pub(super) fn empty(target_generation: u64) -> Self {
        Self {
            target_generation,
            entries: HashMap::new(),
            retired_entries: HashMap::new(),
        }
    }
}

#[derive(Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the suffix distinguishes active, retired, and transient entry ownership"
)]
pub(super) struct ComposeSourceBindGroupCache {
    projected_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    retired_projected_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    transient_entries: VecDeque<CachedComposeSourceBindGroup>,
    #[cfg(test)]
    pub(super) creation_count: usize,
}

#[derive(Clone)]
struct CachedComposeSourceBindGroup {
    key: ComposeSourceBindGroupKey,
    source_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    source_lease: Option<Weak<GpuTextureFrameLease>>,
    #[cfg(target_os = "windows")]
    screen_target_lifetime: Option<ScreenResourceLifetime>,
}

const COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP: usize = 4;

impl ComposeSourceBindGroupCache {
    pub(super) fn prepare_projected(
        &self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        target_generation: u64,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        source_count: usize,
        sources: impl IntoIterator<Item = (u64, wgpu::TextureView, Weak<GpuTextureFrameLease>)>,
    ) -> std::result::Result<
        (PreparedProjectedComposeBindGroups, usize),
        std::collections::TryReserveError,
    > {
        let mut entries = HashMap::new();
        entries.try_reserve(source_count.saturating_mul(2))?;
        let mut creation_count = 0_usize;
        for (source_storage_id, source_view, source_lease) in sources {
            for front_as_current in [true, false] {
                let key = ComposeSourceBindGroupKey {
                    target_generation,
                    source_storage_id,
                    front_as_current,
                };
                let cached = self
                    .projected_entries
                    .get(&key)
                    .filter(|cached| cached.source_view == source_view);
                let entry = if let Some(cached) = cached {
                    cached.clone()
                } else {
                    let (current_view, output_view) = if front_as_current {
                        (front_view, back_view)
                    } else {
                        (back_view, front_view)
                    };
                    creation_count = creation_count.saturating_add(1);
                    CachedComposeSourceBindGroup {
                        key,
                        source_view: source_view.clone(),
                        bind_group: create_compose_bind_group(
                            device,
                            pipeline,
                            current_view,
                            &source_view,
                            output_view,
                            "SparkleFlinger admitted projected-source bind group",
                        ),
                        source_lease: Some(source_lease.clone()),
                        #[cfg(target_os = "windows")]
                        screen_target_lifetime: None,
                    }
                };
                entries.insert(key, entry);
            }
        }
        let mut retired_entries = HashMap::new();
        retired_entries.try_reserve(
            self.retired_projected_entries
                .len()
                .saturating_add(self.projected_entries.len()),
        )?;
        for (key, entry) in &self.retired_projected_entries {
            let remains_leased = entry
                .source_lease
                .as_ref()
                .is_some_and(|lease| lease.strong_count() > 0);
            if remains_leased && !entries.contains_key(key) {
                retired_entries.insert(*key, entry.clone());
            }
        }
        for (key, entry) in &self.projected_entries {
            let has_external_lease = entry
                .source_lease
                .as_ref()
                .is_some_and(|lease| lease.strong_count() > 1);
            if has_external_lease && !entries.contains_key(key) {
                retired_entries.insert(*key, entry.clone());
            }
        }
        Ok((
            PreparedProjectedComposeBindGroups {
                target_generation,
                entries,
                retired_entries,
            },
            creation_count,
        ))
    }

    pub(super) fn install_projected(
        &mut self,
        prepared: PreparedProjectedComposeBindGroups,
        target_generation: u64,
    ) {
        debug_assert_eq!(prepared.target_generation, target_generation);
        debug_assert!(
            prepared
                .entries
                .keys()
                .all(|key| key.target_generation == target_generation)
        );
        self.projected_entries = prepared.entries;
        self.retired_projected_entries = prepared.retired_entries;
    }

    pub(super) fn clear_projected(&mut self) {
        self.projected_entries.clear();
        self.retired_projected_entries.clear();
    }

    fn get_projected(
        &self,
        target_generation: u64,
        source_storage_id: u64,
        source_view: &wgpu::TextureView,
        front_as_current: bool,
    ) -> Option<wgpu::BindGroup> {
        let key = ComposeSourceBindGroupKey {
            target_generation,
            source_storage_id,
            front_as_current,
        };
        exact_projected_entry(
            &self.projected_entries,
            &self.retired_projected_entries,
            &key,
        )
        .filter(|cached| cached.source_view == *source_view)
        .map(|cached| cached.bind_group.clone())
    }

    fn get_or_create_transient(
        &mut self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        target_generation: u64,
        source_storage_id: u64,
        source_view: &wgpu::TextureView,
        front_as_current: bool,
        current_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        #[cfg(target_os = "windows")] screen_target_lifetime: Option<&ScreenResourceLifetime>,
    ) -> wgpu::BindGroup {
        let key = ComposeSourceBindGroupKey {
            target_generation,
            source_storage_id,
            front_as_current,
        };
        if let Some(cached) = self
            .transient_entries
            .iter()
            .find(|cached| cached.key == key && cached.source_view == *source_view)
        {
            return cached.bind_group.clone();
        }
        let bind_group = create_compose_bind_group(
            device,
            pipeline,
            current_view,
            source_view,
            output_view,
            "SparkleFlinger GPU imported producer bind group",
        );
        #[cfg(test)]
        {
            self.creation_count = self.creation_count.saturating_add(1);
        }
        if self.transient_entries.len() >= COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP {
            self.transient_entries.pop_front();
        }
        self.transient_entries
            .push_back(CachedComposeSourceBindGroup {
                key,
                source_view: source_view.clone(),
                bind_group: bind_group.clone(),
                source_lease: None,
                #[cfg(target_os = "windows")]
                screen_target_lifetime: screen_target_lifetime.cloned(),
            });
        bind_group
    }

    fn release_source(&mut self, source_storage_id: u64) {
        self.projected_entries
            .retain(|key, _| key.source_storage_id != source_storage_id);
        self.retired_projected_entries
            .retain(|key, _| key.source_storage_id != source_storage_id);
        self.transient_entries
            .retain(|entry| entry.key.source_storage_id != source_storage_id);
    }

    #[cfg(target_os = "windows")]
    pub(super) fn release_native_screen_entries(&mut self) {
        self.projected_entries
            .retain(|_, entry| entry.screen_target_lifetime.is_none());
        self.retired_projected_entries
            .retain(|_, entry| entry.screen_target_lifetime.is_none());
        self.transient_entries
            .retain(|entry| entry.screen_target_lifetime.is_none());
    }

    #[cfg(test)]
    pub(super) fn projected_entry_count(&self) -> usize {
        self.projected_entries.len()
    }

    #[cfg(test)]
    pub(super) fn projected_source_storage_ids(&self) -> Vec<u64> {
        let mut source_storage_ids = self
            .projected_entries
            .keys()
            .map(|key| key.source_storage_id)
            .collect::<Vec<_>>();
        source_storage_ids.sort_unstable();
        source_storage_ids.dedup();
        source_storage_ids
    }

    #[cfg(test)]
    pub(super) fn retired_projected_entry_count(&self) -> usize {
        self.retired_projected_entries.len()
    }
}

fn exact_projected_entry<'a, T>(
    entries: &'a HashMap<ComposeSourceBindGroupKey, T>,
    retired_entries: &'a HashMap<ComposeSourceBindGroupKey, T>,
    key: &ComposeSourceBindGroupKey,
) -> Option<&'a T> {
    entries.get(key).or_else(|| retired_entries.get(key))
}

fn has_screen_upload_layers(layers: &[CompositionLayer]) -> bool {
    screen_upload_content_keys(layers).next().is_some()
}

#[cfg(feature = "allocation-contract-tests")]
pub(crate) struct ProjectedLookupAllocationFixture {
    entries: HashMap<ComposeSourceBindGroupKey, std::sync::Arc<()>>,
    retired_entries: HashMap<ComposeSourceBindGroupKey, std::sync::Arc<()>>,
    keys: Vec<ComposeSourceBindGroupKey>,
    zero_screen_layers: [CompositionLayer; 2],
}

#[cfg(feature = "allocation-contract-tests")]
impl ProjectedLookupAllocationFixture {
    pub(crate) fn new(source_count: usize) -> Self {
        let target_generation = 41;
        let mut entries = HashMap::with_capacity(source_count.saturating_mul(2));
        let mut keys = Vec::with_capacity(source_count.saturating_mul(2));
        for source_storage_id in 1..=u64::try_from(source_count).unwrap_or(u64::MAX) {
            for front_as_current in [true, false] {
                let key = ComposeSourceBindGroupKey {
                    target_generation,
                    source_storage_id,
                    front_as_current,
                };
                entries.insert(key, std::sync::Arc::new(()));
                keys.push(key);
            }
        }
        Self {
            entries,
            retired_entries: HashMap::new(),
            keys,
            zero_screen_layers: [
                CompositionLayer::replace_opaque(ProducerFrame::Canvas(
                    hypercolor_core::types::canvas::Canvas::new(4, 4),
                )),
                CompositionLayer::replace_opaque(ProducerFrame::Canvas(
                    hypercolor_core::types::canvas::Canvas::new(4, 4),
                )),
            ],
        }
    }

    pub(crate) fn run_round(&self) -> bool {
        let mut hits = 0_usize;
        for key in &self.keys {
            if exact_projected_entry(&self.entries, &self.retired_entries, key)
                .map(std::sync::Arc::clone)
                .is_some()
            {
                hits = hits.saturating_add(1);
            }
        }
        hits == self.keys.len() && !has_screen_upload_layers(&self.zero_screen_layers)
    }
}

pub(super) fn create_compose_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    current: &wgpu::TextureView,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.compose_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(current),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.compose_params.binding(),
            },
        ],
    })
}

/// Uploads compose params into the uniform ring and returns the dynamic
/// offset the dispatch must bind. Byte-identical params re-bind the previous
/// slot without writing, as long as that slot came from a ring write.
fn encode_compose_params_upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    params: &[u8; COMPOSE_PARAM_BYTES],
) -> u32 {
    if surfaces.cached_compose_params.as_ref() == Some(params)
        && let Some(offset) = surfaces.cached_compose_params_offset
    {
        pipeline.compose_params.pin_last_slot();
        return offset;
    }
    let write = pipeline.compose_params.write(
        device,
        queue,
        encoder,
        &mut surfaces.pending_upload_buffers,
        params,
    );
    surfaces.cached_compose_params = Some(*params);
    surfaces.cached_compose_params_offset = write.reusable.then_some(write.offset);
    #[cfg(test)]
    {
        surfaces.compose_param_write_count = surfaces.compose_param_write_count.saturating_add(1);
    }
    write.offset
}

pub(super) fn encode_compose_params(
    width: u32,
    height: u32,
    mode: ComposeShaderMode,
    layer: &CompositionLayer,
    source_flip_y: bool,
) -> [u8; COMPOSE_PARAM_BYTES] {
    let mut bytes = [0u8; COMPOSE_PARAM_BYTES];
    let transform = layer.transform.unwrap_or_default();
    let adjust = layer.adjust.unwrap_or_default();
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&(mode as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(fit_mode(transform.fit) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&layer.frame.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&layer.frame.height().to_le_bytes());
    let processing = if layer.needs_processing_for_size(width, height) {
        1_u32
    } else {
        0_u32
    };
    bytes[24..28].copy_from_slice(&processing.to_le_bytes());
    bytes[28..32].copy_from_slice(&u32::from(source_flip_y).to_le_bytes());
    bytes[32..36].copy_from_slice(&layer.opacity.to_le_bytes());
    bytes[36..40].copy_from_slice(&transform.anchor.x.to_le_bytes());
    bytes[40..44].copy_from_slice(&transform.anchor.y.to_le_bytes());
    bytes[44..48].copy_from_slice(&transform.scale[0].to_le_bytes());
    bytes[48..52].copy_from_slice(&transform.scale[1].to_le_bytes());
    bytes[52..56].copy_from_slice(&transform.rotation.cos().to_le_bytes());
    bytes[56..60].copy_from_slice(&transform.rotation.sin().to_le_bytes());
    let sample_target_space = if transform.sample_target_space {
        1.0_f32
    } else {
        0.0_f32
    };
    bytes[60..64].copy_from_slice(&sample_target_space.to_le_bytes());
    bytes[64..68].copy_from_slice(&adjust.brightness.to_le_bytes());
    bytes[68..72].copy_from_slice(&adjust.saturation.to_le_bytes());
    bytes[72..76].copy_from_slice(&adjust.hue_shift.to_le_bytes());
    let tint_strength = (adjust.tint_strength * adjust.tint[3].clamp(0.0, 1.0)).clamp(0.0, 1.0);
    bytes[76..80].copy_from_slice(&tint_strength.to_le_bytes());
    bytes[80..84].copy_from_slice(&adjust.tint[0].to_le_bytes());
    bytes[84..88].copy_from_slice(&adjust.tint[1].to_le_bytes());
    bytes[88..92].copy_from_slice(&adjust.tint[2].to_le_bytes());
    bytes[92..96].copy_from_slice(&adjust.contrast.to_le_bytes());
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
    Multiply = 4,
    Overlay = 5,
    SoftLight = 6,
    ColorDodge = 7,
    Difference = 8,
    Tint = 9,
    LumaReveal = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeFitMode {
    Contain = 0,
    Cover = 1,
    Stretch = 2,
    Tile = 3,
    Mirror = 4,
}

fn fit_mode(mode: hypercolor_types::viewport::FitMode) -> ComposeFitMode {
    match mode {
        hypercolor_types::viewport::FitMode::Contain => ComposeFitMode::Contain,
        hypercolor_types::viewport::FitMode::Cover => ComposeFitMode::Cover,
        hypercolor_types::viewport::FitMode::Stretch => ComposeFitMode::Stretch,
        hypercolor_types::viewport::FitMode::Tile => ComposeFitMode::Tile,
        hypercolor_types::viewport::FitMode::Mirror => ComposeFitMode::Mirror,
    }
}
