use anyhow::Result;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_types::asset::AssetId;
#[cfg(not(feature = "servo-gpu-import"))]
use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::layer::{LayerSource, MediaPlayback, SceneLayer, SceneLayerId};
use hypercolor_types::scene::Zone;

use super::ZoneRuntime;
use super::effect_errors::render_layer_effect_error;
use super::frame_helpers::{
    color_fill_frame, composed_frame_to_producer_frame, composition_layer_for_scene_layer,
    media_layer_producer_frame, passthrough_effect_layer, producer_frame_is_gpu,
    screen_region_layer_frame, transparent_black_frame,
};
use super::model::{
    CachedMediaProducer, GroupFrameContext, GroupFrameRequirements, MediaLayerFrame,
};
use crate::render_thread::binding_eval::evaluate_layer_runtime;
use crate::render_thread::producer_queue::{ProducerFrame, record_producer_frame};
use crate::render_thread::sparkleflinger::{
    CompositionPlan, PreviewSurfaceRequest, SparkleFlinger,
};

impl ZoneRuntime {
    pub(super) fn render_group_frame(
        &mut self,
        group: &Zone,
        context: GroupFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        requirements: GroupFrameRequirements,
    ) -> Result<Option<ProducerFrame>> {
        let mut composition_layers = Vec::new();
        let mut gpu_source_layers = Vec::new();
        for layer in group.effective_layers() {
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
                    self.render_effect_layer_frame(group, &layer_runtime, context)?
                }
                #[cfg(feature = "servo")]
                LayerSource::WebViewport { .. } => {
                    self.render_effect_layer_frame(group, &layer_runtime, context)?
                }
                LayerSource::ColorFill { rgba } => Some(color_fill_frame(
                    group.layout.canvas_width,
                    group.layout.canvas_height,
                    *rgba,
                )),
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
                                group.id,
                                layer_runtime.id,
                                health,
                            );
                            Some(frame)
                        }
                        MediaLayerFrame::Loading => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                group.id,
                                layer_runtime.id,
                                LayerHealth::Loading,
                            );
                            Some(transparent_black_frame(
                                group.layout.canvas_width,
                                group.layout.canvas_height,
                            ))
                        }
                        MediaLayerFrame::Missing => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                group.id,
                                layer_runtime.id,
                                LayerHealth::AssetMissing,
                            );
                            Some(transparent_black_frame(
                                group.layout.canvas_width,
                                group.layout.canvas_height,
                            ))
                        }
                        MediaLayerFrame::Failed(reason) => {
                            self.layer_runtime.note_health(
                                context.active_scene_id,
                                group.id,
                                layer_runtime.id,
                                LayerHealth::Failed { reason },
                            );
                            Some(transparent_black_frame(
                                group.layout.canvas_width,
                                group.layout.canvas_height,
                            ))
                        }
                    }
                }
                LayerSource::ScreenRegion { viewport } => {
                    if let Some(frame) = screen_region_layer_frame(context.inputs.screen, *viewport)
                    {
                        self.layer_runtime.note_health(
                            context.active_scene_id,
                            group.id,
                            layer_runtime.id,
                            LayerHealth::Active,
                        );
                        Some(frame)
                    } else {
                        self.layer_runtime.note_health(
                            context.active_scene_id,
                            group.id,
                            layer_runtime.id,
                            LayerHealth::Loading,
                        );
                        Some(transparent_black_frame(
                            group.layout.canvas_width,
                            group.layout.canvas_height,
                        ))
                    }
                }
                #[cfg(not(feature = "servo"))]
                LayerSource::WebViewport { .. } => {
                    self.layer_runtime.note_health(
                        context.active_scene_id,
                        group.id,
                        layer_runtime.id,
                        LayerHealth::Failed {
                            reason: "web viewport layer source requires the servo feature".into(),
                        },
                    );
                    Some(transparent_black_frame(
                        group.layout.canvas_width,
                        group.layout.canvas_height,
                    ))
                }
            };
            let Some(frame) = frame else {
                self.layer_runtime.note_health(
                    context.active_scene_id,
                    group.id,
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
                    group.id,
                    layer_runtime.id,
                    LayerHealth::Active,
                );
            }
            if producer_frame_is_gpu(&frame) {
                gpu_source_layers.push(layer_runtime.id);
            }
            record_producer_frame(&frame);
            composition_layers.push(composition_layer_for_scene_layer(&layer_runtime, frame));
        }

        if composition_layers.is_empty() {
            return Ok(None);
        }

        let plan = if composition_layers.len() == 1 {
            let layer = composition_layers
                .into_iter()
                .next()
                .expect("single composition layer should exist");
            CompositionPlan::single(group.layout.canvas_width, group.layout.canvas_height, layer)
        } else {
            CompositionPlan::with_layers(
                group.layout.canvas_width,
                group.layout.canvas_height,
                composition_layers,
            )
        };
        let composed = sparkleflinger.compose_for_outputs(
            plan.with_cpu_replay_cacheable(false),
            requirements.requires_cpu_sampling_canvas,
            requirements
                .requires_published_surface
                .then_some(PreviewSurfaceRequest {
                    width: group.layout.canvas_width,
                    height: group.layout.canvas_height,
                }),
        );
        if composed.gpu_readback_failed {
            for layer_id in gpu_source_layers {
                self.layer_runtime.note_health(
                    context.active_scene_id,
                    group.id,
                    layer_id,
                    LayerHealth::Failed {
                        reason: "gpu_readback_failed".to_owned(),
                    },
                );
            }
        }
        Ok(composed_frame_to_producer_frame(composed, sparkleflinger))
    }

    fn render_media_layer_frame(
        &mut self,
        asset_id: AssetId,
        layer_id: SceneLayerId,
        playback: &MediaPlayback,
        elapsed_ms: u32,
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
        group: &Zone,
        context: GroupFrameContext<'_>,
    ) -> Result<Option<ProducerFrame>> {
        let Some(layer) = passthrough_effect_layer(group) else {
            return Ok(None);
        };

        let frame = self.render_effect_layer_frame(group, &layer, context)?;
        if frame.is_some() {
            self.layer_runtime.note_health(
                context.active_scene_id,
                group.id,
                layer.id,
                LayerHealth::Active,
            );
        } else {
            self.layer_runtime.note_health(
                context.active_scene_id,
                group.id,
                layer.id,
                LayerHealth::Loading,
            );
        }
        Ok(frame)
    }

    pub(super) fn advance_direct_group_effects(
        &mut self,
        group: &Zone,
        context: GroupFrameContext<'_>,
    ) -> Result<()> {
        for layer in group.effective_layers() {
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
                    self.advance_effect_layer_output(group, &layer_runtime, context)?;
                }
                #[cfg(feature = "servo")]
                LayerSource::WebViewport { .. } => {
                    self.advance_effect_layer_output(group, &layer_runtime, context)?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn render_effect_layer_frame(
        &mut self,
        group: &Zone,
        layer: &SceneLayer,
        context: GroupFrameContext<'_>,
    ) -> Result<Option<ProducerFrame>> {
        #[cfg(feature = "servo-gpu-import")]
        {
            match self
                .effect_pool
                .render_layer_output(
                    group,
                    layer,
                    context.inputs.delta_secs,
                    context.inputs.audio,
                    context.inputs.interaction,
                    context.inputs.screen,
                    context.inputs.sensors,
                    context.inputs.sources(),
                )
                .map_err(|error| {
                    anyhow::Error::new(render_layer_effect_error(
                        group,
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
            let mut canvas = Canvas::new(group.layout.canvas_width, group.layout.canvas_height);
            self.effect_pool
                .render_layer_into(
                    group,
                    layer,
                    context.inputs.delta_secs,
                    context.inputs.audio,
                    context.inputs.interaction,
                    context.inputs.screen,
                    context.inputs.sensors,
                    context.inputs.sources(),
                    &mut canvas,
                )
                .map_err(|error| {
                    anyhow::Error::new(render_layer_effect_error(
                        group,
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
        group: &Zone,
        layer: &SceneLayer,
        context: GroupFrameContext<'_>,
    ) -> Result<()> {
        self.effect_pool
            .advance_layer_output(
                group,
                layer,
                context.inputs.delta_secs,
                context.inputs.audio,
                context.inputs.interaction,
                context.inputs.screen,
                context.inputs.sensors,
                context.inputs.sources(),
            )
            .map_err(|error| {
                anyhow::Error::new(render_layer_effect_error(
                    group,
                    layer,
                    context.registry,
                    error,
                ))
            })
    }
}
