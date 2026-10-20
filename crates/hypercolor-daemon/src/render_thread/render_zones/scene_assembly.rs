use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::Zone;
#[cfg(feature = "wgpu")]
use hypercolor_types::scene::ZoneId;

use super::super::frame_sampling::LedSamplingStrategy;
#[cfg(feature = "wgpu")]
use super::super::frame_sampling::{ZoneSampleSource, ZoneSampling, ZoneSamplingSet};
use super::super::micros_u32;
use super::super::producer_queue::{ProducerFrame, record_producer_frame};
use super::super::sparkleflinger::SparkleFlinger;
#[cfg(feature = "wgpu")]
use super::super::sparkleflinger::ZoneTextureSample;
use super::ZoneRuntime;
#[cfg(feature = "wgpu")]
use super::frame_helpers::producer_frame_is_gpu;
use super::model::{
    GpuProjectionReplayUnavailable, RenderSceneContext, ZoneFrameRequirements, ZoneResult,
};
use super::projection::zones_support_projection_composition;
use super::render_pass::RenderedZonePassOutput;
use super::zone_state::{
    enabled_layer_count, scene_logical_layer_count, zone_contributes_to_scene_canvas,
};

impl ZoneRuntime {
    pub(crate) fn render_scene(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<ZoneResult> {
        anyhow::ensure!(
            !self.needs_reconcile(context.dependency_key),
            "render-zone resources were not admitted before scene rendering"
        );
        #[cfg(all(test, feature = "wgpu"))]
        if std::mem::take(&mut self.fail_next_scene_render) {
            anyhow::bail!("injected scene render failure");
        }
        #[cfg(feature = "wgpu")]
        sparkleflinger.begin_media_upload_frame();

        if let Some(result) = self.render_single_full_scene_zone(context, sparkleflinger, zones)? {
            self.clear_effect_error();
            self.retain_frame(context.dependency_key, &result, &[]);
            return Ok(result);
        }

        let mut rendered_zones = RenderedZonePassOutput::default();
        // A multi-zone scene stays on the GPU whenever every scene zone has
        // its retained texture: LEDs sample those textures per zone, and the
        // scene canvas is a projection of them (Spec 86).
        let gpu_resident = sparkleflinger.supports_gpu_output_frames()
            && context
                .zones
                .iter()
                .filter(|zone| zone_contributes_to_scene_canvas(zone))
                .all(|zone| {
                    sparkleflinger.has_projected_zone_resource(
                        zone.id,
                        zone.layout.canvas_width,
                        zone.layout.canvas_height,
                    )
                });
        let project_scene_with_sparkleflinger = gpu_resident
            && zones_support_projection_composition(context.zones, &self.scene_projection_cache);
        let mut projected_scene = self.render_scene_contributor_frames(
            context,
            sparkleflinger,
            gpu_resident,
            project_scene_with_sparkleflinger,
            &mut rendered_zones,
        )?;
        let zone_frames = std::mem::take(&mut projected_scene.zone_frames);
        self.render_display_zone_frames(context, sparkleflinger, None, &mut rendered_zones)?;
        let logical_layer_count = scene_logical_layer_count(context.zones);
        let scene_compose_start = Instant::now();
        #[cfg(feature = "wgpu")]
        let projected_scene_frame = project_scene_with_sparkleflinger
            .then(|| self.compose_projected_scene_frame(projected_scene.layers, sparkleflinger))
            .flatten();
        #[cfg(not(feature = "wgpu"))]
        let projected_scene_frame: Option<ProducerFrame> = None;
        let mut scene_compose_us = micros_u32(scene_compose_start.elapsed());
        if project_scene_with_sparkleflinger
            && projected_scene_frame.is_none()
            && !projected_scene.cpu_replay_complete
        {
            if let Some(retained) = self.reuse_last_good_scene() {
                let _ = sparkleflinger.restore_scene_frame(&retained.scene_frame)?;
                self.clear_effect_error();
                return Ok(retained);
            }
            return Err(GpuProjectionReplayUnavailable.into());
        }
        #[cfg(feature = "wgpu")]
        let zone_sampling = if gpu_resident && !self.combined_led_layout.zones.is_empty() {
            Some(self.zone_sampling_set(context.zones, &zone_frames))
        } else {
            None
        };
        #[cfg(not(feature = "wgpu"))]
        let _ = zone_frames;
        #[cfg(feature = "wgpu")]
        let gpu_zone_sampling = zone_sampling.is_some();
        #[cfg(not(feature = "wgpu"))]
        let gpu_zone_sampling = false;
        let sample_us = if gpu_zone_sampling {
            zones.clear();
            0
        } else {
            let sample_start = Instant::now();
            self.sample_scene_zone_led_zones(context.zones, zones)?;
            micros_u32(sample_start.elapsed())
        };
        let scene_frame = if let Some(frame) = projected_scene_frame {
            frame
        } else {
            let fallback_compose_start = Instant::now();
            let frame = self.compose_scene_frame(context.zones)?;
            scene_compose_us =
                scene_compose_us.saturating_add(micros_u32(fallback_compose_start.elapsed()));
            frame
        };
        #[cfg(feature = "wgpu")]
        let led_sampling_strategy = match zone_sampling {
            Some(set) => LedSamplingStrategy::SparkleFlingerZones(set),
            None => LedSamplingStrategy::PreSampled(Arc::clone(&self.combined_led_layout)),
        };
        #[cfg(not(feature = "wgpu"))]
        let led_sampling_strategy =
            LedSamplingStrategy::PreSampled(Arc::clone(&self.combined_led_layout));

        let rendered_parts = rendered_zones
            .rendered_zones
            .into_parts_for_zone_order(context.zones);
        let result = ZoneResult {
            scene_frame,
            display_zone_frames: rendered_parts.display_zone_frames,
            zone_canvases: rendered_parts.zone_canvases,
            active_display_zone_ids: rendered_parts.active_display_zone_ids,
            led_sampling_strategy,
            producer_full_frame_copy: rendered_zones.producer_full_frame_copy,
            zone_composition_skips: rendered_zones.zone_composition_skips,
            render_us: rendered_zones.render_us,
            sample_us,
            scene_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(context.dependency_key, &result, zones);
        Ok(result)
    }

    /// Render a lone scene zone at the scene extent straight into the scene
    /// frame, skipping projection. A display output's box keeps the zone
    /// that places it without the projected scene's display-last ordering
    /// (Spec 86 §5.1): one contributor places every output, LED or display,
    /// and every box shows that zone's own pixels.
    fn render_single_full_scene_zone(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<Option<ZoneResult>> {
        let Some(scene_zone) = self.single_full_scene_zone(context.zones) else {
            return Ok(None);
        };
        let Some(spatial_engine) = self.spatial_engines.get(&scene_zone.id).cloned() else {
            return Ok(None);
        };

        let mut rendered_zones = RenderedZonePassOutput::default();
        let render_start = Instant::now();
        let scene_frame = if let Some(frame) =
            self.render_passthrough_effect_layer_frame(scene_zone, context.zone_context())?
        {
            frame
        } else {
            let can_keep_zone_gpu_resident = sparkleflinger.supports_gpu_output_frames()
                && sparkleflinger.can_sample_zone_plan(spatial_engine.sampling_plan().as_ref());
            let Some(frame) = self.render_zone_frame(
                scene_zone,
                context.zone_context(),
                sparkleflinger,
                ZoneFrameRequirements {
                    requires_cpu_sampling_canvas: !can_keep_zone_gpu_resident,
                    requires_published_surface: !can_keep_zone_gpu_resident,
                    reuse_unchanged_composition: false,
                },
            )?
            else {
                return Ok(None);
            };
            frame
        };
        let Some(scene_frame) = self.surface_backed_scene_frame(
            scene_frame,
            &mut rendered_zones.producer_full_frame_copy,
        )?
        else {
            return Ok(None);
        };
        let scene_frame = sparkleflinger.stabilize_scene_frame(scene_frame)?;
        record_producer_frame(&scene_frame);
        rendered_zones.record_render_elapsed(render_start);

        let sample_us = 0_u32;
        if !scene_zone.layout.zones.is_empty() {
            zones.clear();
        }
        rendered_zones
            .rendered_zones
            .push_fresh_scene_zone_frame(scene_zone.id, scene_frame.clone());
        self.render_display_zone_frames(
            context,
            sparkleflinger,
            Some(scene_zone.id),
            &mut rendered_zones,
        )?;
        zones.clear();

        let rendered_parts = rendered_zones.rendered_zones.into_parts();
        Ok(Some(ZoneResult {
            scene_frame,
            display_zone_frames: rendered_parts.display_zone_frames,
            zone_canvases: rendered_parts.zone_canvases,
            active_display_zone_ids: rendered_parts.active_display_zone_ids,
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(spatial_engine),
            producer_full_frame_copy: rendered_zones.producer_full_frame_copy,
            zone_composition_skips: rendered_zones.zone_composition_skips,
            render_us: rendered_zones.render_us,
            sample_us,
            scene_compose_us: 0,
            logical_layer_count: enabled_layer_count(scene_zone),
        }))
    }

    /// Describe how each scene zone's LEDs sample this frame, in the zone
    /// order the CPU per-zone path and the combined routing layout use.
    #[cfg(feature = "wgpu")]
    fn zone_sampling_set(
        &self,
        zones: &[Zone],
        zone_frames: &[(ZoneId, ProducerFrame)],
    ) -> Arc<ZoneSamplingSet> {
        let mut sampled = Vec::with_capacity(zone_frames.len());
        let mut gpu_textures = Vec::with_capacity(zone_frames.len());
        for zone in zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone))
        {
            let Some(spatial_engine) = self.spatial_engines.get(&zone.id) else {
                continue;
            };
            let prepared_zones = spatial_engine.sampling_plan();
            let frame = zone_frames
                .iter()
                .find(|(zone_id, _)| *zone_id == zone.id)
                .map(|(_, frame)| frame);
            let source = match frame {
                Some(ProducerFrame::GpuTexture(texture)) => {
                    gpu_textures.push(ZoneTextureSample {
                        frame: texture.clone(),
                        prepared_zones: Arc::clone(&prepared_zones),
                    });
                    ZoneSampleSource::Gpu
                }
                Some(frame) if !producer_frame_is_gpu(frame) => {
                    cpu_zone_sample_source(spatial_engine, frame)
                }
                // A zone that has not produced a frame yet, or an imported GPU
                // frame the sampler cannot bind directly.
                _ => ZoneSampleSource::Held,
            };
            sampled.push(ZoneSampling {
                prepared_zones,
                source,
            });
        }
        Arc::new(ZoneSamplingSet {
            layout: Arc::clone(&self.combined_led_layout),
            zones: sampled,
            gpu_textures,
        })
    }

    fn single_full_scene_zone<'a>(&self, zones: &'a [Zone]) -> Option<&'a Zone> {
        let mut scene_zones = zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone));
        let zone = scene_zones.next()?;
        if scene_zones.next().is_some() {
            return None;
        }
        if zone.layout.canvas_width != self.scene_width
            || zone.layout.canvas_height != self.scene_height
        {
            return None;
        }
        Some(zone)
    }
}

/// Sample a CPU-backed zone on the CPU, where its pixels already live.
#[cfg(feature = "wgpu")]
fn cpu_zone_sample_source(
    spatial_engine: &hypercolor_core::spatial::SpatialEngine,
    frame: &ProducerFrame,
) -> ZoneSampleSource {
    let canvas = match frame {
        ProducerFrame::Canvas(canvas) => std::borrow::Cow::Borrowed(canvas),
        ProducerFrame::Surface(surface) => std::borrow::Cow::Owned(
            hypercolor_types::canvas::Canvas::from_published_surface(surface),
        ),
        other => match other.clone().into_cpu_render_frame() {
            Some((canvas, _)) => std::borrow::Cow::Owned(canvas),
            None => return ZoneSampleSource::Held,
        },
    };
    let mut colors = Vec::new();
    match spatial_engine.try_sample_into(&canvas, &mut colors) {
        Ok(()) => ZoneSampleSource::Cpu(colors),
        Err(error) => {
            tracing::warn!(%error, "CPU-backed zone sampling failed; holding its published colors");
            ZoneSampleSource::Held
        }
    }
}
