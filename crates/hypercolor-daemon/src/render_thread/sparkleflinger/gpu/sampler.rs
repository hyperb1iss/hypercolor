use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_types::event::ZoneColors;

use crate::render_thread::producer_queue::GpuTextureFrame;
use crate::render_thread::sparkleflinger::gpu_sampling::{
    GpuSamplePass, GpuSampleSource, GpuSamplingPlan, GpuSamplingPlanKey, GpuSamplingPreparation,
    PendingGpuSampleReadback,
};

use super::{FrameInFlight, GpuCompositorOutputSurface, GpuSparkleFlinger};

/// One zone's retained texture and the prepared plan its LEDs sample.
#[derive(Clone)]
pub(crate) struct ZoneTextureSample {
    pub(crate) frame: GpuTextureFrame,
    pub(crate) prepared_zones: Arc<[PreparedZonePlan]>,
}

/// What a sample read: the compositor's output at one generation, possibly
/// with zone textures sampled beside it, or a set of zone textures at their
/// content generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleContentKey {
    Output(u64),
    OutputWithTextures { generation: u64, textures: u64 },
    ZoneTextures(u64),
}

impl SampleContentKey {
    fn for_output(generation: u64, textures: &[ZoneTextureSample]) -> Self {
        if textures.is_empty() {
            Self::Output(generation)
        } else {
            Self::OutputWithTextures {
                generation,
                textures: texture_content_digest(textures),
            }
        }
    }

    fn for_zone_textures(textures: &[ZoneTextureSample]) -> Self {
        Self::ZoneTextures(texture_content_digest(textures))
    }

    /// The compositor output generation this sample read, if it read one.
    const fn output_generation(self) -> Option<u64> {
        match self {
            Self::Output(generation) | Self::OutputWithTextures { generation, .. } => {
                Some(generation)
            }
            Self::ZoneTextures(_) => None,
        }
    }
}

fn texture_content_digest(textures: &[ZoneTextureSample]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for texture in textures {
        texture.frame.storage_id.hash(&mut hasher);
        texture.frame.content_generation.hash(&mut hasher);
    }
    textures.len().hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedSampleResultKey {
    content: SampleContentKey,
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
    pub(super) content: SampleContentKey,
    pub(super) sampling_plan: Option<GpuSamplingPlanKey>,
    pub(super) pending_readback: PendingGpuSampleReadback,
}

impl GpuSparkleFlinger {
    pub(crate) fn can_sample_zone_plan(&mut self, prepared_zones: &[PreparedZonePlan]) -> bool {
        let dimensions = self
            .surfaces
            .as_ref()
            .map(|surfaces| (surfaces.width, surfaces.height))
            .or_else(|| {
                prepared_zones
                    .first()
                    .map(|zone| (zone.prepared_canvas_width, zone.prepared_canvas_height))
            });
        let Some((width, height)) = dimensions else {
            return GpuSamplingPlan::supports_prepared_zones(prepared_zones);
        };
        self.spatial_sampler
            .can_sample_plan(&self.device, width, height, prepared_zones)
    }

    pub(in crate::render_thread::sparkleflinger) fn prepare_zone_sampling_plan(
        &mut self,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
    ) -> GpuSamplingPreparation {
        self.spatial_sampler
            .prepare_plan(&self.device, width, height, prepared_zones)
    }

    pub(in crate::render_thread::sparkleflinger) fn apply_zone_sampling_plan(
        &mut self,
        preparation: GpuSamplingPreparation,
    ) {
        self.spatial_sampler.apply_preparation(preparation);
        self.cached_sample_result = None;
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger) fn fail_next_sampling_preparation(&mut self) {
        self.spatial_sampler.fail_next_plan_preparation();
    }

    pub(crate) fn sample_texture_zone_plan(
        &mut self,
        frame: &GpuTextureFrame,
        prepared_zones: &[PreparedZonePlan],
    ) -> Result<Option<Vec<ZoneColors>>> {
        self.spatial_sampler.clear_bind_groups();
        let result = (|| {
            let mut zones = Vec::new();
            let dispatch = self.spatial_sampler.sample_texture_into(
                &self.device,
                &self.queue,
                GpuSampleSource::Diagnostic,
                &frame.view,
                frame.width,
                frame.height,
                prepared_zones,
                &mut zones,
                None,
            )?;
            if let Some(submission_index) = dispatch.submission_index.clone() {
                self.retire_native_screen_leases(
                    submission_index,
                    frame.native_screen_lease.clone().into_iter().collect(),
                );
            }
            if dispatch.queue_saturated || !dispatch.sampled {
                if let Some(pending) = dispatch.pending_readback {
                    self.spatial_sampler.discard_pending_readback(pending);
                }
                return Ok(None);
            }
            if let Some(pending) = dispatch.pending_readback {
                self.spatial_sampler
                    .finish_pending_readback(&self.device, pending, &mut zones)?;
            }
            Ok(Some(zones))
        })();
        self.spatial_sampler.clear_bind_groups();
        result
    }

    pub(crate) fn sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        match self.begin_sample_output_into(prepared_zones, &[], zones)? {
            GpuZoneSamplingDispatch::Unsupported => Ok(false),
            GpuZoneSamplingDispatch::Ready => Ok(true),
            GpuZoneSamplingDispatch::Saturated => Ok(false),
            GpuZoneSamplingDispatch::Pending(pending) => {
                self.finish_pending_zone_sampling(pending, zones)?;
                Ok(true)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn begin_sample_zone_plan_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<GpuZoneSamplingDispatch> {
        self.begin_sample_output_into(prepared_zones, &[], zones)
    }

    /// Sample the compositor's current output with `prepared_zones`, then
    /// each of `textures` with its own plan, in one dispatch chained after
    /// the output's own work. Colors arrive in that order.
    pub(crate) fn begin_sample_output_into(
        &mut self,
        prepared_zones: &[PreparedZonePlan],
        textures: &[ZoneTextureSample],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<GpuZoneSamplingDispatch> {
        let sampling_plan = output_plan_key(prepared_zones, textures);
        let content = SampleContentKey::for_output(self.output_generation, textures);
        if let Some(sampling_plan) = sampling_plan
            && let Some(cached) = self.cached_sample_result.as_ref()
            && cached.key
                == (CachedSampleResultKey {
                    content,
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
        let mut frame_in_flight = self.frame_in_flight.take();
        if let Some(frame) = frame_in_flight.as_ref() {
            debug_assert_eq!(frame.generation, self.output_generation);
        }
        let pending_output_submission = frame_in_flight
            .as_mut()
            .and_then(FrameInFlight::take_encoder_for_chaining);
        let pending_preview_generation = frame_in_flight
            .as_ref()
            .map_or(self.output_generation, |frame| frame.generation);
        let previous_submission = frame_in_flight
            .as_ref()
            .and_then(FrameInFlight::submission_index);
        let passes = std::iter::once(GpuSamplePass {
            source,
            view: &source_view,
            width: output_width,
            height: output_height,
            prepared_zones,
        })
        .chain(textures.iter().map(zone_texture_pass))
        .collect::<Vec<_>>();
        let sampling_dispatch = match self.spatial_sampler.sample_passes_into(
            &self.device,
            &self.queue,
            &passes,
            zones,
            pending_output_submission,
        ) {
            Ok(dispatch) => dispatch,
            Err(error) => {
                if let Some(frame) = frame_in_flight.take() {
                    drop(frame.supersede("GPU sampling dispatch failed"));
                }
                return Err(error);
            }
        };
        if let Some(submission_index) = sampling_dispatch
            .submission_index
            .clone()
            .or(previous_submission)
        {
            if let Some(frame) = frame_in_flight.as_mut() {
                self.retire_native_screen_leases(
                    submission_index.clone(),
                    frame.take_native_screen_leases(),
                );
            }
            if let Some(pending_preview_readback) = frame_in_flight
                .as_mut()
                .and_then(FrameInFlight::take_preview_readback)
            {
                if self.pending_preview_map.is_some() {
                    self.frame_in_flight = Some(FrameInFlight::submitted(
                        pending_preview_generation,
                        submission_index,
                        pending_preview_readback,
                    ));
                } else {
                    self.begin_pending_preview_map(
                        pending_preview_readback,
                        Some(submission_index),
                    )?;
                }
            }
        } else if let Some(frame) = frame_in_flight.take() {
            drop(frame.supersede("GPU sampling dispatch produced no submission"));
        }
        if sampling_dispatch.queue_saturated {
            return Ok(GpuZoneSamplingDispatch::Saturated);
        }
        if let Some(pending_readback) = sampling_dispatch.pending_readback {
            return Ok(GpuZoneSamplingDispatch::Pending(PendingGpuZoneSampling {
                content,
                sampling_plan,
                pending_readback,
            }));
        }
        if sampling_dispatch.sampled
            && let Some(sampling_plan) = sampling_plan
        {
            self.store_sample_result(content, sampling_plan, zones);
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
                pending.content,
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
        self.cache_finished_zone_sampling(pending.content, pending.sampling_plan, zones.as_slice());
        Ok(true)
    }

    pub(crate) fn pending_zone_sampling_matches_current_work(
        &self,
        pending: &PendingGpuZoneSampling,
        prepared_zones: &[PreparedZonePlan],
        textures: &[ZoneTextureSample],
    ) -> bool {
        pending.content == SampleContentKey::for_output(self.output_generation, textures)
            && pending.sampling_plan == output_plan_key(prepared_zones, textures)
    }

    /// Whether `pending` sampled with the plans of an output request.
    pub(crate) fn pending_zone_sampling_used_output_plan(
        pending: &PendingGpuZoneSampling,
        prepared_zones: &[PreparedZonePlan],
        textures: &[ZoneTextureSample],
    ) -> bool {
        pending.sampling_plan == output_plan_key(prepared_zones, textures)
    }

    /// Whether `pending` sampled with the plans of a zone texture request.
    pub(crate) fn pending_zone_sampling_used_texture_plan(
        pending: &PendingGpuZoneSampling,
        textures: &[ZoneTextureSample],
    ) -> bool {
        pending.sampling_plan == zone_texture_plan_key(textures)
    }

    pub(crate) fn pending_zone_texture_sampling_matches(
        pending: &PendingGpuZoneSampling,
        textures: &[ZoneTextureSample],
    ) -> bool {
        pending.content == SampleContentKey::for_zone_textures(textures)
            && pending.sampling_plan == zone_texture_plan_key(textures)
    }

    /// Sample every zone texture in one dispatch, each with its own plan, and
    /// return their LED colors in texture order.
    ///
    /// Zone textures are snapshots whose copies were submitted before this
    /// call, so the dispatch needs no ordering against compositor work still
    /// in flight and submits on its own encoder.
    pub(crate) fn begin_sample_zone_textures_into(
        &mut self,
        textures: &[ZoneTextureSample],
        zones: &mut Vec<ZoneColors>,
    ) -> Result<GpuZoneSamplingDispatch> {
        let content = SampleContentKey::for_zone_textures(textures);
        let sampling_plan = zone_texture_plan_key(textures);
        if let Some(sampling_plan) = sampling_plan
            && let Some(cached) = self.cached_sample_result.as_ref()
            && cached.key
                == (CachedSampleResultKey {
                    content,
                    sampling_plan,
                })
        {
            zones.clone_from(&cached.zones);
            return Ok(GpuZoneSamplingDispatch::Ready);
        }
        let passes = textures.iter().map(zone_texture_pass).collect::<Vec<_>>();
        let dispatch = self.spatial_sampler.sample_passes_into(
            &self.device,
            &self.queue,
            &passes,
            zones,
            None,
        )?;
        if dispatch.queue_saturated {
            return Ok(GpuZoneSamplingDispatch::Saturated);
        }
        if let Some(pending_readback) = dispatch.pending_readback {
            return Ok(GpuZoneSamplingDispatch::Pending(PendingGpuZoneSampling {
                content,
                sampling_plan,
                pending_readback,
            }));
        }
        if !dispatch.sampled {
            return Ok(GpuZoneSamplingDispatch::Unsupported);
        }
        if let Some(sampling_plan) = sampling_plan {
            self.store_sample_result(content, sampling_plan, zones);
        }
        Ok(GpuZoneSamplingDispatch::Ready)
    }

    pub(crate) fn take_last_sample_readback_wait_blocked(&mut self) -> bool {
        self.spatial_sampler.take_last_readback_wait_blocked()
    }

    /// Sample passes the spatial sampler has encoded since it was created.
    pub(crate) fn sample_pass_count(&self) -> usize {
        self.spatial_sampler.sample_dispatch_count()
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
        content: SampleContentKey,
        sampling_plan: Option<GpuSamplingPlanKey>,
        zones: &[ZoneColors],
    ) {
        // An output sample is only worth caching while the compositor still
        // holds that output; a zone-texture sample is keyed by the snapshot
        // generations it read, so a later lookup can only hit when they match.
        if let Some(generation) = content.output_generation()
            && generation != self.output_generation
        {
            return;
        }
        let Some(sampling_plan) = sampling_plan else {
            return;
        };
        self.store_sample_result(content, sampling_plan, zones);
    }

    /// Forget a cached sample of the compositor output once that output
    /// changes. A zone texture sample stays: its key names the storage and
    /// content generation of every texture it read, which composing new
    /// output never touches.
    pub(super) fn invalidate_output_sample(&mut self) {
        if self
            .cached_sample_result
            .as_ref()
            .is_some_and(|cached| cached.key.content.output_generation().is_some())
        {
            self.cached_sample_result = None;
        }
    }

    fn store_sample_result(
        &mut self,
        content: SampleContentKey,
        sampling_plan: GpuSamplingPlanKey,
        zones: &[ZoneColors],
    ) {
        let mut cached_zones = self
            .cached_sample_result
            .take()
            .map_or_else(Vec::new, |cached| cached.zones);
        cached_zones.clear();
        cached_zones.extend_from_slice(zones);
        self.cached_sample_result = Some(CachedSampleResult {
            key: CachedSampleResultKey {
                content,
                sampling_plan,
            },
            zones: cached_zones,
        });
    }
}

fn zone_texture_pass(texture: &ZoneTextureSample) -> GpuSamplePass<'_> {
    GpuSamplePass {
        source: GpuSampleSource::Texture(texture.frame.storage_id),
        view: &texture.frame.view,
        width: texture.frame.width,
        height: texture.frame.height,
        prepared_zones: &texture.prepared_zones,
    }
}

fn output_plan_key(
    prepared_zones: &[PreparedZonePlan],
    textures: &[ZoneTextureSample],
) -> Option<GpuSamplingPlanKey> {
    GpuSamplingPlan::key_for_plans(
        std::iter::once(prepared_zones).chain(
            textures
                .iter()
                .map(|texture| texture.prepared_zones.as_ref()),
        ),
    )
}

fn zone_texture_plan_key(textures: &[ZoneTextureSample]) -> Option<GpuSamplingPlanKey> {
    GpuSamplingPlan::key_for_plans(
        textures
            .iter()
            .map(|texture| texture.prepared_zones.as_ref()),
    )
}
