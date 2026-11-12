use std::time::Instant;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::spatial::{LayerFrame, LayerPlacement, is_led_sampled_zone};
use hypercolor_types::scene::{Zone, ZoneId};

use super::super::micros_u32;
use super::super::producer_queue::{ProducerFrame, record_producer_frame};
use super::super::sparkleflinger::{CompositionLayer, CompositionPlan, SparkleFlinger};
use super::ZoneRuntime;
use super::frame_helpers::{
    composed_frame_to_producer_frame, copy_producer_frame_to_canvas, opaque_black_frame,
    passthrough_effect_layer, producer_frame_is_gpu, transparent_black_frame,
};
use super::model::{
    PendingDisplayZoneFrame, RenderSceneContext, RenderedZoneSet, ZoneFrameContext,
    ZoneFrameRequirements,
};
use super::projection::append_projection_composition_layers_for_zone;
use super::scope_classes::{ScopeClassIndex, ZoneClassKey, ZoneScopeClasses, multi_class};
use super::zone_state::{enabled_layer_count, zone_is_active, zone_publishes_direct_canvas};
use crate::performance::FullFrameCopyMetrics;

/// A zone with several scope classes holds layers on the GPU, and the GPU
/// has no texture for each class to compose into.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "zone '{zone_name}' has scoped layers on the GPU and no GPU texture per scope class; \
     its scopes cannot be kept apart on the CPU"
)]
pub(crate) struct ScopeClassesNeedGpuTextures {
    pub(crate) zone_name: String,
}

#[derive(Default)]
pub(super) struct RenderedZonePassOutput {
    pub(super) rendered_zones: RenderedZoneSet,
    pub(super) render_us: u32,
    pub(super) producer_full_frame_copy: FullFrameCopyMetrics,
    /// Scene zones that reused their held frame because their composition
    /// key matched (Spec 86 §2.1).
    pub(super) zone_composition_skips: u32,
}

pub(super) struct ProjectedSceneFrames {
    pub(super) layers: Vec<CompositionLayer>,
    /// Where display-output layers begin in `layers`: every LED-output
    /// layer sits below it and every display backdrop above (Spec 86 §5.1).
    pub(super) display_layer_start: usize,
    pub(super) cpu_replay_complete: bool,
    /// The frame of each scope class of every GPU-resident scene zone this
    /// pass, in zone order: the sources its LEDs sample from (Spec 86 §3).
    pub(super) zone_frames: Vec<(ZoneClassKey, ProducerFrame)>,
}

impl Default for ProjectedSceneFrames {
    fn default() -> Self {
        Self {
            layers: Vec::new(),
            display_layer_start: 0,
            cpu_replay_complete: true,
            zone_frames: Vec::new(),
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
                    reuse_unchanged_composition: false,
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
        gpu_resident: bool,
        project_scene_with_sparkleflinger: bool,
        output: &mut RenderedZonePassOutput,
    ) -> Result<ProjectedSceneFrames> {
        let mut projected_scene = ProjectedSceneFrames::default();
        self.composition_skips = 0;
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
            projected_scene.display_layer_start = projected_scene.layers.len();
        }
        let render_result = (|| -> Result<()> {
            for zone in context.zones {
                if !zone_is_active(zone) || zone_publishes_direct_canvas(zone) {
                    continue;
                }

                let render_start = Instant::now();
                match multi_class(&self.scope_classes, zone.id).cloned() {
                    None => self.render_scene_zone(
                        zone,
                        context,
                        sparkleflinger,
                        gpu_resident,
                        project_scene_with_sparkleflinger,
                        &mut projected_scene,
                        output,
                    )?,
                    Some(classes) => self.render_scene_zone_classes(
                        zone,
                        &classes,
                        context,
                        sparkleflinger,
                        gpu_resident,
                        project_scene_with_sparkleflinger,
                        &mut projected_scene,
                        output,
                    )?,
                }
                output.record_render_elapsed(render_start);
            }
            Ok(())
        })();
        if let Err(error) = render_result {
            projected_scene.layers.clear();
            self.projected_scene_layers = projected_scene.layers;
            return Err(error);
        }
        output.zone_composition_skips = std::mem::take(&mut self.composition_skips);
        Ok(projected_scene)
    }

    /// Render a scene zone that composes one stack: every zone with no
    /// scoped layer.
    #[expect(
        clippy::too_many_arguments,
        reason = "a scene zone renders into the pass's projection and output"
    )]
    fn render_scene_zone(
        &mut self,
        zone: &Zone,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        gpu_resident: bool,
        project_scene_with_sparkleflinger: bool,
        projected_scene: &mut ProjectedSceneFrames,
        output: &mut RenderedZonePassOutput,
    ) -> Result<()> {
        let key = ZoneClassKey::base(zone.id);
        let mut frame = self.render_zone_frame(
            zone,
            context.zone_context(),
            sparkleflinger,
            ZoneFrameRequirements {
                requires_cpu_sampling_canvas: !gpu_resident,
                requires_published_surface: false,
                reuse_unchanged_composition: gpu_resident,
            },
        )?;
        // A zone whose layers were all still loading gets one more poll. On
        // a compositor that hands back GPU frames it stays GPU-resident: a
        // CPU canvas would come from the shared readback latch, a
        // whole-image readback that can hold another zone's pixels (Spec 48
        // §4.1).
        if frame.is_none() && gpu_resident {
            let requires_cpu_sampling_canvas = !sparkleflinger.supports_gpu_output_frames();
            frame = self.render_zone_frame(
                zone,
                context.zone_context(),
                sparkleflinger,
                ZoneFrameRequirements {
                    requires_cpu_sampling_canvas,
                    requires_published_surface: false,
                    reuse_unchanged_composition: false,
                },
            )?;
        }
        if gpu_resident {
            return self.place_gpu_resident_zone_frame(
                zone,
                frame,
                sparkleflinger,
                project_scene_with_sparkleflinger,
                projected_scene,
                output,
            );
        }
        let Some(frame) = frame else {
            if let Some(target) = self.target_canvases.get_mut(&key) {
                target.clear();
            }
            return Ok(());
        };
        let frame = if project_scene_with_sparkleflinger {
            sparkleflinger.stabilize_projected_zone_frame(key, frame)?
        } else {
            frame
        };
        if project_scene_with_sparkleflinger
            && let Some(projection) = self.scene_projection_cache.get(&zone.id)
            && append_projection_composition_layers_for_zone(
                &mut projected_scene.layers,
                &mut projected_scene.display_layer_start,
                |_| Some(&frame),
                zone,
                projection,
                self.scene_width,
                self.scene_height,
            )
        {
            let replayed = if let Some(target) = self.target_canvases.get_mut(&key) {
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
            return Ok(());
        }
        let target = self
            .target_canvases
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("CPU zone target was not admitted"))?;
        if !copy_producer_frame_to_canvas(frame, target, &mut output.producer_full_frame_copy)? {
            target.clear();
            return Ok(());
        }
        output
            .rendered_zones
            .push_fresh_scene_zone_frame(zone.id, ProducerFrame::Canvas(target.clone()));
        Ok(())
    }

    /// Render a scene zone whose outputs see different stacks of layers.
    ///
    /// Each layer renders once. Each scope class then composes its own
    /// stack from those frames into its own target, so an output never
    /// samples a layer outside its scope, and the layers every class shares
    /// are the same pixels under each of them.
    #[expect(
        clippy::too_many_arguments,
        reason = "a scene zone renders into the pass's projection and output"
    )]
    fn render_scene_zone_classes(
        &mut self,
        zone: &Zone,
        classes: &Arc<ZoneScopeClasses>,
        context: RenderSceneContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        gpu_resident: bool,
        project_scene_with_sparkleflinger: bool,
        projected_scene: &mut ProjectedSceneFrames,
        output: &mut RenderedZonePassOutput,
    ) -> Result<()> {
        let requirements = ZoneFrameRequirements {
            requires_cpu_sampling_canvas: !gpu_resident,
            requires_published_surface: false,
            reuse_unchanged_composition: gpu_resident,
        };
        let zone_context = context.zone_context();
        let mut rendered =
            self.render_layer_frames(zone, zone_context, sparkleflinger, requirements)?;
        // Layers that were all still loading get one more poll, as a zone
        // with one class does.
        if rendered.is_empty() && gpu_resident {
            rendered =
                self.render_layer_frames(zone, zone_context, sparkleflinger, requirements)?;
        }
        rendered.record_producer_frames();
        // Off the GPU-resident path a class needs a CPU canvas of its own.
        // A layer that lives on the GPU can reach one only through the
        // compositor's readback latch, which is keyed by size alone and so
        // can hand one class another's pixels. Showing a layer on outputs
        // outside its scope is the one thing a scope must never do, so the
        // frame fails instead and reports through the scene's render
        // failure.
        if !gpu_resident && rendered.holds_gpu_frames() {
            return Err(ScopeClassesNeedGpuTextures {
                zone_name: zone.name.clone(),
            }
            .into());
        }

        let mut frames: Vec<(ScopeClassIndex, ProducerFrame)> =
            Vec::with_capacity(classes.class_count());
        for (class_index, class) in classes.iter() {
            let key = ZoneClassKey::new(zone.id, class_index);
            // The base class is first, so its frame of this pass is there
            // for the classes that add to it.
            let base = frames
                .first()
                .filter(|(index, _)| index.is_base() && !class_index.is_base())
                .map(|(_, frame)| frame);
            let (stack, identity) = Self::class_stack(&rendered, classes, class, base);
            let frame = self.compose_zone_stack(
                zone,
                key,
                stack,
                identity,
                zone_context,
                sparkleflinger,
                requirements,
            )?;
            let frame = if gpu_resident {
                self.stabilize_class_frame(key, frame, sparkleflinger)?
            } else {
                self.settle_cpu_class_frame(key, frame, output)?
            };
            if let Some(frame) = frame {
                frames.push((class_index, frame));
            }
        }

        if gpu_resident {
            // A lone zone at the scene's extent is the scene's own picture:
            // its base class shows where no output sits, as the zone's
            // whole canvas did before any layer was scoped. It takes the
            // place of the black the projection starts from.
            if project_scene_with_sparkleflinger
                && self.lone_scene_zone(context.zones).is_some()
                && let Some((_, base)) = frames.first().filter(|(index, _)| index.is_base())
                && let Some(ground) = projected_scene.layers.first_mut()
            {
                *ground = CompositionLayer::replace_opaque(base.clone());
            }
            let output_frame = |output_index: usize| {
                let class = classes.class_of_output(output_index);
                frames
                    .iter()
                    .find_map(|(index, frame)| (*index == class).then_some(frame))
            };
            let projected = project_scene_with_sparkleflinger
                && self
                    .scene_projection_cache
                    .get(&zone.id)
                    .is_some_and(|projection| {
                        append_projection_composition_layers_for_zone(
                            &mut projected_scene.layers,
                            &mut projected_scene.display_layer_start,
                            output_frame,
                            zone,
                            projection,
                            self.scene_width,
                            self.scene_height,
                        )
                    });
            // The CPU scene reconstruction replays only frames that are
            // already CPU-backed; GPU frames never materialize there.
            let mut replayed_all = true;
            for (class_index, frame) in &frames {
                let key = ZoneClassKey::new(zone.id, *class_index);
                let mut replayed = false;
                if !producer_frame_is_gpu(frame)
                    && let Some(target) = self.target_canvases.get_mut(&key)
                {
                    replayed = copy_producer_frame_to_canvas(
                        frame.clone(),
                        target,
                        &mut output.producer_full_frame_copy,
                    )?;
                }
                if !replayed
                    && !projected
                    && let Some(target) = self.target_canvases.get_mut(&key)
                {
                    target.clear();
                }
                replayed_all &= replayed;
            }
            if projected && !replayed_all {
                projected_scene.cpu_replay_complete = false;
            }
        }

        if self.pictured_zones.contains(&zone.id)
            && let Some(picture) =
                self.compose_zone_picture(zone, classes, &frames, sparkleflinger, gpu_resident)
        {
            output.rendered_zones.push_zone_picture(zone.id, picture);
        }
        output
            .rendered_zones
            .note_zone_classes(zone.id, Arc::clone(classes));
        for (class_index, frame) in frames {
            let key = ZoneClassKey::new(zone.id, class_index);
            let published = held_zone_frame(&frame).unwrap_or_else(|| frame.clone());
            if class_index.is_base() {
                output
                    .rendered_zones
                    .push_fresh_scene_zone_frame(zone.id, published);
            } else {
                output.rendered_zones.push_class_frame(key, published);
            }
            if gpu_resident {
                projected_scene.zone_frames.push((key, frame));
            }
        }
        Ok(())
    }

    /// One frame that shows a zone with several classes for a preview: the
    /// base class, then every output's box drawn from that output's own
    /// class in the order the scene canvas paints them, LED outputs first
    /// and screens over them. Where boxes overlap the later one wins, in
    /// the picture only: LEDs and screens read their class's frame, never
    /// this one.
    fn compose_zone_picture(
        &mut self,
        zone: &Zone,
        classes: &ZoneScopeClasses,
        frames: &[(ScopeClassIndex, ProducerFrame)],
        sparkleflinger: &mut SparkleFlinger,
        gpu_resident: bool,
    ) -> Option<ProducerFrame> {
        let zone_size = (zone.layout.canvas_width, zone.layout.canvas_height);
        let frame_of = |class: ScopeClassIndex| {
            frames
                .iter()
                .find_map(|(index, frame)| (*index == class).then_some(frame))
        };
        let (base, base_frame) = if let Some(base) = frame_of(ScopeClassIndex::BASE) {
            (CompositionLayer::replace(base.clone()), base.clone())
        } else {
            let black = opaque_black_frame(
                &mut self.static_layer_surface_cache,
                zone_size.0,
                zone_size.1,
            )
            .ok()?;
            (CompositionLayer::replace_opaque(black.clone()), black)
        };
        let mut layers = vec![base];
        // The boxes painted so far from a class other than the base. The
        // base lies under everything, so a base-class output paints again
        // only where it follows one of these and takes the overlap back.
        let mut scoped_boxes = Vec::<[f32; 4]>::new();
        for led_stratum in [true, false] {
            for (index, output) in zone.layout.zones.iter().enumerate() {
                if is_led_sampled_zone(output) != led_stratum {
                    continue;
                }
                let Some(window) = LayerFrame::of_output(output)
                    .and_then(|frame| LayerPlacement::window(&frame, zone_size, zone_size))
                else {
                    continue;
                };
                let bounds = window.frame_bounds();
                let class = classes.class_of_output(index);
                let frame = if class.is_base() {
                    let follows_a_scoped_box = bounds.is_some_and(|bounds| {
                        scoped_boxes
                            .iter()
                            .any(|painted| boxes_overlap(bounds, *painted))
                    });
                    if !follows_a_scoped_box {
                        continue;
                    }
                    &base_frame
                } else {
                    let Some(frame) = frame_of(class) else {
                        continue;
                    };
                    scoped_boxes.extend(bounds);
                    frame
                };
                layers.push(CompositionLayer::replace(frame.clone()).with_placement(window));
            }
        }
        let plan = CompositionPlan::with_layers(zone_size.0, zone_size.1, layers)
            .with_cpu_replay_cacheable(false)
            .preserving_scene_preview();
        // A picture with no texture of its own to be kept in is composed
        // on the CPU, as its classes are: the compositor's output is one
        // texture, and the next zone's picture would be written over it.
        let plan = if gpu_resident {
            plan
        } else {
            plan.composed_on_cpu()
        };
        let composed = sparkleflinger.compose_for_outputs(plan, !gpu_resident, None);
        let frame = composed_frame_to_producer_frame(composed, sparkleflinger, false)?;
        if !gpu_resident {
            return Some(frame);
        }
        match sparkleflinger.stabilize_projected_zone_frame(ZoneClassKey::picture(zone.id), frame) {
            Ok(frame) => Some(held_zone_frame(&frame).unwrap_or(frame)),
            Err(error) => {
                tracing::debug!(%error, zone_id = %zone.id, "zone picture was not kept");
                None
            }
        }
    }

    /// Stabilize a GPU-resident class's frame into its snapshot.
    ///
    /// A class whose layers produced nothing this pass keeps its last
    /// stabilized frame; one that never produced a frame has none, and its
    /// outputs hold their published colors (Spec 86 §2.2).
    fn stabilize_class_frame(
        &mut self,
        key: ZoneClassKey,
        frame: Option<ProducerFrame>,
        sparkleflinger: &mut SparkleFlinger,
    ) -> Result<Option<ProducerFrame>> {
        let Some(frame) = frame else {
            return Ok(self.last_zone_frames.get(&key).cloned());
        };
        let frame = sparkleflinger.stabilize_projected_zone_frame(key, frame)?;
        match held_zone_frame(&frame) {
            Some(held) => self.last_zone_frames.insert(key, held),
            None => self.last_zone_frames.remove(&key),
        };
        Ok(Some(frame))
    }

    /// Keep a class's frame of a scene composed on the CPU in the class's
    /// own canvas, which its LEDs sample and the scene canvas reads.
    fn settle_cpu_class_frame(
        &mut self,
        key: ZoneClassKey,
        frame: Option<ProducerFrame>,
        output: &mut RenderedZonePassOutput,
    ) -> Result<Option<ProducerFrame>> {
        let Some(frame) = frame else {
            if let Some(target) = self.target_canvases.get_mut(&key) {
                target.clear();
            }
            return Ok(None);
        };
        let target = self
            .target_canvases
            .get_mut(&key)
            .ok_or_else(|| anyhow::anyhow!("CPU zone target was not admitted"))?;
        if !copy_producer_frame_to_canvas(frame, target, &mut output.producer_full_frame_copy)? {
            target.clear();
            return Ok(None);
        }
        Ok(Some(ProducerFrame::Canvas(target.clone())))
    }

    /// Stabilize a GPU-resident zone's frame into its snapshot, project it
    /// into the scene canvas, and record it as the zone's sampling source.
    ///
    /// A zone whose layers produced nothing this pass keeps its last
    /// stabilized frame; a zone that never produced one contributes no
    /// source, and its outputs hold their published colors (Spec 86 §2.2).
    fn place_gpu_resident_zone_frame(
        &mut self,
        zone: &Zone,
        frame: Option<ProducerFrame>,
        sparkleflinger: &mut SparkleFlinger,
        project_scene_with_sparkleflinger: bool,
        projected_scene: &mut ProjectedSceneFrames,
        output: &mut RenderedZonePassOutput,
    ) -> Result<()> {
        let key = ZoneClassKey::base(zone.id);
        let Some(frame) = self.stabilize_class_frame(key, frame, sparkleflinger)? else {
            return Ok(());
        };
        let projected = project_scene_with_sparkleflinger
            && self
                .scene_projection_cache
                .get(&zone.id)
                .is_some_and(|projection| {
                    append_projection_composition_layers_for_zone(
                        &mut projected_scene.layers,
                        &mut projected_scene.display_layer_start,
                        |_| Some(&frame),
                        zone,
                        projection,
                        self.scene_width,
                        self.scene_height,
                    )
                });
        // The CPU scene reconstruction is the fallback when GPU projection
        // fails or a zone cannot be projected. It can only replay frames that
        // are already CPU-backed; GPU frames never materialize there.
        let mut replayed = false;
        if !producer_frame_is_gpu(&frame)
            && let Some(target) = self.target_canvases.get_mut(&key)
        {
            replayed = copy_producer_frame_to_canvas(
                frame.clone(),
                target,
                &mut output.producer_full_frame_copy,
            )?;
        }
        if !replayed
            && !projected
            && let Some(target) = self.target_canvases.get_mut(&key)
        {
            target.clear();
        }
        if projected && !replayed {
            projected_scene.cpu_replay_complete = false;
        }
        output.rendered_zones.push_fresh_scene_zone_frame(
            zone.id,
            held_zone_frame(&frame).unwrap_or_else(|| frame.clone()),
        );
        projected_scene.zone_frames.push((key, frame));
        Ok(())
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
                context.display_zone_target_fps.get(&zone.id).copied(),
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

/// The copy of a zone's frame kept for passes in which its layers produce
/// nothing, when keeping it costs only a handle copy. A GPU copy drops the
/// snapshot lease: it only ever stands in for this zone's own snapshot,
/// which lives until the next reconcile clears the copy, and holding the
/// lease would keep retired projection resources accounted as in use.
fn held_zone_frame(frame: &ProducerFrame) -> Option<ProducerFrame> {
    match frame {
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(texture) => Some(ProducerFrame::GpuTexture(
            super::super::producer_queue::GpuTextureFrame {
                immutable_lease: None,
                ..texture.clone()
            },
        )),
        ProducerFrame::Surface(_) => Some(frame.clone()),
        _ => None,
    }
}

/// Whether two boxes of zone pixels, each left, top, right, bottom, share
/// any area.
fn boxes_overlap(first: [f32; 4], second: [f32; 4]) -> bool {
    first[0] < second[2] && second[0] < first[2] && first[1] < second[3] && second[1] < first[3]
}
