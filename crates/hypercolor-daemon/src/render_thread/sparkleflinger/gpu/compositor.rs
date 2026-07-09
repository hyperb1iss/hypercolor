use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{PublishedSurface, RenderSurfacePool, SurfaceDescriptor};

use super::super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
};
use super::frame_set::{
    gpu_bypassed_canvas_frame, gpu_bypassed_surface_frame, gpu_bypassed_without_surfaces,
    gpu_composed_from_surface, gpu_composed_with_preview_surface, gpu_composed_without_surfaces,
};
use super::preview::{
    CachedPreviewSurfaceKey, bypass_preview_surface, preview_request_matches_plan,
};
use super::readback::{
    CachedReadbackKey, CachedReadbackSurface, copy_mapped_readback_buffer_into_surface,
};
use super::source::{
    CachedSourceUpload, cached_readback_key, cached_source_upload, copy_frame_into_output_texture,
    copy_gpu_source_frame_into_texture, gpu_source_frame, upload_frame_into_cached_texture,
    upload_frame_into_source_texture,
};
use super::telemetry::record_gpu_source_upload_skipped;
use super::{
    COMPOSE_PARAM_BYTES, COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH,
    GpuCompositorOutputSurface, GpuCompositorPipeline, GpuCompositorSurfaceSet, GpuSparkleFlinger,
    padded_bytes_per_row, texture_extent,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::{GpuTextureFrameOrigin, ProducerFrame};

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

struct SamplingReadbackBuffers {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    readbacks: [wgpu::Buffer; SAMPLING_READBACK_SLOT_COUNT],
    next_slot: usize,
    surfaces: RenderSurfacePool,
}

impl SamplingReadbackBuffers {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let size = u64::from(padded_bytes_per_row).saturating_mul(u64::from(height));
        Self {
            width,
            height,
            padded_bytes_per_row,
            readbacks: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU sampling latch readback"),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            next_slot: 0,
            surfaces: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(width, height),
                SAMPLING_READBACK_SURFACE_SLOTS,
            ),
        }
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
    pub(crate) fn compose(
        &mut self,
        plan: &CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
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
            );
        }
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

        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width != plan.width || current_height != plan.height
        ) {
            self.discard_superseded_preview_work();
        }
        self.ensure_surface_size(plan.width, plan.height);
        if let Some(key) = readback_key.as_ref()
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
            );
        }
        if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
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
        let mut layers = plan.layers.iter();
        let first_layer = layers
            .next()
            .context("GPU composition requires at least one layer")?;

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
            compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &mut self.pipeline,
                surfaces,
                &mut encoder,
                first_layer,
                true,
            );
            use_front_as_current = false;
        }

        for layer in layers {
            compose_layer_into_gpu(
                &self.device,
                &self.queue,
                &mut self.pipeline,
                surfaces,
                &mut encoder,
                layer,
                use_front_as_current,
            );
            use_front_as_current = !use_front_as_current;
        }

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
            self.queue.submit(Some(encoder.finish()));
            self.clear_pending_upload_buffers();
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
        )
    }

    pub(crate) fn ensure_surface_size(&mut self, width: u32, height: u32) {
        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return;
        }

        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("compositor surfaces resized"));
        self.surfaces = Some(GpuCompositorSurfaceSet::new(
            &self.device,
            &self.pipeline,
            width,
            height,
        ));
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

    fn layer_reuses_current_output_texture(
        &self,
        layer: &CompositionLayer,
        width: u32,
        height: u32,
    ) -> bool {
        self.current_output.is_some()
            && layer.mode == CompositionMode::Replace
            && layer.opacity >= 1.0
            && layer.transform.is_none()
            && layer.adjust.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::CompositorOutput
                        && frame.storage_id == self.output_generation
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
            ProducerFrame::Surface(_) => false,
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

        self.discard_superseded_preview_work();
        self.ensure_surface_size(plan.width, plan.height);
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
            return Ok(self.latch_sampling_surface_readback(width, height, encoder));
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
    ) -> ComposedFrameSet {
        self.resolve_pending_sampling_readback();
        let latched = self
            .sampling_latch
            .latched
            .as_ref()
            .filter(|latched| latched.width == width && latched.height == height)
            .map(|latched| latched.surface.clone());
        self.stage_sampling_surface_readback(width, height, encoder);
        match latched {
            Some(surface) => gpu_composed_from_surface(surface, true),
            None => gpu_composed_without_surfaces(),
        }
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
    ) {
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
            return;
        }
        let Some(source_texture) = source_texture else {
            return;
        };

        self.ensure_sampling_readback_buffers(width, height);
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
        self.clear_pending_upload_buffers();
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
    }

    fn submit_sampling_encoder(&mut self, encoder: Option<wgpu::CommandEncoder>) {
        if let Some(encoder) = encoder {
            self.queue.submit(Some(encoder.finish()));
            self.clear_pending_upload_buffers();
            self.release_retired_uniform_slots();
        }
    }

    fn ensure_sampling_readback_buffers(&mut self, width: u32, height: u32) {
        if self
            .sampling_latch
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.width == width && buffers.height == height)
        {
            return;
        }
        // The pending readback's mapped buffer belongs to the old set; drop
        // it before replacing the buffers.
        self.discard_pending_sampling_readback();
        self.sampling_latch.buffers =
            Some(SamplingReadbackBuffers::new(&self.device, width, height));
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
    use_front_as_current: bool,
) {
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

    if let Some(frame) = gpu_source_frame(&layer.frame)
        && shader_mode == ComposeShaderMode::Replace
        && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
    {
        record_gpu_source_upload_skipped();
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
            &frame,
            output,
        );
        set_texture_contents(surfaces, output_surface, None);
        return;
    }

    let gpu_frame = gpu_source_frame(&layer.frame);

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
            return;
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
    if let Some(frame) = gpu_frame {
        record_gpu_source_upload_skipped();
        let (width, height) = (surfaces.width, surfaces.height);
        let bind_group = {
            let GpuCompositorSurfaceSet {
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
            compose_source_bind_groups.get_or_create(
                device,
                pipeline,
                source_view,
                use_front_as_current,
                current_view,
                output_view,
            )
        };
        dispatch_compose_pass(encoder, pipeline, &bind_group, params_offset, width, height);
        set_texture_contents(surfaces, output_surface, None);
        return;
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

/// Compose bind groups for GPU producer layers, cached by source-view
/// identity and ping-pong direction. The current/output views are the
/// surface set's own front/back textures, so direction fully determines
/// them; entries die with the surface set on resize. Cached views keep the
/// producer textures alive, bounded by the cache cap.
#[derive(Default)]
pub(super) struct ComposeSourceBindGroupCache {
    entries: Vec<CachedComposeSourceBindGroup>,
    #[cfg(test)]
    pub(super) creation_count: usize,
}

struct CachedComposeSourceBindGroup {
    source_view: wgpu::TextureView,
    front_as_current: bool,
    bind_group: wgpu::BindGroup,
}

const COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP: usize = 4;

impl ComposeSourceBindGroupCache {
    fn get_or_create(
        &mut self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        source_view: &wgpu::TextureView,
        front_as_current: bool,
        current_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        if let Some(cached) = self.entries.iter().find(|cached| {
            cached.front_as_current == front_as_current && cached.source_view == *source_view
        }) {
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
        if self.entries.len() >= COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries.push(CachedComposeSourceBindGroup {
            source_view: source_view.clone(),
            front_as_current,
            bind_group: bind_group.clone(),
        });
        bind_group
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

fn encode_compose_params(
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
enum ComposeShaderMode {
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
