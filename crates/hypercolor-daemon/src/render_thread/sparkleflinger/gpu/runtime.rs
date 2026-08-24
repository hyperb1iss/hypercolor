#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;

use anyhow::Result;

use crate::render_thread::gpu_device::{GpuBackendPreference, GpuRenderDevice};
use crate::render_thread::producer_queue::ProducerFrame;
use crate::render_thread::producer_queue::SubmissionRetirementQueue;
use crate::render_thread::sparkleflinger::CompositionPlan;
use crate::render_thread::sparkleflinger::gpu_sampling::GpuSpatialSampler;

use super::canvas::{GpuCanvasAdmission, gpu_canvas_admission};
use super::pipeline::GpuCompositorPipeline;
use super::probe::probe_render_device;
use super::source::gpu_source_frame;
use super::{
    GpuCompositorSurfaceSet, GpuCompositorSurfaceSnapshot, GpuSparkleFlinger,
    MAX_CACHED_PREVIEW_SURFACES, SamplingReadbackLatch,
};

impl GpuSparkleFlinger {
    #[cfg(test)]
    pub(crate) fn compositor_surface_cache_entry_count(&self) -> usize {
        self.compositor_surface_cache.len()
    }

    #[cfg(test)]
    pub(crate) fn screen_layer_host_allocation_count(&self) -> usize {
        self.surfaces
            .iter()
            .chain(
                self.compositor_surface_cache
                    .values()
                    .filter_map(Option::as_ref),
            )
            .fold(0_usize, |total, surfaces| {
                total.saturating_add(surfaces.screen_layer_host_allocation_count)
            })
    }

    #[cfg(test)]
    pub(crate) fn active_surface_generation(&self) -> Option<u64> {
        self.surfaces.as_ref().map(|surfaces| surfaces.generation)
    }

    pub(crate) fn surface_snapshot(&self) -> Option<GpuCompositorSurfaceSnapshot> {
        self.surfaces
            .as_ref()
            .map(GpuCompositorSurfaceSet::snapshot)
    }

    pub(crate) fn new() -> Result<Self> {
        Self::new_with_backend_preference(GpuBackendPreference::Default)
    }

    pub(crate) fn new_with_backend_preference(
        backend_preference: GpuBackendPreference,
    ) -> Result<Self> {
        Self::with_render_device(GpuRenderDevice::new_with_backend_preference(
            "SparkleFlinger GPU compositor",
            backend_preference,
        )?)
    }

    pub(crate) fn with_render_device(render_device: GpuRenderDevice) -> Result<Self> {
        let probe = probe_render_device(&render_device)?;
        #[cfg(all(
            any(target_os = "linux", target_os = "macos", target_os = "windows"),
            feature = "servo-gpu-import"
        ))]
        {
            let info = render_device.info();
            #[cfg(target_os = "windows")]
            let servo_adapter_info = Some(hypercolor_core::effect::ServoGpuImportAdapterInfo {
                vendor_id: info.adapter_vendor_id,
                device_id: info.adapter_device_id,
            });
            #[cfg(not(target_os = "windows"))]
            let servo_adapter_info = None;
            if info.servo_gpu_import_backend_compatible()
                && let Err(error) = hypercolor_core::effect::install_servo_gpu_import_device(
                    render_device.device_handle(),
                    servo_adapter_info,
                )
            {
                tracing::debug!(
                    %error,
                    "Servo GPU import device was already installed or unavailable"
                );
            } else if let Some(reason) = info.servo_gpu_import_backend_reason() {
                tracing::debug!(reason, "Servo GPU import device was not installed");
            }
        }
        let device = render_device.device().clone();
        let queue = render_device.queue().clone();
        let max_buffer_size = device.limits().max_buffer_size;
        let max_storage_buffer_binding_size = device.limits().max_storage_buffer_binding_size;

        let pipeline = GpuCompositorPipeline::new(&device);
        let spatial_sampler = GpuSpatialSampler::new(&device);
        let native_screen =
            super::native_screen::install(&device, &queue, probe.max_texture_dimension_2d)?;

        Ok(Self {
            _render_device: render_device,
            device,
            queue,
            probe,
            max_buffer_size,
            max_storage_buffer_binding_size,
            canvas_gpu_admitted: true,
            pipeline,
            spatial_sampler,
            opaque_black_texture: None,
            surfaces: None,
            compositor_surface_cache: HashMap::new(),
            display_finalize_surfaces: HashMap::new(),
            display_finalize_generation: 0,
            preview_surfaces: None,
            media_texture_pools: HashMap::new(),
            media_texture_epoch: 0,
            projected_zone_snapshots: HashMap::new(),
            immutable_scene_snapshots: Vec::new(),
            current_output: None,
            cached_composition_key: None,
            cached_readback_surface: None,
            cached_preview_surfaces: Vec::with_capacity(MAX_CACHED_PREVIEW_SURFACES),
            frame_in_flight: None,
            pending_preview_map: None,
            ready_preview_surface: None,
            sampling_latch: SamplingReadbackLatch::default(),
            output_generation: 0,
            producer_content_generation: 0,
            cached_sample_result: None,
            screen_bridge: native_screen.bridge,
            screen_target: native_screen.target,
            native_screen_lease_retirements: SubmissionRetirementQueue::default(),
            #[cfg(test)]
            superseded_frame_count: 0,
            #[cfg(test)]
            preview_surface_allocation_count: 0,
            #[cfg(test)]
            defer_preview_resolve_once: false,
            #[cfg(test)]
            defer_preview_map_resolve_once: false,
            #[cfg(test)]
            fail_next_sampling_readback_preparation: false,
            #[cfg(test)]
            fail_next_preview_scale_output_preparation: false,
            #[cfg(test)]
            fail_next_screen_upload_pool_saturation: false,
            #[cfg(test)]
            snapshot_texture_allocation_count: Cell::new(0),
            #[cfg(test)]
            compositor_surface_allocation_count: Cell::new(0),
            #[cfg(test)]
            projected_bind_group_creation_count: Cell::new(0),
            #[cfg(test)]
            fail_next_projected_scene_preparation: Cell::new(false),
        })
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn take_sampling_readback_failure_injection(
        &mut self,
    ) -> bool {
        #[cfg(test)]
        {
            std::mem::take(&mut self.fail_next_sampling_readback_preparation)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn take_preview_scale_output_failure_injection(
        &mut self,
    ) -> bool {
        #[cfg(test)]
        {
            std::mem::take(&mut self.fail_next_preview_scale_output_preparation)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn take_screen_upload_pool_saturation_injection(
        &mut self,
    ) -> bool {
        std::mem::take(&mut self.fail_next_screen_upload_pool_saturation)
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) fn fail_next_sampling_readback_preparation(
        &mut self,
    ) {
        self.fail_next_sampling_readback_preparation = true;
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) fn fail_next_preview_scale_output_preparation(
        &mut self,
    ) {
        self.fail_next_preview_scale_output_preparation = true;
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) fn fail_next_screen_upload_pool_saturation(
        &mut self,
    ) {
        self.fail_next_screen_upload_pool_saturation = true;
    }

    pub(crate) fn supports_plan(&self, plan: &CompositionPlan) -> bool {
        self.canvas_gpu_admitted
            && matches!(
                gpu_canvas_admission(self.probe.max_texture_dimension_2d, plan.width, plan.height,),
                GpuCanvasAdmission::Gpu
            )
            && !plan.layers.is_empty()
            && plan.layers.iter().all(|layer| {
                gpu_source_frame(&layer.frame).is_some()
                    || layer.frame_matches_size(plan.width, plan.height)
            })
            && !self.plan_samples_compositor_storage(plan)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn plan_samples_compositor_storage(
        &self,
        plan: &CompositionPlan,
    ) -> bool {
        plan.layers.iter().any(|layer| {
            let ProducerFrame::GpuTexture(frame) = &layer.frame else {
                return false;
            };
            if plan.layers.len() == 1
                && self.layer_reuses_current_output_texture(layer, plan.width, plan.height)
            {
                return false;
            }
            self.surfaces
                .iter()
                .chain(
                    self.compositor_surface_cache
                        .values()
                        .filter_map(Option::as_ref),
                )
                .any(|surfaces| {
                    frame.storage_id == surfaces.front.storage_id
                        || frame.storage_id == surfaces.back.storage_id
                        || frame.storage_id == surfaces.source.storage_id
                })
        })
    }

    pub(in crate::render_thread::sparkleflinger) const fn canvas_gpu_admitted(&self) -> bool {
        self.canvas_gpu_admitted
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) const fn max_texture_dimension_2d(&self) -> u32 {
        self.probe.max_texture_dimension_2d
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) fn backend_name(&self) -> &str {
        &self.probe.backend
    }
}
