use anyhow::Result;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::FrameDataSources;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_core::spatial::{DeviceSurfaceMap, LayerFrame, LayerFraming, LayerPlacement};
use hypercolor_types::asset::AssetId;
#[cfg(not(feature = "servo-gpu-import"))]
use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{
    LayerSource, LayerTransform, MediaPlayback, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::Zone;

use super::ZoneRuntime;
use super::effect_errors::render_layer_effect_error;
#[cfg(feature = "wgpu")]
use super::frame_helpers::upload_off_extent_canvas;
use super::frame_helpers::{
    color_fill_frame, composed_frame_to_producer_frame, composition_layer_for_scene_layer,
    media_layer_producer_frame, opaque_black_frame, passthrough_effect_layer,
    producer_frame_is_gpu, screen_region_layer_frame, transparent_black_frame,
};
use super::model::{
    CachedMediaProducer, MediaLayerFrame, ZoneCompositionIdentity, ZoneContentKey,
    ZoneFrameContext, ZoneFrameRequirements,
};
use super::scope_classes::{ScopeClass, ZoneClassKey, ZoneScopeClasses, multi_class};
use crate::render_thread::binding_eval::evaluate_layer_runtime;
use crate::render_thread::producer_queue::{ProducerFrame, record_producer_frame};
use crate::render_thread::sparkleflinger::{
    CompositionLayer, CompositionPlan, PreviewSurfaceRequest, SparkleFlinger,
};

/// One layer's frame this pass, with the layer as its bindings left it.
pub(super) struct RenderedLayer {
    /// The layer's index in its zone's stack.
    index: usize,
    layer: SceneLayer,
    frame: ProducerFrame,
    /// Where the frame lands when the layer is framed to its scope.
    placement: Option<LayerPlacement>,
}

/// Every layer of a zone that produced a frame this pass, bottom to top.
/// Each layer renders once, whatever number of scope classes show it.
pub(super) struct RenderedLayers {
    layers: Vec<RenderedLayer>,
    unavailable_source: bool,
}

impl RenderedLayers {
    pub(super) fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Whether any layer's frame lives on the GPU.
    pub(super) fn holds_gpu_frames(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| producer_frame_is_gpu(&layer.frame))
    }

    /// Count each layer's frame once, whatever number of stacks compose it.
    pub(super) fn record_producer_frames(&self) {
        for layer in &self.layers {
            record_producer_frame(&layer.frame);
        }
    }
}

/// One stack of layers ready to compose at its zone's extent.
pub(super) struct ZoneStack {
    layers: Vec<CompositionLayer>,
    gpu_source_layers: Vec<SceneLayerId>,
    unavailable_source: bool,
}

impl ZoneRuntime {
    /// Where device touch surfaces sit on this zone's canvas.
    pub(super) fn device_surfaces_for(&mut self, zone: &Zone) -> Option<DeviceSurfaceMap> {
        self.device_surfaces
            .surfaces_for(zone.id, self.spatial_engines.get(&zone.id))
    }

    /// How the layer at `index` of `zone`'s stack is framed.
    fn layer_framing(&self, zone: &Zone, index: usize) -> LayerFraming {
        self.scope_classes
            .get(&zone.id)
            .map_or(LayerFraming::Zone, |classes| classes.framing(index))
    }

    pub(super) fn render_zone_frame(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        requirements: ZoneFrameRequirements,
    ) -> Result<Option<ProducerFrame>> {
        let rendered = self.render_layer_frames(zone, context, sparkleflinger, requirements)?;
        rendered.record_producer_frames();
        let stack = ZoneStack {
            unavailable_source: rendered.unavailable_source,
            ..composition_stack(rendered.layers.iter())
        };
        self.compose_zone_stack(
            zone,
            ZoneClassKey::base(zone.id),
            stack,
            None,
            context,
            sparkleflinger,
            requirements,
        )
    }

    /// Render every enabled layer of `zone` that has an output to show on.
    ///
    /// A layer whose scope names no output of the zone is dormant: it has
    /// no renderer and is skipped here, so it reaches no stack.
    pub(super) fn render_layer_frames(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        requirements: ZoneFrameRequirements,
    ) -> Result<RenderedLayers> {
        let zone_size = (zone.layout.canvas_width, zone.layout.canvas_height);
        let mut layer_frames = Vec::new();
        let mut unavailable_source = false;
        for (index, layer) in zone.layers.iter().enumerate() {
            if !layer.enabled {
                continue;
            }
            let framing = self.layer_framing(zone, index);
            let Some((width, height)) = framing.render_extent(zone_size) else {
                continue;
            };
            if self.note_unavailable_effect(zone, layer, context) {
                unavailable_source = true;
                continue;
            }
            let layer_runtime = evaluate_layer_runtime(
                &layer,
                context.inputs.audio,
                context.inputs.sensors,
                context.elapsed_ms,
            )
            .apply_to_layer(&layer);
            let frame = match &layer_runtime.source {
                LayerSource::Effect { .. } => self.render_effect_layer_frame(
                    zone,
                    &layer_runtime,
                    self.layer_touch(zone, index, &layer_runtime, framing),
                    context,
                )?,
                #[cfg(feature = "servo")]
                LayerSource::WebViewport { .. } => self.render_effect_layer_frame(
                    zone,
                    &layer_runtime,
                    self.layer_touch(zone, index, &layer_runtime, framing),
                    context,
                )?,
                LayerSource::ColorFill { rgba } => Some(color_fill_frame(
                    &mut self.static_layer_surface_cache,
                    width,
                    height,
                    *rgba,
                )?),
                LayerSource::Media { asset_id, playback } => {
                    match self.render_media_layer_frame(
                        *asset_id,
                        layer_runtime.id,
                        playback,
                        context.elapsed_ms,
                        sparkleflinger,
                    ) {
                        MediaLayerFrame::Ready { frame, health } => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                zone.id,
                                layer_runtime.id,
                                health,
                            );
                            Some(frame)
                        }
                        MediaLayerFrame::Loading => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                zone.id,
                                layer_runtime.id,
                                LayerHealth::Loading,
                            );
                            Some(transparent_black_frame(
                                &mut self.static_layer_surface_cache,
                                width,
                                height,
                            )?)
                        }
                        MediaLayerFrame::Missing => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                zone.id,
                                layer_runtime.id,
                                LayerHealth::AssetMissing,
                            );
                            unavailable_source = true;
                            continue;
                        }
                        MediaLayerFrame::Failed(reason) => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                zone.id,
                                layer_runtime.id,
                                LayerHealth::Failed { reason },
                            );
                            Some(transparent_black_frame(
                                &mut self.static_layer_surface_cache,
                                width,
                                height,
                            )?)
                        }
                    }
                }
                LayerSource::ScreenRegion { viewport } => {
                    if let Some(frame) =
                        screen_region_layer_frame(context.inputs.screen, *viewport)?
                    {
                        self.layer_runtime.note_health(
                            context.active_scene_id,
                            zone.id,
                            layer_runtime.id,
                            LayerHealth::Active,
                        );
                        Some(frame)
                    } else {
                        self.layer_runtime.note_health(
                            context.active_scene_id,
                            zone.id,
                            layer_runtime.id,
                            LayerHealth::Loading,
                        );
                        Some(transparent_black_frame(
                            &mut self.static_layer_surface_cache,
                            width,
                            height,
                        )?)
                    }
                }
                #[cfg(not(feature = "servo"))]
                LayerSource::WebViewport { .. } => {
                    self.layer_runtime.note_health(
                        context.active_scene_id,
                        zone.id,
                        layer_runtime.id,
                        LayerHealth::Failed {
                            reason: "web viewport layer source requires the servo feature".into(),
                        },
                    );
                    Some(transparent_black_frame(
                        &mut self.static_layer_surface_cache,
                        width,
                        height,
                    )?)
                }
            };
            let Some(frame) = frame else {
                self.layer_runtime.note_health(
                    context.active_scene_id,
                    zone.id,
                    layer_runtime.id,
                    LayerHealth::Loading,
                );
                continue;
            };
            if matches!(
                &layer_runtime.source,
                LayerSource::Effect { .. } | LayerSource::ColorFill { .. }
            ) {
                self.layer_runtime.note_health(
                    context.active_scene_id,
                    zone.id,
                    layer_runtime.id,
                    LayerHealth::Active,
                );
            }
            layer_frames.push((index, framing, layer_runtime, frame));
        }

        // A plan that can only compose on the GPU, because the zone asks for
        // no CPU output or a layer is a GPU texture the CPU compositor cannot
        // read, loses its whole frame to one canvas off the zone's extent.
        #[cfg(feature = "wgpu")]
        if requirements.requires_gpu_output()
            || layer_frames
                .iter()
                .any(|(_, _, _, frame)| producer_frame_is_gpu(frame))
        {
            for (_, _, layer_runtime, frame) in &mut layer_frames {
                upload_off_extent_canvas(frame, layer_runtime.id, zone, sparkleflinger);
            }
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = (sparkleflinger, requirements);

        let mut layers = Vec::with_capacity(layer_frames.len());
        for (index, framing, layer, frame) in layer_frames {
            // A framed layer draws through the placement of the frame it
            // produced. One that cannot be placed draws nothing.
            let placement = match framing {
                LayerFraming::Framed(layer_frame) => {
                    let Some(placement) = LayerPlacement::new(
                        &layer_frame,
                        layer.transform,
                        (frame.width(), frame.height()),
                        zone_size,
                    ) else {
                        continue;
                    };
                    Some(placement)
                }
                LayerFraming::Zone | LayerFraming::Dormant => None,
            };
            layers.push(RenderedLayer {
                index,
                layer,
                frame,
                placement,
            });
        }
        Ok(RenderedLayers {
            layers,
            unavailable_source,
        })
    }

    /// The layers of one scope class, ready to compose.
    ///
    /// `base` is the base class's frame of this pass. A class that adds its
    /// layers on top of the whole base stack starts from those pixels and
    /// composes only what it adds; the second value is then the class's
    /// whole stack, which says what its frame shows however it was
    /// composed.
    pub(super) fn class_stack(
        rendered: &RenderedLayers,
        classes: &ZoneScopeClasses,
        class: &ScopeClass,
        base: Option<&ProducerFrame>,
    ) -> (ZoneStack, Option<Vec<CompositionLayer>>) {
        let in_class = |layer: &&RenderedLayer| class.layers.contains(&layer.index);
        let mut whole = composition_stack(rendered.layers.iter().filter(in_class));
        whole.unavailable_source = rendered.unavailable_source;
        let Some(base) = base.filter(|_| classes.extends_base(class)) else {
            return (whole, None);
        };
        let above_base = |layer: &&RenderedLayer| {
            in_class(layer) && !classes.base().layers.contains(&layer.index)
        };
        let mut stack = composition_stack(rendered.layers.iter().filter(above_base));
        stack
            .layers
            .insert(0, CompositionLayer::replace(base.clone()));
        stack.unavailable_source = rendered.unavailable_source;
        (stack, Some(whole.layers))
    }

    /// Compose one stack of layers at `zone`'s extent, as the frame of
    /// `key`.
    ///
    /// `identity` is the stack that says what the frame shows, when it is
    /// not the stack being composed: a class that starts from the base
    /// class's pixels is still told apart by every layer it shows.
    #[expect(
        clippy::too_many_arguments,
        reason = "composing a stack needs the zone, its key, and the frame's requirements"
    )]
    pub(super) fn compose_zone_stack(
        &mut self,
        zone: &Zone,
        key: ZoneClassKey,
        stack: ZoneStack,
        identity: Option<Vec<CompositionLayer>>,
        context: ZoneFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        requirements: ZoneFrameRequirements,
    ) -> Result<Option<ProducerFrame>> {
        let ZoneStack {
            layers: mut composition_layers,
            gpu_source_layers,
            unavailable_source,
        } = stack;
        // The bottom layer of a plan replaces the whole canvas with its own
        // blend set aside. A framed layer keeps its blend and draws where
        // its placement says, so it goes over black instead of becoming the
        // base. A scene zone with scoped layers is black as well where no
        // layer applies: it is active, and nothing is drawn there.
        let over_black = match composition_layers.first() {
            Some(bottom) => bottom.is_framed(),
            None => {
                !unavailable_source
                    && zone.display_target.is_none()
                    && self.scope_classes.contains_key(&zone.id)
            }
        };
        if over_black {
            composition_layers.insert(0, self.black_base_layer(zone)?);
        }
        let identity_layers = identity.as_deref().unwrap_or(&composition_layers);
        let composition_identity = ZoneCompositionIdentity::of(zone, identity_layers);
        self.note_zone_content(
            key,
            composition_identity.clone().map(ZoneContentKey::Layers),
        );
        if composition_layers.is_empty() {
            if unavailable_source {
                return transparent_black_frame(
                    &mut self.static_layer_surface_cache,
                    zone.layout.canvas_width,
                    zone.layout.canvas_height,
                )
                .map(Some);
            }
            return Ok(None);
        }
        if requirements.reuse_unchanged_composition
            && let Some(held) = self.unchanged_zone_frame(zone, key, identity_layers)
        {
            self.composition_skips = self.composition_skips.saturating_add(1);
            return Ok(Some(held));
        }

        let plan = if composition_layers.len() == 1 {
            let layer = composition_layers
                .into_iter()
                .next()
                .expect("single composition layer should exist");
            CompositionPlan::single(zone.layout.canvas_width, zone.layout.canvas_height, layer)
        } else {
            CompositionPlan::with_layers(
                zone.layout.canvas_width,
                zone.layout.canvas_height,
                composition_layers,
            )
        };
        // The classes of one zone are all its size. Where a class needs a
        // CPU canvas it is composed on the CPU, so it is never handed
        // another class's pixels by a GPU compositor's readback latch.
        let plan = if requirements.requires_cpu_sampling_canvas
            && multi_class(&self.scope_classes, zone.id).is_some()
        {
            plan.composed_on_cpu()
        } else {
            plan
        };
        let composed = sparkleflinger.compose_for_outputs(
            plan.with_cpu_replay_cacheable(false)
                .preserving_scene_preview(),
            requirements.requires_cpu_sampling_canvas,
            requirements
                .requires_published_surface
                .then_some(PreviewSurfaceRequest {
                    width: zone.layout.canvas_width,
                    height: zone.layout.canvas_height,
                }),
        );
        if composed.gpu_readback_failed {
            for layer_id in gpu_source_layers {
                self.layer_runtime.note_health(
                    context.active_scene_id,
                    zone.id,
                    layer_id,
                    LayerHealth::Failed {
                        reason: "gpu_readback_failed".to_owned(),
                    },
                );
            }
        }
        let frame = composed_frame_to_producer_frame(composed, sparkleflinger, false);
        if requirements.reuse_unchanged_composition {
            match (&frame, composition_identity) {
                (Some(_), Some(identity)) => {
                    self.zone_composition_identities.insert(key, identity);
                }
                _ => {
                    self.zone_composition_identities.remove(&key);
                }
            }
        }
        Ok(frame)
    }

    /// The opaque black a stack with no whole-zone layer at its bottom is
    /// composed over.
    fn black_base_layer(&mut self, zone: &Zone) -> Result<CompositionLayer> {
        Ok(CompositionLayer::replace_opaque(opaque_black_frame(
            &mut self.static_layer_surface_cache,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )?))
    }

    /// The held frame of `key` when its layers match the ones that produced
    /// it, so composing again would draw the same pixels.
    fn unchanged_zone_frame(
        &self,
        zone: &Zone,
        key: ZoneClassKey,
        layers: &[CompositionLayer],
    ) -> Option<ProducerFrame> {
        self.zone_composition_identities
            .get(&key)
            .is_some_and(|identity| identity.matches(zone, layers))
            .then(|| self.last_zone_frames.get(&key).cloned())
            .flatten()
    }

    /// The device surfaces the effect layer at `index` of `zone` is handed:
    /// the zone's, narrowed to the layer when it is scoped or transformed.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn effect_layer_surfaces_for_test(
        &mut self,
        zone: &Zone,
        index: usize,
    ) -> Option<DeviceSurfaceMap> {
        let layer = zone.layers.get(index)?;
        let touch = self.layer_touch(zone, index, layer, self.layer_framing(zone, index));
        self.layer_device_surfaces(zone, touch)
    }

    /// The zone's device surfaces as one layer sees them.
    fn layer_device_surfaces(
        &mut self,
        zone: &Zone,
        touch: Option<LayerTouch>,
    ) -> Option<DeviceSurfaceMap> {
        self.device_surfaces_for(zone).map(|surfaces| match touch {
            Some(touch) => touch.narrow(&surfaces),
            None => surfaces,
        })
    }

    /// How a touch reaches the layer's own source: the placement the layer
    /// is drawn with and the outputs it applies to. `None` for a layer that
    /// fills its zone untransformed, whose source is the zone canvas.
    fn layer_touch(
        &self,
        zone: &Zone,
        index: usize,
        layer: &SceneLayer,
        framing: LayerFraming,
    ) -> Option<LayerTouch> {
        let zone_size = (zone.layout.canvas_width, zone.layout.canvas_height);
        let frame = match framing {
            LayerFraming::Framed(frame) => frame,
            LayerFraming::Zone if layer.transform != LayerTransform::default() => LayerFrame::ZONE,
            LayerFraming::Zone | LayerFraming::Dormant => return None,
        };
        let source_size = framing.render_extent(zone_size)?;
        Some(LayerTouch {
            placement: LayerPlacement::new(&frame, layer.transform, source_size, zone_size)?,
            zone_size,
            scope: self
                .scope_classes
                .get(&zone.id)
                .and_then(|classes| classes.scope(index)),
        })
    }

    fn render_media_layer_frame(
        &mut self,
        asset_id: AssetId,
        layer_id: SceneLayerId,
        playback: &MediaPlayback,
        elapsed_ms: u64,
        sparkleflinger: &mut SparkleFlinger,
    ) -> MediaLayerFrame {
        let Some(asset_library) = &self.asset_library else {
            return MediaLayerFrame::Missing;
        };
        let Ok(library) = asset_library.try_read() else {
            return MediaLayerFrame::Loading;
        };
        let Some(record) = library.get(asset_id).cloned() else {
            return MediaLayerFrame::Missing;
        };
        let object_path = match library.object_path_for_hash(&record.hash_sha256) {
            Ok(path) => path,
            Err(error) => return MediaLayerFrame::Failed(error.to_string()),
        };
        let stream_url_policy = library.stream_url_policy().clone();
        drop(library);

        let needs_reload = self
            .media_producers
            .get(&asset_id)
            .is_none_or(|cached| cached.hash_sha256 != record.hash_sha256);
        if needs_reload {
            match MediaProducer::from_path_with_stream_policy(
                &object_path,
                &record.mime_type,
                &stream_url_policy,
            ) {
                Ok(producer) => {
                    self.media_producers.insert(
                        asset_id,
                        CachedMediaProducer {
                            hash_sha256: record.hash_sha256,
                            producer,
                        },
                    );
                }
                Err(error) => return MediaLayerFrame::Failed(error.to_string()),
            }
        }

        let Some(cached) = self.media_producers.get(&asset_id) else {
            return MediaLayerFrame::Loading;
        };
        if let Some(reason) = cached.producer.live_stream_error() {
            if !cached.producer.has_renderable_frame() {
                return MediaLayerFrame::Failed(reason);
            }
            return MediaLayerFrame::Ready {
                frame: media_layer_producer_frame(
                    layer_id,
                    cached.producer.intrinsic_frame(playback, elapsed_ms),
                    &record.mime_type,
                    sparkleflinger,
                ),
                health: LayerHealth::Failed { reason },
            };
        }
        if !cached.producer.has_renderable_frame() {
            return MediaLayerFrame::Loading;
        }
        MediaLayerFrame::Ready {
            frame: media_layer_producer_frame(
                layer_id,
                cached.producer.intrinsic_frame(playback, elapsed_ms),
                &record.mime_type,
                sparkleflinger,
            ),
            health: LayerHealth::Active,
        }
    }

    pub(super) fn render_passthrough_effect_layer_frame(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
    ) -> Result<Option<ProducerFrame>> {
        let Some(layer) = passthrough_effect_layer(zone) else {
            return Ok(None);
        };

        if self.note_unavailable_effect(zone, &layer, context) {
            self.note_zone_content(ZoneClassKey::base(zone.id), None);
            return transparent_black_frame(
                &mut self.static_layer_surface_cache,
                zone.layout.canvas_width,
                zone.layout.canvas_height,
            )
            .map(Some);
        }

        let frame = self.render_effect_layer_frame(zone, &layer, None, context)?;
        self.note_zone_content(
            ZoneClassKey::base(zone.id),
            frame
                .as_ref()
                .and_then(ProducerFrame::stable_identity)
                .map(ZoneContentKey::Frame),
        );
        if frame.is_some() {
            self.layer_runtime.note_health(
                context.active_scene_id,
                zone.id,
                layer.id,
                LayerHealth::Active,
            );
        } else {
            self.layer_runtime.note_health(
                context.active_scene_id,
                zone.id,
                layer.id,
                LayerHealth::Loading,
            );
        }
        Ok(frame)
    }

    pub(super) fn advance_direct_zone_effects(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
    ) -> Result<()> {
        for (index, layer) in zone.layers.iter().enumerate() {
            if !layer.enabled || self.layer_framing(zone, index).is_dormant() {
                continue;
            }
            if self.note_unavailable_effect(zone, layer, context) {
                continue;
            }
            let layer_runtime = evaluate_layer_runtime(
                &layer,
                context.inputs.audio,
                context.inputs.sensors,
                context.elapsed_ms,
            )
            .apply_to_layer(&layer);
            match &layer_runtime.source {
                LayerSource::Effect { .. } => {
                    self.advance_effect_layer_output(zone, &layer_runtime, context)?;
                }
                #[cfg(feature = "servo")]
                LayerSource::WebViewport { .. } => {
                    self.advance_effect_layer_output(zone, &layer_runtime, context)?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn note_unavailable_effect(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        context: ZoneFrameContext<'_>,
    ) -> bool {
        if !self.effect_pool.layer_unavailable(zone.id, layer.id) {
            return false;
        }
        self.layer_runtime.note_health(
            context.active_scene_id,
            zone.id,
            layer.id,
            LayerHealth::Failed {
                reason: "Effect is not installed".into(),
            },
        );
        true
    }

    /// Stands in for one Servo render of `layer_id`: the next effect render
    /// of that layer returns `frame` instead of asking the effect pool.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn inject_effect_layer_frame_for_test(
        &mut self,
        layer_id: SceneLayerId,
        frame: ProducerFrame,
    ) {
        self.injected_effect_layer_frames.insert(layer_id, frame);
    }

    /// Stands in for a Servo page still loading: the next effect render of
    /// `layer_id` reports `Pending`, and the one after it renders as usual.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn pend_effect_layer_once_for_test(&mut self, layer_id: SceneLayerId) {
        self.pend_effect_layer_renders_for_test(layer_id, 1);
    }

    /// Stands in for a Servo page that stays loading: the next `renders`
    /// effect renders of `layer_id` report `Pending`. A GPU-resident scene
    /// zone whose layers are all pending polls them twice in one frame, so
    /// `2 * n` keeps such a zone without a frame for `n` frames.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn pend_effect_layer_renders_for_test(
        &mut self,
        layer_id: SceneLayerId,
        renders: u32,
    ) {
        if renders == 0 {
            self.pending_effect_layers.remove(&layer_id);
        } else {
            self.pending_effect_layers.insert(layer_id, renders);
        }
    }

    /// Render one effect layer. `touch` narrows the zone's device surfaces
    /// to the layer, so the page is told where a finger lands on its own
    /// source.
    fn render_effect_layer_frame(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        touch: Option<LayerTouch>,
        context: ZoneFrameContext<'_>,
    ) -> Result<Option<ProducerFrame>> {
        #[cfg(all(test, feature = "wgpu"))]
        if let Some(remaining) = self.pending_effect_layers.get_mut(&layer.id) {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                self.pending_effect_layers.remove(&layer.id);
            }
            return Ok(None);
        }
        #[cfg(all(test, feature = "wgpu"))]
        if let Some(frame) = self.injected_effect_layer_frames.remove(&layer.id) {
            return Ok(Some(frame));
        }
        let device_surfaces = self.layer_device_surfaces(zone, touch);
        let sources = FrameDataSources {
            device_surfaces: device_surfaces.as_ref(),
            ..context.inputs.sources()
        };
        #[cfg(feature = "servo-gpu-import")]
        {
            match self
                .effect_pool
                .render_layer_output(
                    zone,
                    layer,
                    context.inputs.delta_secs,
                    context.inputs.audio,
                    context.inputs.interaction,
                    context.inputs.screen,
                    context.inputs.sensors,
                    sources,
                )
                .map_err(|error| {
                    anyhow::Error::new(render_layer_effect_error(
                        zone,
                        layer,
                        context.registry,
                        error,
                    ))
                })? {
                EffectRenderOutput::Cpu(canvas) => Ok(Some(ProducerFrame::Canvas(canvas))),
                EffectRenderOutput::Gpu(frame) => Ok(Some(ProducerFrame::Gpu(frame))),
                EffectRenderOutput::Pending => Ok(None),
            }
        }

        #[cfg(not(feature = "servo-gpu-import"))]
        {
            let mut canvas = Canvas::try_new(zone.layout.canvas_width, zone.layout.canvas_height)?;
            self.effect_pool
                .render_layer_into(
                    zone,
                    layer,
                    context.inputs.delta_secs,
                    context.inputs.audio,
                    context.inputs.interaction,
                    context.inputs.screen,
                    context.inputs.sensors,
                    sources,
                    &mut canvas,
                )
                .map_err(|error| {
                    anyhow::Error::new(render_layer_effect_error(
                        zone,
                        layer,
                        context.registry,
                        error,
                    ))
                })?;
            Ok(Some(ProducerFrame::Canvas(canvas)))
        }
    }

    fn advance_effect_layer_output(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        context: ZoneFrameContext<'_>,
    ) -> Result<()> {
        let device_surfaces = self.device_surfaces_for(zone);
        let sources = FrameDataSources {
            device_surfaces: device_surfaces.as_ref(),
            ..context.inputs.sources()
        };
        self.effect_pool
            .advance_layer_output(
                zone,
                layer,
                context.inputs.delta_secs,
                context.inputs.audio,
                context.inputs.interaction,
                context.inputs.screen,
                context.inputs.sensors,
                sources,
            )
            .map_err(|error| {
                anyhow::Error::new(render_layer_effect_error(
                    zone,
                    layer,
                    context.registry,
                    error,
                ))
            })
    }
}

/// A layer's own view of its zone's device surfaces.
#[derive(Clone)]
pub(super) struct LayerTouch {
    placement: LayerPlacement,
    zone_size: (u32, u32),
    scope: Option<std::sync::Arc<hypercolor_types::layer::LayerScope>>,
}

impl LayerTouch {
    fn narrow(self, surfaces: &DeviceSurfaceMap) -> DeviceSurfaceMap {
        surfaces.for_layer(self.placement, self.zone_size, self.scope)
    }
}

/// The composition layers of `rendered`, bottom to top, each drawn the way
/// its scene layer asks.
fn composition_stack<'a>(rendered: impl Iterator<Item = &'a RenderedLayer>) -> ZoneStack {
    let mut layers = Vec::new();
    let mut gpu_source_layers = Vec::new();
    for rendered in rendered {
        if producer_frame_is_gpu(&rendered.frame) {
            gpu_source_layers.push(rendered.layer.id);
        }
        layers.push(composition_layer_for_scene_layer(
            &rendered.layer,
            rendered.frame.clone(),
            rendered.placement,
        ));
    }
    ZoneStack {
        layers,
        gpu_source_layers,
        unavailable_source: false,
    }
}
