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
    GroupFrameContext, GroupFrameRequirements, PendingDisplayZoneFrame, RenderSceneContext,
    RenderedGroupSet,
};
use super::projection::append_projection_composition_layers_for_group;
use crate::performance::FullFrameCopyMetrics;

#[derive(Default)]
pub(super) struct RenderedGroupPassOutput {
    pub(super) rendered_groups: RenderedGroupSet,
    pub(super) render_us: u32,
    pub(super) producer_full_frame_copy: FullFrameCopyMetrics,
}

pub(super) struct ProjectedSceneFrames {
    pub(super) layers: Vec<CompositionLayer>,
    pub(super) cpu_replay_complete: bool,
}

impl Default for ProjectedSceneFrames {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            cpu_replay_complete: true,
        }
    }
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
    ) -> Result<Option<PendingDisplayZoneFrame>> {
        let display_target = group
            .display_target
            .clone()
            .expect("direct display zone should carry a display target");

        let empty_direct_shell = enabled_layer_count(group) == 0;
        let frame = if empty_direct_shell {
            self.effect_pool.remove_zone(group.id);
            self.retained_materialized_group_frames.remove(&group.id);
            transparent_black_frame(
                &mut self.static_layer_surface_cache,
                group.layout.canvas_width,
                group.layout.canvas_height,
            )?
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
        let Some(frame) = self.surface_backed_direct_frame(group.id, frame, full_frame_copy)?
        else {
            return Ok(None);
        };
        record_producer_frame(&frame);
        Ok(Some(PendingDisplayZoneFrame {
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
    ) -> Result<ProjectedSceneFrames> {
        let mut projected_scene = ProjectedSceneFrames::default();
        if project_scene_with_sparkleflinger {
            projected_scene.layers = std::mem::take(&mut self.projected_scene_layers);
            projected_scene.layers.clear();
            #[cfg(feature = "wgpu")]
            if let Some(opaque_black) = sparkleflinger.opaque_black_gpu_frame() {
                if projected_scene.layers.len() == projected_scene.layers.capacity() {
                    self.projected_scene_layers = projected_scene.layers;
                    anyhow::bail!("projected scene layer scratch was not admitted");
                }
                projected_scene
                    .layers
                    .push(CompositionLayer::replace_opaque(opaque_black));
            }
        }
        let render_result = (|| -> Result<()> {
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
                let Some(frame) = frame else {
                    if let Some(target) = self.target_canvases.get_mut(&group.id) {
                        target.clear();
                    }
                    output.record_render_elapsed(render_start);
                    continue;
                };
                let frame = if project_scene_with_sparkleflinger {
                    sparkleflinger.stabilize_projected_group_frame(group.id, frame)?
                } else {
                    frame
                };
                if project_scene_with_sparkleflinger
                    && let Some(projection) = self.scene_projection_cache.get(&group.id)
                    && append_projection_composition_layers_for_group(
                        &mut projected_scene.layers,
                        &frame,
                        group,
                        projection,
                        self.scene_width,
                        self.scene_height,
                    )
                {
                    let replayed = if let Some(target) = self.target_canvases.get_mut(&group.id) {
                        let replayed = copy_producer_frame_to_canvas(
                            frame,
                            target,
                            &mut output.producer_full_frame_copy,
                        )?;
                        if !replayed {
                            target.clear();
                        }
                        replayed
                    } else {
                        false
                    };
                    if !replayed {
                        projected_scene.cpu_replay_complete = false;
                    }
                    output.record_render_elapsed(render_start);
                    continue;
                }
                let target = self
                    .target_canvases
                    .get_mut(&group.id)
                    .ok_or_else(|| anyhow::anyhow!("CPU group target was not admitted"))?;
                if !copy_producer_frame_to_canvas(
                    frame,
                    target,
                    &mut output.producer_full_frame_copy,
                )? {
                    target.clear();
                    output.record_render_elapsed(render_start);
                    continue;
                }
                output
                    .rendered_groups
                    .push_fresh_scene_group_frame(group.id, ProducerFrame::Canvas(target.clone()));
                output.record_render_elapsed(render_start);
            }
            Ok(())
        })();
        if let Err(error) = render_result {
            projected_scene.layers.clear();
            self.projected_scene_layers = projected_scene.layers;
            return Err(error);
        }
        Ok(projected_scene)
    }

    pub(super) fn render_display_zone_frames(
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
                context.display_zone_target_fps,
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
