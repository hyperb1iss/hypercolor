use anyhow::Result;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::FrameDataSources;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_core::spatial::DeviceSurfaceMap;
use hypercolor_types::asset::AssetId;
#[cfg(not(feature = "servo-gpu-import"))]
use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{LayerSource, MediaPlayback, SceneLayer, SceneLayerId};
use hypercolor_types::scene::Zone;

use super::ZoneRuntime;
use super::effect_errors::render_layer_effect_error;
#[cfg(feature = "wgpu")]
use super::frame_helpers::gpu_composable_layer_frame;
use super::frame_helpers::{
    color_fill_frame, composed_frame_to_producer_frame, composition_layer_for_scene_layer,
    media_layer_producer_frame, passthrough_effect_layer, producer_frame_is_gpu,
    screen_region_layer_frame, transparent_black_frame,
};
use super::model::{
    CachedMediaProducer, MediaLayerFrame, ZoneCompositionIdentity, ZoneFrameContext,
    ZoneFrameRequirements,
};
use crate::render_thread::binding_eval::evaluate_layer_runtime;
use crate::render_thread::producer_queue::{ProducerFrame, record_producer_frame};
use crate::render_thread::sparkleflinger::{
    CompositionLayer, CompositionPlan, PreviewSurfaceRequest, SparkleFlinger,
};

impl ZoneRuntime {
    /// Where device touch surfaces sit on this zone's canvas.
    pub(super) fn device_surfaces_for(&mut self, zone: &Zone) -> Option<DeviceSurfaceMap> {
        self.device_surfaces
            .surfaces_for(zone.id, self.spatial_engines.get(&zone.id))
    }

    pub(super) fn render_zone_frame(
        &mut self,
        zone: &Zone,
        context: ZoneFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        requirements: ZoneFrameRequirements,
    ) -> Result<Option<ProducerFrame>> {
        let mut composition_layers = Vec::new();
        let mut gpu_source_layers = Vec::new();
        for layer in &zone.layers {
            if !layer.enabled {
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
                LayerSource::Effect { .. } => {
                    self.render_effect_layer_frame(zone, &layer_runtime, context)?
                }
                #[cfg(feature = "servo")]
                LayerSource::WebViewport { .. } => {
                    self.render_effect_layer_frame(zone, &layer_runtime, context)?
                }
                LayerSource::ColorFill { rgba } => Some(color_fill_frame(
                    &mut self.static_layer_surface_cache,
                    zone.layout.canvas_width,
                    zone.layout.canvas_height,
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
                                zone.layout.canvas_width,
                                zone.layout.canvas_height,
                            )?)
                        }
                        MediaLayerFrame::Missing => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                zone.id,
                                layer_runtime.id,
                                LayerHealth::AssetMissing,
                            );
                            Some(transparent_black_frame(
                                &mut self.static_layer_surface_cache,
                                zone.layout.canvas_width,
                                zone.layout.canvas_height,
                            )?)
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
                                zone.layout.canvas_width,
                                zone.layout.canvas_height,
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
                            zone.layout.canvas_width,
                            zone.layout.canvas_height,
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
                        zone.layout.canvas_width,
                        zone.layout.canvas_height,
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
            #[cfg(feature = "wgpu")]
            let frame = if requirements.requires_gpu_output() {
                gpu_composable_layer_frame(frame, layer_runtime.id, zone, sparkleflinger)
            } else {
                frame
            };
            if producer_frame_is_gpu(&frame) {
                gpu_source_layers.push(layer_runtime.id);
            }
            record_producer_frame(&frame);
            composition_layers.push(composition_layer_for_scene_layer(&layer_runtime, frame));
        }

        if composition_layers.is_empty() {
            return Ok(None);
        }
        if requirements.reuse_unchanged_composition
            && let Some(held) = self.unchanged_zone_frame(zone, &composition_layers)
        {
            self.composition_skips = self.composition_skips.saturating_add(1);
            return Ok(Some(held));
        }
        let composition_identity = if requirements.reuse_unchanged_composition {
            ZoneCompositionIdentity::of(zone, &composition_layers)
        } else {
            None
        };

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
                    self.zone_composition_identities.insert(zone.id, identity);
                }
                _ => {
                    self.zone_composition_identities.remove(&zone.id);
                }
            }
        }
        Ok(frame)
    }

    /// The zone's held frame when its layers match the ones that produced it,
    /// so composing again would draw the same pixels.
    fn unchanged_zone_frame(
        &self,
        zone: &Zone,
        layers: &[CompositionLayer],
    ) -> Option<ProducerFrame> {
        self.zone_composition_identities
            .get(&zone.id)
            .is_some_and(|identity| identity.matches(zone, layers))
            .then(|| self.last_zone_frames.get(&zone.id).cloned())
            .flatten()
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

        let frame = self.render_effect_layer_frame(zone, &layer, context)?;
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
        for layer in &zone.layers {
            if !layer.enabled {
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

    fn render_effect_layer_frame(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
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
        let device_surfaces = self.device_surfaces_for(zone);
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
