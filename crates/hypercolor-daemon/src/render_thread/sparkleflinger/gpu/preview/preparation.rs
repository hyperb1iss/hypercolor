use anyhow::{Context, Result};
use hypercolor_core::types::canvas::PublishedSurface;

use super::super::GpuSparkleFlinger;
use super::{
    CachedPreviewSurface, CachedPreviewSurfaceKey, GpuPreviewScaleOutput, GpuPreviewSurfaceSet,
    MAX_CACHED_PREVIEW_SURFACES, PreparedPreviewSurfaceChange, PreparedPreviewSurfaceChangeKind,
    preview_requires_scale,
};
use crate::render_thread::sparkleflinger::PreviewSurfaceRequest;

impl GpuSparkleFlinger {
    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_preview_surfaces_for_canvas_resize(
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

    pub(in crate::render_thread::sparkleflinger::gpu) fn cached_preview_surface(
        &self,
        key: &CachedPreviewSurfaceKey,
    ) -> Option<PublishedSurface> {
        self.cached_preview_surfaces
            .iter()
            .find(|cached| &cached.key == key)
            .map(|cached| cached.surface.clone())
    }

    pub(super) fn store_cached_preview_surface(
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

    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_preview_surface_readback(
        &mut self,
        source_width: u32,
        source_height: u32,
        request: PreviewSurfaceRequest,
        scale_views: Option<(wgpu::TextureView, wgpu::TextureView)>,
        force_replace: bool,
    ) -> Result<PreparedPreviewSurfaceChange> {
        super::super::ensure_readback_buffer_capacity(
            self.max_buffer_size,
            request.width,
            request.height,
            false,
        )?;
        let requires_scale = preview_requires_scale(request, source_width, source_height);
        if requires_scale {
            super::super::ensure_storage_buffer_capacity(
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

    pub(super) fn commit_preview_surface_change(&mut self, prepared: PreparedPreviewSurfaceChange) {
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
}
