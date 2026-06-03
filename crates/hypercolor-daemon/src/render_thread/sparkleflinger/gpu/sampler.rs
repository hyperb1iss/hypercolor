use anyhow::Result;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_types::event::ZoneColors;

use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSampleSource, GpuSamplingPlan, GpuSamplingPlanKey, PendingGpuSampleReadback,
};

use super::{GpuCompositorOutputSurface, GpuSparkleFlinger};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedSampleResultKey {
    output_generation: u64,
    sampling_plan: GpuSamplingPlanKey,
}

#[derive(Debug, Clone)]
pub(super) struct CachedSampleResult {
    key: CachedSampleResultKey,
    zones: Vec<ZoneColors>,
}

pub(crate) enum GpuZoneSamplingDispatch {
    Unsupported,
    Ready,
    Saturated,
    Pending(PendingGpuZoneSampling),
}

pub(crate) struct PendingGpuZoneSampling {
    pub(super) output_generation: u64,
    pub(super) sampling_plan: Option<GpuSamplingPlanKey>,
    pub(super) pending_readback: PendingGpuSampleReadback,
}

impl GpuSparkleFlinger {
    pub(crate) fn sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        match self.begin_sample_zone_plan_into(prepared_zones, zones)? {
            GpuZoneSamplingDispatch::Unsupported => Ok(false),
            GpuZoneSamplingDispatch::Ready => Ok(true),
            GpuZoneSamplingDispatch::Saturated => Ok(false),
            GpuZoneSamplingDispatch::Pending(pending) => {
                self.finish_pending_zone_sampling(pending, zones)?;
                Ok(true)
            }
        }
    }

    pub(crate) fn begin_sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<GpuZoneSamplingDispatch> {
        let sampling_plan = GpuSamplingPlan::key(prepared_zones);
        if let Some(sampling_plan) = sampling_plan
            && let Some(cached) = self.cached_sample_result.as_ref()
            && cached.key
                == (CachedSampleResultKey {
                    output_generation: self.output_generation,
                    sampling_plan,
                })
        {
            zones.clone_from(&cached.zones);
            return Ok(GpuZoneSamplingDispatch::Ready);
        }
        let Some(output) = self.current_output else {
            return Ok(GpuZoneSamplingDispatch::Unsupported);
        };
        let (source, source_view, output_width, output_height) = {
            let Some(surfaces) = self.surfaces.as_ref() else {
                return Ok(GpuZoneSamplingDispatch::Unsupported);
            };
            let (source, source_view) = match output {
                GpuCompositorOutputSurface::Front => {
                    (GpuSampleSource::Front, surfaces.front.view.clone())
                }
                GpuCompositorOutputSurface::Back => {
                    (GpuSampleSource::Back, surfaces.back.view.clone())
                }
            };
            (source, source_view, surfaces.width, surfaces.height)
        };
        let pending_output_submission = self.pending_output_submission.take();
        let pending_preview_readback = self.pending_preview_readback.take();
        let sampling_dispatch = self.spatial_sampler.sample_texture_into(
            &self.device,
            &self.queue,
            source,
            &source_view,
            output_width,
            output_height,
            prepared_zones,
            zones,
            pending_output_submission,
        )?;
        if let Some(pending_preview_readback) = pending_preview_readback {
            if sampling_dispatch.submission_index.is_some() {
                if self.pending_preview_map.is_some() {
                    self.discard_pending_preview_map();
                }
                self.begin_pending_preview_map(pending_preview_readback)?;
                self.pending_preview_submission = None;
            } else {
                self.pending_preview_readback = Some(pending_preview_readback);
            }
        }
        if sampling_dispatch.queue_saturated {
            return Ok(GpuZoneSamplingDispatch::Saturated);
        }
        if let Some(pending_readback) = sampling_dispatch.pending_readback {
            return Ok(GpuZoneSamplingDispatch::Pending(PendingGpuZoneSampling {
                output_generation: self.output_generation,
                sampling_plan,
                pending_readback,
            }));
        }
        if sampling_dispatch.sampled
            && let Some(sampling_plan) = sampling_plan
        {
            let mut cached_zones = self
                .cached_sample_result
                .take()
                .map_or_else(Vec::new, |cached| cached.zones);
            cached_zones.clone_from(zones);
            self.cached_sample_result = Some(CachedSampleResult {
                key: CachedSampleResultKey {
                    output_generation: self.output_generation,
                    sampling_plan,
                },
                zones: cached_zones,
            });
        }
        if sampling_dispatch.sampled {
            Ok(GpuZoneSamplingDispatch::Ready)
        } else {
            Ok(GpuZoneSamplingDispatch::Unsupported)
        }
    }

    pub(crate) fn finish_pending_zone_sampling(
        &mut self,
        mut pending: PendingGpuZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<()> {
        if !self.try_finish_pending_zone_sampling(&mut pending, zones)? {
            self.spatial_sampler.finish_pending_readback(
                &self.device,
                pending.pending_readback,
                zones,
            )?;
            self.cache_finished_zone_sampling(
                pending.output_generation,
                pending.sampling_plan,
                zones.as_slice(),
            );
        }
        Ok(())
    }

    pub(crate) fn try_finish_pending_zone_sampling(
        &mut self,
        pending: &mut PendingGpuZoneSampling,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        if !self.spatial_sampler.try_finish_pending_readback(
            &self.device,
            &mut pending.pending_readback,
            zones,
        )? {
            return Ok(false);
        }
        self.cache_finished_zone_sampling(
            pending.output_generation,
            pending.sampling_plan,
            zones.as_slice(),
        );
        Ok(true)
    }

    pub(crate) fn pending_zone_sampling_matches_current_work(
        &self,
        pending: &PendingGpuZoneSampling,
        prepared_zones: &[PreparedZonePlan],
    ) -> bool {
        pending.output_generation == self.output_generation
            && pending.sampling_plan == GpuSamplingPlan::key(prepared_zones)
    }

    pub(crate) fn take_last_sample_readback_wait_blocked(&mut self) -> bool {
        self.spatial_sampler.take_last_readback_wait_blocked()
    }

    pub(crate) const fn max_pending_zone_sampling(&self) -> usize {
        self.spatial_sampler.max_pending_readbacks()
    }

    pub(crate) fn discard_pending_zone_sampling(&mut self, pending: PendingGpuZoneSampling) {
        self.spatial_sampler
            .discard_pending_readback(pending.pending_readback);
    }

    fn cache_finished_zone_sampling(
        &mut self,
        output_generation: u64,
        sampling_plan: Option<GpuSamplingPlanKey>,
        zones: &[ZoneColors],
    ) {
        if output_generation != self.output_generation {
            return;
        }
        let Some(sampling_plan) = sampling_plan else {
            return;
        };
        let mut cached_zones = self
            .cached_sample_result
            .take()
            .map_or_else(Vec::new, |cached| cached.zones);
        cached_zones.clear();
        cached_zones.extend_from_slice(zones);
        self.cached_sample_result = Some(CachedSampleResult {
            key: CachedSampleResultKey {
                output_generation: self.output_generation,
                sampling_plan,
            },
            zones: cached_zones,
        });
    }
}
