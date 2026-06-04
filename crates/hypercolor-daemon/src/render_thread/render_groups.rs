use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::RwLock;

use hypercolor_core::asset::AssetLibrary;
#[cfg(test)]
use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
use hypercolor_core::effect::EffectPool;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::media::MediaProducer;
#[cfg(test)]
use hypercolor_core::input::ScreenData;
use hypercolor_core::spatial::SpatialEngine;
#[cfg(test)]
use hypercolor_core::spatial::sample_led;
use hypercolor_types::asset::AssetId;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::{HypercolorEvent, LayerHealth, ZoneColors};
#[cfg(test)]
use hypercolor_types::layer::{LayerAdjust, LayerBlendMode, LayerTransform};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
#[cfg(test)]
use hypercolor_types::scene::DisplayFaceTarget;
use hypercolor_types::scene::{Zone, ZoneId};
#[cfg(test)]
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;
#[cfg(test)]
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode};

use super::binding_eval::evaluate_layer_runtime;
use super::frame_sampling::LedSamplingStrategy;
use super::layer_runtime::LayerRuntimeRegistry;
use super::micros_u32;
use super::producer_queue::{ProducerFrame, record_producer_frame};
use super::scene_dependency::SceneDependencyKey;
use super::sparkleflinger::{CompositionPlan, PreviewSurfaceRequest, SparkleFlinger};
use crate::performance::FullFrameCopyMetrics;
use effect_errors::render_layer_effect_error;
#[cfg(all(test, feature = "wgpu"))]
use frame_helpers::media_mime_prefers_gpu_texture;
use frame_helpers::{
    color_fill_frame, composed_frame_to_producer_frame, composition_layer_for_scene_layer,
    media_layer_producer_frame, passthrough_effect_layer, producer_frame_is_gpu,
    screen_region_layer_frame, surface_backed_frame, transparent_black_frame,
};
use group_state::{
    combined_led_state, empty_group_layout, enabled_layer_count, group_contributes_to_scene_canvas,
    scene_logical_layer_count,
};
use model::{
    CachedMediaProducer, GroupFrameContext, GroupFrameRequirements, MediaLayerFrame,
    RetainedDirectGroupFrame, RetainedMaterializedGroupFrame, RetainedRenderGroupFrame,
};
pub(crate) use model::{
    GroupCanvasFrame, PendingGroupCanvasFrame, RenderSceneContext, ZoneEffectError,
    ZoneFrameInputs, ZoneResult,
};
use projection::{CachedGroupProjection, groups_support_projection_composition};
#[cfg(test)]
use projection::{
    blit_zone_projection, copy_full_scene_identity_projection,
    projection_composition_layers_for_group, zone_local_position_for_scene_pixel,
};
use render_pass::RenderedGroupPassOutput;

/// Initial slot count for the full-resolution scene surface pool. Sized to absorb
/// typical downstream pins: the canvas watch channel, display-output
/// dispatch, and one in-flight JPEG encode per HTML-face worker. Undersizing
/// forces `begin_dequeue` to reallocate a fresh canvas every frame whenever
/// all slots are still shared downstream, which shows up as producer-stage
/// stalls proportional to `canvas_width * canvas_height * 4` bytes.
const SCENE_SURFACE_POOL_INITIAL_SLOTS: usize = 8;
const SCENE_SURFACE_POOL_MAX_SLOTS: usize = 64;

pub(crate) struct ZoneRuntime {
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    effect_pool: EffectPool,
    media_producers: HashMap<AssetId, CachedMediaProducer>,
    target_canvases: HashMap<ZoneId, Canvas>,
    scene_projection_cache: HashMap<ZoneId, CachedGroupProjection>,
    spatial_engines: HashMap<ZoneId, SpatialEngine>,
    direct_surface_pools: HashMap<ZoneId, RenderSurfacePool>,
    retained_direct_group_frames: HashMap<ZoneId, RetainedDirectGroupFrame>,
    retained_materialized_group_frames: HashMap<ZoneId, RetainedMaterializedGroupFrame>,
    scene_surface_pool: RenderSurfacePool,
    reconciled_dependency_key: Option<SceneDependencyKey>,
    retained_frame: Option<RetainedRenderGroupFrame>,
    last_effect_error: Option<ZoneEffectError>,
    recovered_effect_error: Option<ZoneEffectError>,
    layer_runtime: LayerRuntimeRegistry,
    combined_led_layout: Arc<SpatialLayout>,
    combined_led_spatial_engine: SpatialEngine,
    scene_width: u32,
    scene_height: u32,
}

impl ZoneRuntime {
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Self {
        let (combined_led_layout, combined_led_spatial_engine) =
            combined_led_state(empty_group_layout(scene_width, scene_height));
        Self {
            asset_library: None,
            effect_pool: EffectPool::new(),
            media_producers: HashMap::new(),
            target_canvases: HashMap::new(),
            scene_projection_cache: HashMap::new(),
            spatial_engines: HashMap::new(),
            direct_surface_pools: HashMap::new(),
            retained_direct_group_frames: HashMap::new(),
            retained_materialized_group_frames: HashMap::new(),
            // 8 slots absorbs typical downstream fan-out (watch channel +
            // display-output dispatch + one pin per display worker mid-
            // encode). The higher cap lets preview/display bursts settle
            // into a larger working set instead of reallocating per frame.
            scene_surface_pool: RenderSurfacePool::with_slot_count_and_cap(
                SurfaceDescriptor::rgba8888(scene_width, scene_height),
                SCENE_SURFACE_POOL_INITIAL_SLOTS,
                SCENE_SURFACE_POOL_MAX_SLOTS,
            ),
            reconciled_dependency_key: None,
            retained_frame: None,
            last_effect_error: None,
            recovered_effect_error: None,
            layer_runtime: LayerRuntimeRegistry::default(),
            combined_led_layout,
            combined_led_spatial_engine,
            scene_width,
            scene_height,
        }
    }

    pub(crate) fn with_asset_library(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Self {
        let mut runtime = Self::new(scene_width, scene_height);
        runtime
            .effect_pool
            .set_asset_library(Arc::clone(&asset_library));
        runtime.asset_library = Some(asset_library);
        runtime
    }

    pub(crate) fn asset_library(&self) -> Option<Arc<RwLock<AssetLibrary>>> {
        self.asset_library.clone()
    }

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
            let Some(frame) = self.render_group_frame(
                scene_group,
                context.group_context(),
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

    pub(crate) fn drain_layer_runtime_events(&mut self) -> Vec<HypercolorEvent> {
        self.layer_runtime.drain_events()
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

    fn render_group_frame(
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
        playback: &hypercolor_types::layer::MediaPlayback,
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

    fn render_passthrough_effect_layer_frame(
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

    fn surface_backed_scene_frame(
        &mut self,
        frame: ProducerFrame,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> Option<ProducerFrame> {
        surface_backed_frame(&mut self.scene_surface_pool, frame, full_frame_copy)
    }

    fn surface_backed_direct_frame(
        &mut self,
        group_id: ZoneId,
        frame: ProducerFrame,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> Option<ProducerFrame> {
        let surface_pool = self.direct_surface_pools.get_mut(&group_id)?;
        surface_backed_frame(surface_pool, frame, full_frame_copy)
    }
}

mod display_retention;
mod effect_errors;
mod frame_helpers;
mod group_state;
mod model;
mod projection;
mod reconcile;
mod render_pass;
mod scene_output;
mod scene_retention;
mod surface_pools;
#[cfg(test)]
mod tests;
