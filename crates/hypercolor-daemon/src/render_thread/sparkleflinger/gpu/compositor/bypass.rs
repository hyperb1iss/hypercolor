use anyhow::{Context, Result};

use super::super::super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
};
use super::super::frame_set::{
    gpu_bypassed_canvas_frame, gpu_bypassed_surface_frame, gpu_bypassed_without_surfaces,
    gpu_composed_with_preview_surface,
};
use super::super::preview::{
    CachedPreviewSurfaceKey, PreparedPreviewSurfaceChange, bypass_preview_surface,
    preview_request_matches_plan, preview_requires_scale,
};
use super::super::readback::{CachedReadbackKey, CachedReadbackSurface};
use super::super::source::upload_frame_into_cached_texture;
use super::super::{GpuCompositorOutputSurface, GpuCompositorSurfaceSet, GpuSparkleFlinger};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::{GpuTextureFrameOrigin, ProducerFrame};

impl GpuSparkleFlinger {
    #[cfg(test)]
    pub(crate) fn try_ensure_surface_size(&mut self, width: u32, height: u32) -> Result<()> {
        let replacement = self.prepare_surface_size(width, height)?;
        self.commit_surface_size(replacement);
        Ok(())
    }

    pub(super) fn prepare_surface_size(
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
            super::super::gpu_canvas_admission(self.probe.max_texture_dimension_2d, width, height);
        if let super::super::GpuCanvasAdmission::CpuFallback(reason) = admission {
            anyhow::bail!("{}", reason.message());
        }

        let replacement = self
            .compositor_surface_cache
            .remove(&(width, height))
            .flatten()
            .map_or_else(|| self.try_create_compositor_surface_set(width, height), Ok)?;
        Ok(Some(replacement))
    }

    pub(super) fn commit_surface_size(&mut self, replacement: Option<GpuCompositorSurfaceSet>) {
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

    pub(super) fn prepare_preview_for_surfaces(
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

    pub(in crate::render_thread::sparkleflinger::gpu) fn layer_reuses_current_output_texture(
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
    pub(super) fn compose_bypass_layer(
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
            ProducerFrame::ScreenPublication(publication) => gpu_bypassed_surface_frame(
                publication.published_surface(),
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
}
