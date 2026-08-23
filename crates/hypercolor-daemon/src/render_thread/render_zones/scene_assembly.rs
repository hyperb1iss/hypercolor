use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::Zone;

use super::super::frame_sampling::LedSamplingStrategy;
use super::super::micros_u32;
#[cfg(not(feature = "wgpu"))]
use super::super::producer_queue::ProducerFrame;
use super::super::producer_queue::record_producer_frame;
use super::super::sparkleflinger::SparkleFlinger;
use super::ZoneRuntime;
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
        #[cfg(feature = "wgpu")]
        sparkleflinger.begin_media_upload_frame();

        if let Some(result) = self.render_single_full_scene_zone(context, sparkleflinger, zones)? {
            self.clear_effect_error();
            self.retain_frame(context.dependency_key, &result, &[]);
            return Ok(result);
        }

        let mut rendered_zones = RenderedZonePassOutput::default();
        let project_scene_with_sparkleflinger = sparkleflinger.supports_gpu_output_frames()
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
                })
            && zones_support_projection_composition(context.zones, &self.scene_projection_cache);
        let projected_scene = self.render_scene_contributor_frames(
            context,
            sparkleflinger,
            project_scene_with_sparkleflinger,
            &mut rendered_zones,
        )?;
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
        let use_gpu_scene_sampling =
            projected_scene_frame.is_some() && !self.combined_led_layout.zones.is_empty();
        let sample_us = if use_gpu_scene_sampling {
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
        let led_sampling_strategy = if use_gpu_scene_sampling {
            LedSamplingStrategy::SparkleFlinger(self.combined_led_spatial_engine.clone())
        } else {
            LedSamplingStrategy::PreSampled(Arc::clone(&self.combined_led_layout))
        };

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
            render_us: rendered_zones.render_us,
            sample_us,
            scene_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(context.dependency_key, &result, zones);
        Ok(result)
    }

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
            render_us: rendered_zones.render_us,
            sample_us,
            scene_compose_us: 0,
            logical_layer_count: enabled_layer_count(scene_zone),
        }))
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
