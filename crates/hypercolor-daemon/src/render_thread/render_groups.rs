use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::RwLock;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_core::effect::{EffectPool, EffectRegistry};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
#[cfg(test)]
use hypercolor_core::spatial::sample_led;
use hypercolor_types::asset::AssetId;
use hypercolor_types::audio::AudioData;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::{HypercolorEvent, LayerHealth, ZoneColors};
#[cfg(test)]
use hypercolor_types::layer::{LayerAdjust, LayerBlendMode, LayerTransform};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use super::binding_eval::evaluate_layer_runtime;
use super::frame_sampling::{LedSamplingStrategy, RetainedLedSamplingStrategy};
use super::layer_runtime::LayerRuntimeRegistry;
use super::micros_u32;
use super::producer_queue::{ProducerFrame, record_producer_frame};
use super::scene_dependency::SceneDependencyKey;
use super::sparkleflinger::{
    CompositionLayer, CompositionPlan, PreviewSurfaceRequest, SparkleFlinger,
};
use crate::performance::FullFrameCopyMetrics;
#[cfg(all(test, feature = "wgpu"))]
use frame_helpers::media_mime_prefers_gpu_texture;
use frame_helpers::{
    color_fill_frame, composed_frame_to_producer_frame, composition_layer_for_scene_layer,
    copy_producer_frame_to_canvas, media_layer_producer_frame, passthrough_effect_layer,
    producer_frame_is_gpu, screen_region_layer_frame, surface_backed_frame,
    transparent_black_frame,
};
use projection::{
    CachedGroupProjection, build_group_projection, compose_authoritative_scene_canvas,
    groups_support_projection_composition, projection_composition_layers_for_group,
};
#[cfg(test)]
use projection::{
    blit_zone_projection, copy_full_scene_identity_projection, zone_local_position_for_scene_pixel,
};

/// Initial slot count for the full-resolution scene surface pool. Sized to absorb
/// typical downstream pins: the canvas watch channel, display-output
/// dispatch, and one in-flight JPEG encode per HTML-face worker. Undersizing
/// forces `begin_dequeue` to reallocate a fresh canvas every frame whenever
/// all slots are still shared downstream, which shows up as producer-stage
/// stalls proportional to `canvas_width * canvas_height * 4` bytes.
const SCENE_SURFACE_POOL_INITIAL_SLOTS: usize = 8;
const SCENE_SURFACE_POOL_MAX_SLOTS: usize = 64;

/// Initial slot count for per-group direct-canvas pools (HTML-face zones).
/// Same failure mode as the scene surface pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_INITIAL_SLOTS: usize = 6;
const DIRECT_SURFACE_POOL_MAX_SLOTS: usize = 32;

#[derive(Clone)]
pub(crate) struct PendingGroupCanvasFrame {
    pub frame: ProducerFrame,
    pub display_target: DisplayFaceTarget,
    pub(crate) empty_direct_shell: bool,
}

#[cfg(test)]
impl PendingGroupCanvasFrame {
    fn surface_for_test(&self) -> &PublishedSurface {
        match &self.frame {
            ProducerFrame::Surface(surface) => surface,
            ProducerFrame::Canvas(_) => panic!("direct group test expected a published surface"),
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => panic!("direct group test expected a CPU surface"),
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => panic!("direct group test expected a CPU surface"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct GroupCanvasFrame {
    pub frame: DisplayGroupFrame,
    pub display_target: DisplayGroupTarget,
}

pub(crate) struct ZoneResult {
    pub scene_frame: ProducerFrame,
    pub group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    pub zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub active_group_canvas_ids: Vec<ZoneId>,
    pub led_sampling_strategy: LedSamplingStrategy,
    pub producer_full_frame_copy: FullFrameCopyMetrics,
    pub render_us: u32,
    pub sample_us: u32,
    pub scene_compose_us: u32,
    pub logical_layer_count: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct ZoneFrameInputs<'a> {
    pub(crate) delta_secs: f32,
    pub(crate) audio: &'a AudioData,
    pub(crate) interaction: &'a InteractionData,
    pub(crate) screen: Option<&'a ScreenData>,
    pub(crate) sensors: &'a SystemSnapshot,
}

#[derive(Clone, Copy)]
pub(crate) struct RenderSceneContext<'a> {
    pub(crate) groups: &'a [Zone],
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) dependency_key: SceneDependencyKey,
    pub(crate) elapsed_ms: u32,
    pub(crate) display_group_target_fps: &'a HashMap<ZoneId, u32>,
    pub(crate) registry: &'a EffectRegistry,
    pub(crate) inputs: ZoneFrameInputs<'a>,
}

#[derive(Clone, Copy)]
struct GroupFrameContext<'a> {
    active_scene_id: Option<SceneId>,
    elapsed_ms: u32,
    registry: &'a EffectRegistry,
    inputs: ZoneFrameInputs<'a>,
}

impl<'a> RenderSceneContext<'a> {
    fn group_context(&self) -> GroupFrameContext<'a> {
        GroupFrameContext {
            active_scene_id: self.active_scene_id,
            elapsed_ms: self.elapsed_ms,
            registry: self.registry,
            inputs: self.inputs,
        }
    }
}

#[derive(Clone, Copy)]
struct GroupFrameRequirements {
    requires_cpu_sampling_canvas: bool,
    requires_published_surface: bool,
}

#[derive(Default)]
struct RenderedGroupSet {
    group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    active_group_canvas_ids: Vec<ZoneId>,
}

impl RenderedGroupSet {
    fn mark_direct_group_active(&mut self, group_id: ZoneId) {
        self.active_group_canvas_ids.push(group_id);
    }

    fn push_direct_group_frame(&mut self, group_id: ZoneId, frame: PendingGroupCanvasFrame) {
        self.zone_canvases.push((group_id, frame.frame.clone()));
        self.group_canvases.push((group_id, frame));
    }

    fn push_scene_group_frame(&mut self, group_id: ZoneId, frame: ProducerFrame) {
        self.zone_canvases.push((group_id, frame));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("zone '{group_name}' effect '{effect_name}' ({effect_id}) failed: {error}")]
pub(crate) struct ZoneEffectError {
    pub(crate) effect_id: String,
    pub(crate) effect_name: String,
    pub(crate) group_id: ZoneId,
    pub(crate) group_name: String,
    pub(crate) error: String,
}

#[derive(Clone)]
struct RetainedRenderGroupFrame {
    dependency_key: SceneDependencyKey,
    scene_frame: ProducerFrame,
    group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    active_group_canvas_ids: Vec<ZoneId>,
    zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    led_sampling_strategy: RetainedLedSamplingStrategy,
    logical_layer_count: u32,
}

#[derive(Clone)]
struct RetainedDirectGroupFrame {
    frame: PendingGroupCanvasFrame,
    rendered_at_ms: u32,
    dependency_key: SceneDependencyKey,
}

#[derive(Clone)]
struct RetainedMaterializedGroupFrame {
    frame: GroupCanvasFrame,
    rendered_at_ms: u32,
    dependency_key: SceneDependencyKey,
    display_target: DisplayFaceTarget,
    display_route: DisplayGroupOutputRoute,
    empty_direct_shell: bool,
}

struct CachedMediaProducer {
    hash_sha256: String,
    producer: MediaProducer,
}

enum MediaLayerFrame {
    Ready {
        frame: ProducerFrame,
        health: LayerHealth,
    },
    Loading,
    Missing,
    Failed(String),
}

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

    /// Total count of times the backing scene-surface pool had to reuse a
    /// still-shared Published slot (and therefore allocate a fresh canvas).
    /// Monotonically increasing; non-zero growth means the pool is
    /// undersized for current downstream fan-out.
    #[must_use]
    pub(crate) fn scene_surface_pool_saturation_reallocs(&self) -> u64 {
        self.scene_surface_pool.saturation_reallocs()
    }

    /// Same as `scene_surface_pool_saturation_reallocs` but summed across
    /// every direct-canvas group pool (one per HTML-face zone).
    #[must_use]
    pub(crate) fn direct_surface_pool_saturation_reallocs(&self) -> u64 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::saturation_reallocs)
            .sum()
    }

    /// Count of slots the backing scene-surface pool has appended above its
    /// initial capacity since construction. Non-zero values are benign and
    /// reflect the pool settling at its working-set size.
    #[must_use]
    pub(crate) fn scene_surface_pool_grown_slots(&self) -> u32 {
        self.scene_surface_pool.grown_slots()
    }

    /// Total grown slots across every direct-canvas group pool.
    #[must_use]
    pub(crate) fn direct_surface_pool_grown_slots(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::grown_slots)
            .sum()
    }

    #[must_use]
    pub(crate) fn scene_surface_pool_slot_count(&self) -> u32 {
        u32::try_from(self.scene_surface_pool.slot_count()).unwrap_or(u32::MAX)
    }

    #[must_use]
    pub(crate) fn scene_surface_pool_max_slots(&self) -> u32 {
        u32::try_from(self.scene_surface_pool.max_slots()).unwrap_or(u32::MAX)
    }

    #[must_use]
    pub(crate) fn direct_surface_pool_slot_count(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(|pool| u32::try_from(pool.slot_count()).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    #[must_use]
    pub(crate) fn direct_surface_pool_max_slots(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(|pool| u32::try_from(pool.max_slots()).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    pub(crate) fn scene_surface_pool_shared_published_slots(&mut self) -> u32 {
        let counts = self.scene_surface_pool.sharing_counts();
        u32::try_from(counts.shared_published).unwrap_or(u32::MAX)
    }

    pub(crate) fn scene_surface_pool_max_ref_count(&mut self) -> u32 {
        let counts = self.scene_surface_pool.sharing_counts();
        u32::try_from(counts.max_ref_count).unwrap_or(u32::MAX)
    }

    pub(crate) fn direct_surface_pool_shared_published_slots(&mut self) -> u32 {
        self.direct_surface_pools
            .values_mut()
            .map(|pool| u32::try_from(pool.sharing_counts().shared_published).unwrap_or(u32::MAX))
            .fold(0_u32, u32::saturating_add)
    }

    pub(crate) fn direct_surface_pool_max_ref_count(&mut self) -> u32 {
        self.direct_surface_pools
            .values_mut()
            .map(|pool| u32::try_from(pool.sharing_counts().max_ref_count).unwrap_or(u32::MAX))
            .max()
            .unwrap_or_default()
    }

    pub(crate) fn clear_inactive_groups(&mut self) {
        if !self.has_inactive_group_resources() {
            return;
        }

        self.effect_pool.clear();
        self.media_producers.clear();
        self.target_canvases.clear();
        self.scene_projection_cache.clear();
        self.spatial_engines.clear();
        self.direct_surface_pools.clear();
        self.retained_direct_group_frames.clear();
        self.retained_materialized_group_frames.clear();
        self.reconciled_dependency_key = None;
        self.retained_frame = None;
        self.last_effect_error = None;
        self.recovered_effect_error = None;
        self.layer_runtime.clear();
        let (layout, engine) =
            combined_led_state(empty_group_layout(self.scene_width, self.scene_height));
        self.combined_led_layout = layout;
        self.combined_led_spatial_engine = engine;
    }

    pub(crate) fn reuse_scene(&self, dependency_key: SceneDependencyKey) -> Option<ZoneResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.dependency_key != dependency_key {
            return None;
        }

        Some(ZoneResult {
            scene_frame: retained.scene_frame.clone(),
            group_canvases: retained.group_canvases.clone(),
            zone_canvases: retained.zone_canvases.clone(),
            active_group_canvas_ids: retained.active_group_canvas_ids.clone(),
            led_sampling_strategy: LedSamplingStrategy::from_retained(
                &retained.led_sampling_strategy,
            ),
            producer_full_frame_copy: FullFrameCopyMetrics::default(),
            render_us: 0,
            sample_us: 0,
            scene_compose_us: 0,
            logical_layer_count: retained.logical_layer_count,
        })
    }

    fn has_inactive_group_resources(&self) -> bool {
        self.effect_pool.slot_count() > 0
            || !self.target_canvases.is_empty()
            || !self.scene_projection_cache.is_empty()
            || !self.spatial_engines.is_empty()
            || !self.direct_surface_pools.is_empty()
            || !self.retained_direct_group_frames.is_empty()
            || !self.retained_materialized_group_frames.is_empty()
            || self.retained_frame.is_some()
            || self.reconciled_dependency_key.is_some()
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

        let mut render_us = 0_u32;
        let mut producer_full_frame_copy = FullFrameCopyMetrics::default();
        let mut rendered_groups = RenderedGroupSet::default();
        let project_scene_with_sparkleflinger = sparkleflinger.supports_gpu_output_frames()
            && groups_support_projection_composition(context.groups, &self.scene_projection_cache);
        let mut projected_scene_layers = Vec::new();
        for group in context.groups {
            if !group_is_active(group) {
                continue;
            }

            if group_publishes_direct_canvas(group) {
                rendered_groups.mark_direct_group_active(group.id);
                if let Some(retained) = self.reuse_retained_direct_group_frame(
                    group,
                    context.elapsed_ms,
                    context.display_group_target_fps,
                    context.dependency_key,
                ) {
                    rendered_groups.push_direct_group_frame(group.id, retained);
                    continue;
                }
                let render_start = Instant::now();
                let Some(frame) = self.render_direct_group_frame(
                    group,
                    context.group_context(),
                    sparkleflinger,
                    &mut producer_full_frame_copy,
                )?
                else {
                    if let Some(retained) = self.reuse_latest_direct_group_frame(group) {
                        rendered_groups.push_direct_group_frame(group.id, retained);
                    }
                    continue;
                };
                render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
                self.retain_direct_group_frame(
                    group.id,
                    context.elapsed_ms,
                    context.dependency_key,
                    &frame,
                );
                rendered_groups.push_direct_group_frame(group.id, frame);
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
                continue;
            };
            let Some(frame) = frame else {
                target.clear();
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
                render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
                continue;
            }
            if !copy_producer_frame_to_canvas(frame, target, &mut producer_full_frame_copy) {
                target.clear();
                continue;
            }
            rendered_groups.push_scene_group_frame(group.id, ProducerFrame::Canvas(target.clone()));
            render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
        }
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

        let result = ZoneResult {
            scene_frame,
            group_canvases: rendered_groups.group_canvases,
            zone_canvases: rendered_groups.zone_canvases,
            active_group_canvas_ids: rendered_groups.active_group_canvas_ids,
            led_sampling_strategy,
            producer_full_frame_copy,
            render_us,
            sample_us,
            scene_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(context.dependency_key, &result, zones);
        Ok(result)
    }

    fn reconcile(
        &mut self,
        groups: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
    ) -> Result<()> {
        if self.reconciled_dependency_key == Some(dependency_key) {
            return Ok(());
        }

        self.effect_pool.reconcile(groups, registry)?;
        self.layer_runtime.reconcile(active_scene_id, groups);

        let desired_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
        let desired_media_ids = desired_media_asset_ids(groups);
        self.media_producers
            .retain(|asset_id, _| desired_media_ids.contains(asset_id));
        let scene_group_ids = groups
            .iter()
            .filter(|group| group_contributes_to_scene_canvas(group))
            .map(|group| group.id)
            .collect::<HashSet<_>>();
        let direct_group_ids = groups
            .iter()
            .filter(|group| group_publishes_direct_canvas(group))
            .map(|group| group.id)
            .collect::<HashSet<_>>();
        self.target_canvases
            .retain(|group_id, _| scene_group_ids.contains(group_id));
        self.scene_projection_cache
            .retain(|group_id, _| scene_group_ids.contains(group_id));
        self.spatial_engines
            .retain(|group_id, _| desired_ids.contains(group_id));
        self.direct_surface_pools
            .retain(|group_id, _| direct_group_ids.contains(group_id));
        self.retained_direct_group_frames
            .retain(|group_id, _| direct_group_ids.contains(group_id));
        self.retained_materialized_group_frames
            .retain(|group_id, _| direct_group_ids.contains(group_id));

        for group in groups {
            if group_contributes_to_scene_canvas(group) {
                self.ensure_group_canvas(group);
                self.ensure_scene_projection(group);
            }
            if group_publishes_direct_canvas(group) {
                self.ensure_direct_surface_pool(group);
            }
            self.ensure_spatial_engine(group);
        }

        self.reconcile_combined_led_state(groups);
        self.reconciled_dependency_key = Some(dependency_key);

        Ok(())
    }

    fn ensure_group_canvas(&mut self, group: &Zone) {
        let needs_canvas = self.target_canvases.get(&group.id).is_none_or(|canvas| {
            canvas.width() != group.layout.canvas_width
                || canvas.height() != group.layout.canvas_height
        });
        if needs_canvas {
            self.target_canvases.insert(
                group.id,
                Canvas::new(group.layout.canvas_width, group.layout.canvas_height),
            );
        }
    }

    fn ensure_scene_projection(&mut self, group: &Zone) {
        let needs_projection =
            self.scene_projection_cache
                .get(&group.id)
                .is_none_or(|projection| {
                    projection.scene_width != self.scene_width
                        || projection.scene_height != self.scene_height
                        || projection.layout != group.layout
                });
        if needs_projection {
            self.scene_projection_cache.insert(
                group.id,
                build_group_projection(group, self.scene_width, self.scene_height),
            );
        }
    }

    fn ensure_direct_surface_pool(&mut self, group: &Zone) {
        let descriptor =
            SurfaceDescriptor::rgba8888(group.layout.canvas_width, group.layout.canvas_height);
        let needs_pool = self
            .direct_surface_pools
            .get(&group.id)
            .is_none_or(|pool| pool.descriptor() != descriptor);
        if needs_pool {
            self.direct_surface_pools.insert(
                group.id,
                RenderSurfacePool::with_slot_count_and_cap(
                    descriptor,
                    DIRECT_SURFACE_POOL_INITIAL_SLOTS,
                    DIRECT_SURFACE_POOL_MAX_SLOTS,
                ),
            );
        }
    }

    fn ensure_spatial_engine(&mut self, group: &Zone) {
        let needs_engine = self
            .spatial_engines
            .get(&group.id)
            .is_none_or(|engine| engine.layout().as_ref() != &group.layout);
        if needs_engine {
            self.spatial_engines
                .insert(group.id, SpatialEngine::new(group.layout.clone()));
        }
    }

    fn reconcile_combined_led_state(&mut self, groups: &[Zone]) {
        let mut contributing_groups = groups
            .iter()
            .filter(|group| group_contributes_to_scene_canvas(group));
        if let Some(group) = contributing_groups.next()
            && contributing_groups.next().is_none()
            && let Some(engine) = self.spatial_engines.get(&group.id)
        {
            let engine = engine.clone();
            self.combined_led_layout = engine.layout();
            self.combined_led_spatial_engine = engine;
            return;
        }

        let (layout, engine) = combined_led_state(combine_led_group_layouts(
            groups,
            self.scene_width,
            self.scene_height,
        ));
        self.combined_led_layout = layout;
        self.combined_led_spatial_engine = engine;
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

        let mut producer_full_frame_copy = FullFrameCopyMetrics::default();
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
        let Some(scene_frame) =
            self.surface_backed_scene_frame(scene_frame, &mut producer_full_frame_copy)
        else {
            return Ok(None);
        };
        record_producer_frame(&scene_frame);
        let mut render_us = micros_u32(render_start.elapsed());

        let sample_us = 0_u32;
        if !scene_group.layout.zones.is_empty() {
            zones.clear();
        }
        let mut rendered_groups = RenderedGroupSet::default();
        rendered_groups.push_scene_group_frame(scene_group.id, scene_frame.clone());
        for group in context.groups {
            if !group.enabled || group.id == scene_group.id {
                continue;
            }
            if !group_publishes_direct_canvas(group) {
                continue;
            }

            rendered_groups.mark_direct_group_active(group.id);
            if let Some(retained) = self.reuse_retained_direct_group_frame(
                group,
                context.elapsed_ms,
                context.display_group_target_fps,
                context.dependency_key,
            ) {
                rendered_groups.push_direct_group_frame(group.id, retained);
                continue;
            }

            let render_start = Instant::now();
            let Some(frame) = self.render_direct_group_frame(
                group,
                context.group_context(),
                sparkleflinger,
                &mut producer_full_frame_copy,
            )?
            else {
                if let Some(retained) = self.reuse_latest_direct_group_frame(group) {
                    rendered_groups.push_direct_group_frame(group.id, retained);
                }
                continue;
            };
            render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
            self.retain_direct_group_frame(
                group.id,
                context.elapsed_ms,
                context.dependency_key,
                &frame,
            );
            rendered_groups.push_direct_group_frame(group.id, frame);
        }
        zones.clear();

        Ok(Some(ZoneResult {
            scene_frame,
            group_canvases: rendered_groups.group_canvases,
            zone_canvases: rendered_groups.zone_canvases,
            active_group_canvas_ids: rendered_groups.active_group_canvas_ids,
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(spatial_engine),
            producer_full_frame_copy,
            render_us,
            sample_us,
            scene_compose_us: 0,
            logical_layer_count: enabled_layer_count(scene_group),
        }))
    }

    pub(crate) fn note_effect_error(&mut self, error: &ZoneEffectError) -> Option<ZoneEffectError> {
        self.recovered_effect_error = None;
        if self.last_effect_error.as_ref() == Some(error) {
            return None;
        }

        self.last_effect_error = Some(error.clone());
        Some(error.clone())
    }

    fn clear_effect_error(&mut self) {
        if let Some(error) = self.last_effect_error.take() {
            self.recovered_effect_error = Some(error);
        }
    }

    pub(crate) fn take_recovered_effect_error(&mut self) -> Option<ZoneEffectError> {
        self.recovered_effect_error.take()
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

    fn retain_frame(
        &mut self,
        dependency_key: SceneDependencyKey,
        result: &ZoneResult,
        zones: &[ZoneColors],
    ) {
        self.retained_frame = Some(RetainedRenderGroupFrame {
            dependency_key,
            scene_frame: result.scene_frame.clone(),
            group_canvases: result.group_canvases.clone(),
            active_group_canvas_ids: result.active_group_canvas_ids.clone(),
            zone_canvases: result.zone_canvases.clone(),
            led_sampling_strategy: result.led_sampling_strategy.retain(zones),
            logical_layer_count: result.logical_layer_count,
        });
    }

    fn sample_scene_group_led_zones(&self, groups: &[Zone], zones: &mut Vec<ZoneColors>) {
        for group in groups
            .iter()
            .filter(|group| group_contributes_to_scene_canvas(group))
        {
            let Some(target) = self.target_canvases.get(&group.id) else {
                continue;
            };
            let Some(spatial_engine) = self.spatial_engines.get(&group.id) else {
                continue;
            };
            let start = zones.len();
            spatial_engine.sample_append_into_at(target, zones, start);
        }
    }

    fn reuse_retained_direct_group_frame(
        &self,
        group: &Zone,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<ZoneId, u32>,
        dependency_key: SceneDependencyKey,
    ) -> Option<PendingGroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) || !group.layout.zones.is_empty() {
            return None;
        }

        let target_fps = *display_group_target_fps.get(&group.id)?;
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        if retained.frame.empty_direct_shell != group_publishes_empty_direct_canvas(group) {
            return None;
        }
        if retained.dependency_key != dependency_key {
            return None;
        }
        let frame_interval_ms = 1000_u32.div_ceil(target_fps.max(1));
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    fn retain_direct_group_frame(
        &mut self,
        group_id: ZoneId,
        elapsed_ms: u32,
        dependency_key: SceneDependencyKey,
        frame: &PendingGroupCanvasFrame,
    ) {
        self.retained_direct_group_frames.insert(
            group_id,
            RetainedDirectGroupFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
                dependency_key,
            },
        );
    }

    fn reuse_latest_direct_group_frame(&self, group: &Zone) -> Option<PendingGroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) {
            return None;
        }
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        if retained.frame.empty_direct_shell != group_publishes_empty_direct_canvas(group) {
            return None;
        }
        let display_target = group.display_target.as_ref()?;
        if retained.frame.display_target != *display_target
            || retained.frame.frame.width() != group.layout.canvas_width
            || retained.frame.frame.height() != group.layout.canvas_height
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn reuse_retained_materialized_group_frame(
        &self,
        group_id: ZoneId,
        elapsed_ms: u32,
        target_fps: Option<u32>,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<GroupCanvasFrame> {
        let target_fps = target_fps?;
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_group_frames.get(&group_id)?;
        if retained.dependency_key != dependency_key
            || retained.display_target != *display_target
            || retained.display_route != *display_route
            || retained.empty_direct_shell != empty_direct_shell
        {
            return None;
        }

        let frame_interval_ms = 1000_u32.div_ceil(target_fps.max(1));
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    pub(crate) fn reuse_latest_materialized_group_frame(
        &self,
        group_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
    ) -> Option<GroupCanvasFrame> {
        if display_route.device_id != display_target.device_id {
            return None;
        }

        let retained = self.retained_materialized_group_frames.get(&group_id)?;
        if retained.display_target != *display_target
            || retained.display_route != *display_route
            || retained.empty_direct_shell != empty_direct_shell
        {
            return None;
        }

        Some(retained.frame.clone())
    }

    pub(crate) fn retain_materialized_group_frame(
        &mut self,
        group_id: ZoneId,
        elapsed_ms: u32,
        dependency_key: SceneDependencyKey,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        empty_direct_shell: bool,
        frame: &GroupCanvasFrame,
    ) {
        if display_route.device_id != display_target.device_id || !frame.display_target.finalized {
            return;
        }

        self.retained_materialized_group_frames.insert(
            group_id,
            RetainedMaterializedGroupFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
                dependency_key,
                display_target: display_target.clone(),
                display_route: display_route.clone(),
                empty_direct_shell,
            },
        );
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

    fn compose_scene_frame(&mut self, groups: &[Zone]) -> ProducerFrame {
        let Some(mut lease) = self.scene_surface_pool.dequeue() else {
            let mut scene_canvas = Canvas::new(self.scene_width, self.scene_height);
            compose_authoritative_scene_canvas(
                &mut scene_canvas,
                groups,
                &self.target_canvases,
                self.scene_width,
                self.scene_height,
                &self.scene_projection_cache,
            );
            let frame = ProducerFrame::Canvas(scene_canvas);
            record_producer_frame(&frame);
            return frame;
        };

        compose_authoritative_scene_canvas(
            lease.canvas_mut(),
            groups,
            &self.target_canvases,
            self.scene_width,
            self.scene_height,
            &self.scene_projection_cache,
        );

        let frame = ProducerFrame::Surface(lease.submit(0, 0));
        record_producer_frame(&frame);
        frame
    }

    fn compose_projected_scene_frame(
        &mut self,
        layers: Vec<CompositionLayer>,
        sparkleflinger: &mut SparkleFlinger,
    ) -> Option<ProducerFrame> {
        if layers.is_empty() {
            return None;
        }

        let plan = CompositionPlan::with_layers(self.scene_width, self.scene_height, layers)
            .with_cpu_replay_cacheable(false);
        let composed = sparkleflinger.compose_for_outputs(plan.clone(), false, None);
        if let Some(frame) = composed_frame_to_producer_frame(composed, sparkleflinger) {
            record_producer_frame(&frame);
            return Some(frame);
        }

        None
    }

    #[cfg(test)]
    fn compose_preview_grid_for_test(&mut self, groups: &[Zone]) -> ProducerFrame {
        let Some(mut lease) = self.scene_surface_pool.dequeue() else {
            let mut preview_grid = Canvas::new(self.scene_width, self.scene_height);
            compose_preview_grid_canvas(
                &mut preview_grid,
                groups,
                &self.target_canvases,
                self.scene_width,
                self.scene_height,
            );
            return ProducerFrame::Canvas(preview_grid);
        };

        compose_preview_grid_canvas(
            lease.canvas_mut(),
            groups,
            &self.target_canvases,
            self.scene_width,
            self.scene_height,
        );

        ProducerFrame::Surface(lease.submit(0, 0))
    }
}

#[cfg(test)]
fn compose_preview_grid_canvas(
    preview: &mut Canvas,
    groups: &[Zone],
    target_canvases: &HashMap<ZoneId, Canvas>,
    preview_width: u32,
    preview_height: u32,
) {
    let preview_count = groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .count();

    if preview_count == 0 {
        preview.clear();
        return;
    }

    if preview_count == 1
        && let Some(source) = groups
            .iter()
            .find(|group| group_contributes_to_scene_canvas(group))
            .and_then(|group| target_canvases.get(&group.id))
    {
        if source.width() == preview_width && source.height() == preview_height {
            preview
                .as_rgba_bytes_mut()
                .copy_from_slice(source.as_rgba_bytes());
            return;
        }

        blit_scaled_tile(preview, source, 0, 0, preview_width, preview_height);
        return;
    }

    let columns = tile_columns(preview_count);
    let rows = preview_count.div_ceil(columns);
    for (index, group) in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .enumerate()
    {
        let Some(source) = target_canvases.get(&group.id) else {
            continue;
        };

        let column = index % columns;
        let row = index / columns;
        let x0 = tile_origin(column, columns, preview_width);
        let x1 = tile_origin(column + 1, columns, preview_width);
        let y0 = tile_origin(row, rows, preview_height);
        let y1 = tile_origin(row + 1, rows, preview_height);
        blit_scaled_tile(preview, source, x0, y0, x1, y1);
    }
}

fn group_is_active(group: &Zone) -> bool {
    enabled_layer_count(group) > 0 || group_publishes_empty_direct_canvas(group)
}

fn group_contributes_to_scene_canvas(group: &Zone) -> bool {
    group_is_active(group) && group.display_target.is_none()
}

fn group_publishes_direct_canvas(group: &Zone) -> bool {
    group.enabled
        && group.display_target.is_some()
        && (enabled_layer_count(group) > 0 || group_publishes_empty_direct_canvas(group))
}

fn group_publishes_empty_direct_canvas(group: &Zone) -> bool {
    group.enabled
        && group.display_target.is_some()
        && group.role == ZoneRole::Display
        && enabled_layer_count(group) == 0
}

fn enabled_layer_count(group: &Zone) -> u32 {
    if !group.enabled {
        return 0;
    }
    u32::try_from(
        group
            .effective_layers()
            .into_iter()
            .filter(|layer| layer.enabled)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn desired_media_asset_ids(groups: &[Zone]) -> HashSet<AssetId> {
    groups
        .iter()
        .filter(|group| group.enabled)
        .flat_map(Zone::effective_layers)
        .filter_map(|layer| match layer.source {
            LayerSource::Media { asset_id, .. } if layer.enabled => Some(asset_id),
            _ => None,
        })
        .collect()
}

fn scene_logical_layer_count(groups: &[Zone]) -> u32 {
    groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .map(enabled_layer_count)
        .fold(0_u32, u32::saturating_add)
}

fn empty_group_layout(width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: "scene-groups".into(),
        name: "Scene Groups".into(),
        description: Some("Combined render-group routing layout".into()),
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn render_layer_effect_error(
    group: &Zone,
    layer: &SceneLayer,
    registry: &EffectRegistry,
    error: anyhow::Error,
) -> ZoneEffectError {
    let effect_id = match &layer.source {
        LayerSource::Effect { effect_id, .. } => effect_id.to_string(),
        LayerSource::WebViewport { url, .. } => format!("web_viewport:{url}"),
        _ => "unknown".to_owned(),
    };
    let effect_name = group
        .effective_layers()
        .into_iter()
        .find(|candidate| candidate.id == layer.id)
        .and_then(|layer| match layer.source {
            LayerSource::Effect { effect_id, .. } => Some(effect_id),
            _ => None,
        })
        .and_then(|effect_id| {
            registry
                .get(&effect_id)
                .map(|entry| entry.metadata.name.clone())
        })
        .unwrap_or_else(|| effect_id.clone());

    ZoneEffectError {
        effect_id,
        effect_name,
        group_id: group.id,
        group_name: group.name.clone(),
        error: error.to_string(),
    }
}

fn combine_led_group_layouts(groups: &[Zone], width: u32, height: u32) -> SpatialLayout {
    let mut layout = empty_group_layout(width, height);
    let zone_count = groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .map(|group| group.layout.zones.len())
        .sum();
    let mut zones = Vec::with_capacity(zone_count);
    for group in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
    {
        zones.extend_from_slice(&group.layout.zones);
    }
    layout.zones = zones;
    layout
}

fn combined_led_state(layout: SpatialLayout) -> (Arc<SpatialLayout>, SpatialEngine) {
    let engine = SpatialEngine::new(layout);
    (engine.layout(), engine)
}

#[cfg(test)]
fn tile_columns(count: usize) -> usize {
    let mut side = 1_usize;
    while side.saturating_mul(side) < count.max(1) {
        side = side.saturating_add(1);
    }
    side.max(1)
}

#[cfg(test)]
fn tile_origin(index: usize, count: usize, extent: u32) -> u32 {
    let numerator = u64::try_from(index)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(extent));
    let denominator = u64::try_from(count.max(1)).unwrap_or(1);
    u32::try_from(numerator / denominator).unwrap_or(extent)
}

#[cfg(test)]
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "preview tile coordinates are bounded canvas dimensions that fit comfortably in f32"
)]
fn blit_scaled_tile(target: &mut Canvas, source: &Canvas, x0: u32, y0: u32, x1: u32, y1: u32) {
    let width = x1.saturating_sub(x0);
    let height = y1.saturating_sub(y0);
    if width == 0 || height == 0 {
        return;
    }

    for y in 0..height {
        let ny = (y as f32 + 0.5) / height as f32;
        for x in 0..width {
            let nx = (x as f32 + 0.5) / width as f32;
            target.set_pixel(x0 + x, y0 + y, source.sample_bilinear(nx, ny));
        }
    }
}

mod frame_helpers;
mod projection;
#[cfg(test)]
mod tests;
