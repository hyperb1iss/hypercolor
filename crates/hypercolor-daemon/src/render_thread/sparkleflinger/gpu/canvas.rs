use anyhow::{Context, Result};

use super::super::PreviewSurfaceRequest;
use super::preview;
use super::source;
use super::{
    GpuCanvasGeneration, GpuCompositorSurfaceSet, GpuCompositorTexture, GpuImmutableSceneSnapshot,
    GpuSparkleFlinger, IMMUTABLE_SCENE_GENERATIONS_IN_FLIGHT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GpuCanvasFallbackReason {
    InvalidExtent,
    TextureDimension,
    ResourceAllocation,
}

impl GpuCanvasFallbackReason {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::InvalidExtent => "canvas extent is empty or not representable",
            Self::TextureDimension => "canvas extent exceeds the GPU texture dimension limit",
            Self::ResourceAllocation => "GPU canvas resources could not be admitted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GpuCanvasAdmission {
    Gpu,
    CpuFallback(GpuCanvasFallbackReason),
}

pub(super) fn gpu_canvas_admission(
    max_texture_dimension_2d: u32,
    width: u32,
    height: u32,
) -> GpuCanvasAdmission {
    if width == 0 || height == 0 {
        return GpuCanvasAdmission::CpuFallback(GpuCanvasFallbackReason::InvalidExtent);
    }
    if width > max_texture_dimension_2d || height > max_texture_dimension_2d {
        return GpuCanvasAdmission::CpuFallback(GpuCanvasFallbackReason::TextureDimension);
    }
    GpuCanvasAdmission::Gpu
}

pub(crate) enum GpuCanvasPreparation {
    Gpu {
        generation: GpuCanvasGeneration,
        immutable_scene_snapshots: Vec<GpuImmutableSceneSnapshot>,
        opaque_black_texture: GpuCompositorTexture,
    },
    CpuFallback,
}

impl GpuCanvasPreparation {
    pub(in crate::render_thread::sparkleflinger) const fn is_admitted(&self) -> bool {
        matches!(self, Self::Gpu { .. })
    }

    pub(in crate::render_thread::sparkleflinger) const fn cpu_fallback() -> Self {
        Self::CpuFallback
    }

    pub(super) fn compositor_surfaces(&self) -> Option<&GpuCompositorSurfaceSet> {
        match self {
            Self::Gpu { generation, .. } => Some(&generation.surfaces),
            Self::CpuFallback => None,
        }
    }
}

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger) fn prepare_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
        active_preview_request: Option<PreviewSurfaceRequest>,
    ) -> GpuCanvasPreparation {
        let admission = gpu_canvas_admission(self.probe.max_texture_dimension_2d, width, height);
        match admission {
            GpuCanvasAdmission::Gpu => {}
            GpuCanvasAdmission::CpuFallback(reason) => {
                tracing::info!(
                    width,
                    height,
                    reason = reason.message(),
                    "using CPU compositor for canvas extent"
                );
                return GpuCanvasPreparation::CpuFallback;
            }
        }
        let preparation = (|| {
            let generation =
                self.prepare_gpu_canvas_generation(width, height, active_preview_request)?;
            let immutable_scene_snapshots = (0..IMMUTABLE_SCENE_GENERATIONS_IN_FLIGHT)
                .map(|_| GpuImmutableSceneSnapshot::try_new(&self.device, width, height))
                .collect::<Result<Vec<_>>>()?;
            let opaque_black_texture =
                GpuCompositorTexture::try_new(&self.device, 1, 1, "SparkleFlinger Opaque Black")
                    .context("GPU opaque-black base texture allocation failed")?;
            source::write_rgba_texture(
                &self.queue,
                &opaque_black_texture.texture,
                1,
                1,
                &[0, 0, 0, 255],
            );
            #[cfg(test)]
            self.snapshot_texture_allocation_count.set(
                self.snapshot_texture_allocation_count
                    .get()
                    .saturating_add(immutable_scene_snapshots.len()),
            );
            Ok::<_, anyhow::Error>(GpuCanvasPreparation::Gpu {
                generation,
                immutable_scene_snapshots,
                opaque_black_texture,
            })
        })();
        match preparation {
            Ok(preparation) => preparation,
            Err(error) => {
                tracing::warn!(
                    %error,
                    width,
                    height,
                    reason = GpuCanvasFallbackReason::ResourceAllocation.message(),
                    "using CPU compositor after GPU canvas admission failed"
                );
                GpuCanvasPreparation::CpuFallback
            }
        }
    }

    fn prepare_gpu_canvas_generation(
        &mut self,
        width: u32,
        height: u32,
        active_preview_request: Option<PreviewSurfaceRequest>,
    ) -> Result<GpuCanvasGeneration> {
        let surfaces = self
            .compositor_surface_cache
            .remove(&(width, height))
            .flatten()
            .map_or_else(|| self.try_create_compositor_surface_set(width, height), Ok)?;
        let sampling_readback_buffers =
            self.prepare_sampling_readback_buffers_for_canvas_resize(width, height)?;
        let preview_surfaces = active_preview_request
            .map(|request| {
                let scale_views = preview::preview_requires_scale(request, width, height)
                    .then_some((&surfaces.front.view, &surfaces.back.view));
                self.prepare_preview_surfaces_for_canvas_resize(width, height, request, scale_views)
            })
            .transpose()?;
        Ok(GpuCanvasGeneration {
            surfaces,
            preview_surfaces,
            sampling_readback_buffers,
        })
    }

    pub(super) fn try_create_compositor_surface_set(
        &self,
        width: u32,
        height: u32,
    ) -> Result<GpuCompositorSurfaceSet> {
        let surfaces =
            GpuCompositorSurfaceSet::try_new(&self.device, &self.pipeline, width, height)?;
        #[cfg(test)]
        self.compositor_surface_allocation_count.set(
            self.compositor_surface_allocation_count
                .get()
                .saturating_add(1),
        );
        Ok(surfaces)
    }

    pub(in crate::render_thread::sparkleflinger) fn apply_canvas_resize(
        &mut self,
        preparation: GpuCanvasPreparation,
    ) {
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("canvas resize committed"));
        self.discard_pending_uploads();
        let (surfaces, preview_surfaces, sampling_readback_buffers) = match preparation {
            GpuCanvasPreparation::Gpu {
                mut generation,
                immutable_scene_snapshots,
                opaque_black_texture,
            } => {
                if let Some(current) = self.surfaces.as_mut() {
                    std::mem::swap(
                        &mut current.screen_upload_pool,
                        &mut generation.surfaces.screen_upload_pool,
                    );
                }
                self.canvas_gpu_admitted = true;
                self.immutable_scene_snapshots = immutable_scene_snapshots;
                self.opaque_black_texture = Some(opaque_black_texture);
                (
                    Some(generation.surfaces),
                    generation.preview_surfaces,
                    generation.sampling_readback_buffers,
                )
            }
            GpuCanvasPreparation::CpuFallback => {
                self.canvas_gpu_admitted = false;
                self.immutable_scene_snapshots.clear();
                self.opaque_black_texture = None;
                (None, None, None)
            }
        };
        self.surfaces = surfaces;
        self.preview_surfaces = preview_surfaces;
        self.sampling_latch
            .install_buffers(sampling_readback_buffers);
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
        self.release_native_screen_caches();
    }
}
