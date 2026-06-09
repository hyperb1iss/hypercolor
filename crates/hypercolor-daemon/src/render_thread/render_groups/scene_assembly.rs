use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::Zone;

use super::super::frame_sampling::LedSamplingStrategy;
use super::super::micros_u32;
use super::super::producer_queue::record_producer_frame;
use super::super::sparkleflinger::SparkleFlinger;
use super::ZoneRuntime;
use super::group_state::{
    enabled_layer_count, group_contributes_to_scene_canvas, scene_logical_layer_count,
};
use super::model::{GroupFrameRequirements, RenderSceneContext, ZoneResult};
use super::projection::groups_support_projection_composition;
use super::render_pass::RenderedGroupPassOutput;

impl ZoneRuntime {
    pub(crate) fn render_scene(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<ZoneResult> {
        self.reconcile(
            context.groups,
            context.active_scene_id,
            context.dependency_key,
            context.registry,
        )?;
        #[cfg(feature = "wgpu")]
        sparkleflinger.begin_media_upload_frame();

        if let Some(result) = self.render_single_full_scene_group(context, sparkleflinger, zones)? {
            self.clear_effect_error();
            self.retain_frame(context.dependency_key, &result, &[]);
            return Ok(result);
        }

        let mut rendered_groups = RenderedGroupPassOutput::default();
        let project_scene_with_sparkleflinger = sparkleflinger.supports_gpu_output_frames()
            && groups_support_projection_composition(context.groups, &self.scene_projection_cache);
        let projected_scene_layers = self.render_scene_contributor_frames(
            context,
            sparkleflinger,
            project_scene_with_sparkleflinger,
            &mut rendered_groups,
        )?;
        self.render_display_group_frames(context, sparkleflinger, None, &mut rendered_groups)?;
        zones.clear();
        let logical_layer_count = scene_logical_layer_count(context.groups);
        let use_gpu_scene_sampling =
            project_scene_with_sparkleflinger && !self.combined_led_layout.zones.is_empty();
        let sample_us = if use_gpu_scene_sampling {
            0
        } else {
            let sample_start = Instant::now();
            self.sample_scene_group_led_zones(context.groups, zones);
            micros_u32(sample_start.elapsed())
        };
        let scene_compose_start = Instant::now();
        let scene_frame = if project_scene_with_sparkleflinger {
            self.compose_projected_scene_frame(projected_scene_layers, sparkleflinger)
                .unwrap_or_else(|| self.compose_scene_frame(context.groups))
        } else {
            self.compose_scene_frame(context.groups)
        };
        let scene_compose_us = micros_u32(scene_compose_start.elapsed());
        let led_sampling_strategy = if use_gpu_scene_sampling {
            LedSamplingStrategy::SparkleFlinger(self.combined_led_spatial_engine.clone())
        } else {
            LedSamplingStrategy::PreSampled(Arc::clone(&self.combined_led_layout))
        };

        let rendered_parts = rendered_groups
            .rendered_groups
            .into_parts_for_group_order(context.groups);
        let result = ZoneResult {
            scene_frame,
            group_canvases: rendered_parts.group_canvases,
            zone_canvases: rendered_parts.zone_canvases,
            active_group_canvas_ids: rendered_parts.active_group_canvas_ids,
            led_sampling_strategy,
            producer_full_frame_copy: rendered_groups.producer_full_frame_copy,
            render_us: rendered_groups.render_us,
            sample_us,
            scene_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(context.dependency_key, &result, zones);
        Ok(result)
    }

    fn render_single_full_scene_group(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<Option<ZoneResult>> {
        let Some(scene_group) = self.single_full_scene_group(context.groups) else {
            return Ok(None);
        };
        let Some(spatial_engine) = self.spatial_engines.get(&scene_group.id).cloned() else {
            return Ok(None);
        };

        let mut rendered_groups = RenderedGroupPassOutput::default();
        let render_start = Instant::now();
        let scene_frame = if let Some(frame) =
            self.render_passthrough_effect_layer_frame(scene_group, context.group_context())?
        {
            frame
        } else {
            let can_keep_group_gpu_resident = sparkleflinger.supports_gpu_output_frames()
                && sparkleflinger.can_sample_zone_plan(spatial_engine.sampling_plan().as_ref());
            let Some(frame) = self.render_group_frame(
                scene_group,
                context.group_context(),
                sparkleflinger,
                GroupFrameRequirements {
                    requires_cpu_sampling_canvas: !can_keep_group_gpu_resident,
                    requires_published_surface: !can_keep_group_gpu_resident,
                },
            )?
            else {
                return Ok(None);
            };
            frame
        };
        let Some(scene_frame) = self
            .surface_backed_scene_frame(scene_frame, &mut rendered_groups.producer_full_frame_copy)
        else {
            return Ok(None);
        };
        record_producer_frame(&scene_frame);
        rendered_groups.record_render_elapsed(render_start);

        let sample_us = 0_u32;
        if !scene_group.layout.zones.is_empty() {
            zones.clear();
        }
        rendered_groups
            .rendered_groups
            .push_fresh_scene_group_frame(scene_group.id, scene_frame.clone());
        self.render_display_group_frames(
            context,
            sparkleflinger,
            Some(scene_group.id),
            &mut rendered_groups,
        )?;
        zones.clear();

        let rendered_parts = rendered_groups.rendered_groups.into_parts();
        Ok(Some(ZoneResult {
            scene_frame,
            group_canvases: rendered_parts.group_canvases,
            zone_canvases: rendered_parts.zone_canvases,
            active_group_canvas_ids: rendered_parts.active_group_canvas_ids,
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(spatial_engine),
            producer_full_frame_copy: rendered_groups.producer_full_frame_copy,
            render_us: rendered_groups.render_us,
            sample_us,
            scene_compose_us: 0,
            logical_layer_count: enabled_layer_count(scene_group),
        }))
    }

    fn single_full_scene_group<'a>(&self, groups: &'a [Zone]) -> Option<&'a Zone> {
        let mut scene_groups = groups
            .iter()
            .filter(|group| group_contributes_to_scene_canvas(group));
        let group = scene_groups.next()?;
        if scene_groups.next().is_some() {
            return None;
        }
        if group.layout.canvas_width != self.scene_width
            || group.layout.canvas_height != self.scene_height
        {
            return None;
        }
        Some(group)
    }
}
