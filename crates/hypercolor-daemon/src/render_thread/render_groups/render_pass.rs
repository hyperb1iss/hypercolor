use std::time::Instant;

use anyhow::Result;
use hypercolor_types::scene::{Zone, ZoneId};

use super::super::micros_u32;
use super::super::producer_queue::{ProducerFrame, record_producer_frame};
use super::super::sparkleflinger::{CompositionLayer, SparkleFlinger};
use super::ZoneRuntime;
use super::frame_helpers::{
    copy_producer_frame_to_canvas, passthrough_effect_layer, transparent_black_frame,
};
use super::group_state::{enabled_layer_count, group_is_active, group_publishes_direct_canvas};
use super::model::{
    GroupFrameContext, GroupFrameRequirements, PendingGroupCanvasFrame, RenderSceneContext,
    RenderedGroupSet,
};
use super::projection::projection_composition_layers_for_group;
use crate::performance::FullFrameCopyMetrics;

#[derive(Default)]
pub(super) struct RenderedGroupPassOutput {
    pub(super) rendered_groups: RenderedGroupSet,
    pub(super) render_us: u32,
    pub(super) producer_full_frame_copy: FullFrameCopyMetrics,
}

impl RenderedGroupPassOutput {
    pub(super) fn record_render_elapsed(&mut self, render_start: Instant) {
        self.render_us = self
            .render_us
            .saturating_add(micros_u32(render_start.elapsed()));
    }
}

impl ZoneRuntime {
    fn render_direct_group_frame(
        &mut self,
        group: &Zone,
        context: GroupFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> Result<Option<PendingGroupCanvasFrame>> {
        let display_target = group
            .display_target
            .clone()
            .expect("direct display group should carry a display target");

        let empty_direct_shell = enabled_layer_count(group) == 0;
        let frame = if empty_direct_shell {
            self.effect_pool.remove_group(group.id);
            self.retained_materialized_group_frames.remove(&group.id);
            transparent_black_frame(group.layout.canvas_width, group.layout.canvas_height)
        } else if passthrough_effect_layer(group).is_some() {
            let Some(frame) = self.render_passthrough_effect_layer_frame(group, context)? else {
                return Ok(None);
            };
            frame
        } else {
            let Some(frame) = self.render_group_frame(
                group,
                context,
                sparkleflinger,
                GroupFrameRequirements {
                    requires_cpu_sampling_canvas: true,
                    requires_published_surface: true,
                },
            )?
            else {
                return Ok(None);
            };
            frame
        };
        let Some(frame) = self.surface_backed_direct_frame(group.id, frame, full_frame_copy) else {
            return Ok(None);
        };
        record_producer_frame(&frame);
        Ok(Some(PendingGroupCanvasFrame {
            frame,
            display_target,
            empty_direct_shell,
        }))
    }

    pub(super) fn render_scene_contributor_frames(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        project_scene_with_sparkleflinger: bool,
        output: &mut RenderedGroupPassOutput,
    ) -> Result<Vec<CompositionLayer>> {
        let mut projected_scene_layers = Vec::new();
        for group in context.groups {
            if !group_is_active(group) || group_publishes_direct_canvas(group) {
                continue;
            }

            let render_start = Instant::now();
            let mut frame = self.render_group_frame(
                group,
                context.group_context(),
                sparkleflinger,
                GroupFrameRequirements {
                    requires_cpu_sampling_canvas: !project_scene_with_sparkleflinger,
                    requires_published_surface: false,
                },
            )?;
            if frame.is_none() && project_scene_with_sparkleflinger {
                frame = self.render_group_frame(
                    group,
                    context.group_context(),
                    sparkleflinger,
                    GroupFrameRequirements {
                        requires_cpu_sampling_canvas: true,
                        requires_published_surface: false,
                    },
                )?;
            }
            let Some(target) = self.target_canvases.get_mut(&group.id) else {
                output.record_render_elapsed(render_start);
                continue;
            };
            let Some(frame) = frame else {
                target.clear();
                output.record_render_elapsed(render_start);
                continue;
            };
            if project_scene_with_sparkleflinger
                && let Some(projection) = self.scene_projection_cache.get(&group.id)
                && let Some(layers) = projection_composition_layers_for_group(
                    &frame,
                    group,
                    projection,
                    self.scene_width,
                    self.scene_height,
                )
            {
                projected_scene_layers.extend(layers);
                output.record_render_elapsed(render_start);
                continue;
            }
            if !copy_producer_frame_to_canvas(frame, target, &mut output.producer_full_frame_copy) {
                target.clear();
                output.record_render_elapsed(render_start);
                continue;
            }
            output
                .rendered_groups
                .push_fresh_scene_group_frame(group.id, ProducerFrame::Canvas(target.clone()));
            output.record_render_elapsed(render_start);
        }

        Ok(projected_scene_layers)
    }

    pub(super) fn render_display_group_frames(
        &mut self,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        skip_group_id: Option<ZoneId>,
        output: &mut RenderedGroupPassOutput,
    ) -> Result<()> {
        for group in context.groups {
            if skip_group_id == Some(group.id)
                || !group.enabled
                || !group_is_active(group)
                || !group_publishes_direct_canvas(group)
            {
                continue;
            }

            output.rendered_groups.mark_direct_group_active(group.id);
            if let Some(retained) = self.reuse_retained_direct_group_frame(
                group,
                context.elapsed_ms,
                context.display_group_target_fps,
                context.dependency_key,
            ) {
                let render_start = Instant::now();
                self.advance_direct_group_effects(group, context.group_context())?;
                output.record_render_elapsed(render_start);
                output
                    .rendered_groups
                    .push_retained_direct_group_frame(group.id, retained);
                continue;
            }

            let render_start = Instant::now();
            let Some(frame) = self.render_direct_group_frame(
                group,
                context.group_context(),
                sparkleflinger,
                &mut output.producer_full_frame_copy,
            )?
            else {
                output.record_render_elapsed(render_start);
                if let Some(retained) = self.reuse_latest_direct_group_frame(group) {
                    output
                        .rendered_groups
                        .push_retained_direct_group_frame(group.id, retained);
                }
                continue;
            };
            output.record_render_elapsed(render_start);
            self.retain_direct_group_frame(
                group.id,
                context.elapsed_ms,
                context.dependency_key,
                &frame,
            );
            output
                .rendered_groups
                .push_fresh_direct_group_frame(group.id, frame);
        }

        Ok(())
    }
}
