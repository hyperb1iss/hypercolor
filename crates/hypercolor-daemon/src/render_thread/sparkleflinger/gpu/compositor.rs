use anyhow::{Context, Result};

use super::super::{ComposedFrameSet, CompositionLayer, CompositionPlan, PreviewSurfaceRequest};
use super::frame_set::{
    gpu_composed_from_surface, gpu_composed_with_preview_surface, gpu_composed_without_surfaces,
};
use super::preview::{CachedPreviewSurfaceKey, preview_request_matches_plan};
use super::screen_upload::ScreenUploadPoolSaturated;
use super::source::{cached_readback_key, copy_frame_into_output_texture};
use super::{GpuCompositorOutputSurface, GpuSparkleFlinger};
use crate::render_thread::producer_queue::ProducerFrame;

mod bind_groups;
mod bypass;
mod layers;
mod sampling_state;

pub(crate) use bind_groups::PreparedProjectedComposeBindGroups;
#[cfg(feature = "allocation-contract-tests")]
pub(crate) use bind_groups::ProjectedLookupAllocationFixture;
#[cfg(test)]
pub(super) use bind_groups::{ComposeShaderMode, encode_compose_params};
pub(super) use bind_groups::{ComposeSourceBindGroupCache, create_compose_bind_group};
use layers::{compose_layer_into_gpu, return_screen_frame_scratch, upload_screen_layers};
pub(super) use sampling_state::{SamplingReadbackBuffers, SamplingReadbackLatch};

fn screen_upload_content_keys(
    layers: &[CompositionLayer],
) -> impl Iterator<Item = super::ScreenUploadContentKey> + '_ {
    layers.iter().filter_map(|layer| {
        let ProducerFrame::ScreenPublication(publication) = &layer.frame else {
            return None;
        };
        let extent = publication.surface().extent();
        Some(super::ScreenUploadContentKey::new(
            publication.plan_generation(),
            publication.descriptor_identity(),
            publication.branch_sequence(),
            extent.width(),
            extent.height(),
        ))
    })
}

fn has_screen_upload_layers(layers: &[CompositionLayer]) -> bool {
    screen_upload_content_keys(layers).next().is_some()
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
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                Vec::new(),
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            let (pending_output_submission, native_screen_leases) = pending_output_submission
                .map_or_else(
                    || (None, Vec::new()),
                    |stashed| (Some(stashed.encoder), stashed.native_screen_leases),
                );
            #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
            let pending_output_submission =
                pending_output_submission.map(|stashed| stashed.encoder);
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
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                native_screen_leases,
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
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        let mut native_screen_leases = Vec::new();

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
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                &mut native_screen_leases,
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
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                &mut native_screen_leases,
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
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                &mut native_screen_leases,
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            self.stage_frame_in_flight_with_native_screen_leases(
                encoder,
                None,
                native_screen_leases,
            );
            #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
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
            self.finish_pending_uploads(submission_index.clone());
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            self.retire_native_screen_leases(submission_index, native_screen_leases);
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
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases,
            prepared_preview_surface,
        )
    }
}
