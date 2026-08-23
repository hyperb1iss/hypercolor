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
use super::model::{
    PendingDisplayZoneFrame, RenderSceneContext, RenderedZoneSet, ZoneFrameContext,
    ZoneFrameRequirements,
};
use super::projection::append_projection_composition_layers_for_zone;
use super::zone_state::{enabled_layer_count, zone_is_active, zone_publishes_direct_canvas};
use crate::performance::FullFrameCopyMetrics;

#[derive(Default)]
pub(super) struct RenderedZonePassOutput {
    pub(super) rendered_zones: RenderedZoneSet,
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

impl RenderedZonePassOutput {
    pub(super) fn record_render_elapsed(&mut self, render_start: Instant) {
        self.render_us = self
            .render_us
            .saturating_add(micros_u32(render_start.elapsed()));
    }
}

impl ZoneRuntime {
    fn render_direct_zone_frame(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> Result<Option<PendingDisplayZoneFrame>> {
        let display_target = zone
            .display_target
            .clone()
            .expect("direct display zone should carry a display target");

        let empty_direct_shell = enabled_layer_count(zone) == 0;
        let frame = if empty_direct_shell {
            self.effect_pool.remove_zone(zone.id);
            self.retained_materialized_zone_frames.remove(&zone.id);
            transparent_black_frame(
                &mut self.static_layer_surface_cache,
                zone.layout.canvas_width,
                zone.layout.canvas_height,
            )?
        } else if passthrough_effect_layer(zone).is_some() {
            let Some(frame) = self.render_passthrough_effect_layer_frame(zone, context)? else {
                return Ok(None);
            };
            frame
        } else {
            let Some(frame) = self.render_zone_frame(
                zone,
                context,
                sparkleflinger,
                ZoneFrameRequirements {
                    requires_cpu_sampling_canvas: true,
                    requires_published_surface: true,
                },
            )?
            else {
                return Ok(None);
            };
            frame
        };
        let Some(frame) = self.surface_backed_direct_frame(zone.id, frame, full_frame_copy)? else {
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
        output: &mut RenderedZonePassOutput,
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
            for zone in context.zones {
                if !zone_is_active(zone) || zone_publishes_direct_canvas(zone) {
                    continue;
                }

                let render_start = Instant::now();
                let mut frame = self.render_zone_frame(
                    zone,
                    context.zone_context(),
                    sparkleflinger,
                    ZoneFrameRequirements {
                        requires_cpu_sampling_canvas: !project_scene_with_sparkleflinger,
                        requires_published_surface: false,
                    },
                )?;
                if frame.is_none() && project_scene_with_sparkleflinger {
                    frame = self.render_zone_frame(
                        zone,
                        context.zone_context(),
                        sparkleflinger,
                        ZoneFrameRequirements {
                            requires_cpu_sampling_canvas: true,
                            requires_published_surface: false,
                        },
                    )?;
                }
                let Some(frame) = frame else {
                    if let Some(target) = self.target_canvases.get_mut(&zone.id) {
                        target.clear();
                    }
                    output.record_render_elapsed(render_start);
                    continue;
                };
                let frame = if project_scene_with_sparkleflinger {
                    sparkleflinger.stabilize_projected_zone_frame(zone.id, frame)?
                } else {
                    frame
                };
                if project_scene_with_sparkleflinger
                    && let Some(projection) = self.scene_projection_cache.get(&zone.id)
                    && append_projection_composition_layers_for_zone(
                        &mut projected_scene.layers,
                        &frame,
                        zone,
                        projection,
                        self.scene_width,
                        self.scene_height,
                    )
                {
                    let replayed = if let Some(target) = self.target_canvases.get_mut(&zone.id) {
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
                    .get_mut(&zone.id)
                    .ok_or_else(|| anyhow::anyhow!("CPU zone target was not admitted"))?;
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
                    .rendered_zones
                    .push_fresh_scene_zone_frame(zone.id, ProducerFrame::Canvas(target.clone()));
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
        skip_zone_id: Option<ZoneId>,
        output: &mut RenderedZonePassOutput,
    ) -> Result<()> {
        for zone in context.zones {
            if skip_zone_id == Some(zone.id)
                || !zone.enabled
                || !zone_is_active(zone)
                || !zone_publishes_direct_canvas(zone)
            {
                continue;
            }

            output.rendered_zones.mark_direct_zone_active(zone.id);
            if let Some(retained) = self.reuse_retained_direct_zone_frame(
                zone,
                context.elapsed_ms,
                context.display_zone_target_fps,
                context.dependency_key,
            ) {
                let render_start = Instant::now();
                self.advance_direct_zone_effects(zone, context.zone_context())?;
                output.record_render_elapsed(render_start);
                output
                    .rendered_zones
                    .push_retained_direct_zone_frame(zone.id, retained);
                continue;
            }

            let render_start = Instant::now();
            let Some(frame) = self.render_direct_zone_frame(
                zone,
                context.zone_context(),
                sparkleflinger,
                &mut output.producer_full_frame_copy,
            )?
            else {
                output.record_render_elapsed(render_start);
                if let Some(retained) = self.reuse_latest_direct_zone_frame(zone) {
                    output
                        .rendered_zones
                        .push_retained_direct_zone_frame(zone.id, retained);
                }
                continue;
            };
            output.record_render_elapsed(render_start);
            self.retain_direct_zone_frame(
                zone.id,
                context.elapsed_ms,
                context.dependency_key,
                &frame,
            );
            output
                .rendered_zones
                .push_fresh_direct_zone_frame(zone.id, frame);
        }

        Ok(())
    }
}
