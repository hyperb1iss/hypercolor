use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::EffectRenderOutput;
use hypercolor_core::effect::{EffectPool, EffectRegistry};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::spatial::{SpatialEngine, sample_led};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{DisplayFaceTarget, RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, NormalizedPosition, SamplingMode, SpatialLayout,
};

use super::frame_sampling::{LedSamplingStrategy, RetainedLedSamplingStrategy};
use super::micros_u32;
use super::producer_queue::{ProducerFrame, record_producer_frame};
use super::scene_dependency::SceneDependencyKey;

/// Initial slot count for the full-resolution scene surface pool. Sized to absorb
/// typical downstream pins: the canvas watch channel, display-output
/// dispatch, and one in-flight JPEG encode per HTML-face worker. Undersizing
/// forces `begin_dequeue` to reallocate a fresh canvas every frame whenever
/// all slots are still shared downstream, which shows up as producer-stage
/// stalls proportional to `canvas_width * canvas_height * 4` bytes.
const SCENE_SURFACE_POOL_INITIAL_SLOTS: usize = 8;
const SCENE_SURFACE_POOL_MAX_SLOTS: usize = 64;

/// Initial slot count for per-group direct-canvas pools (HTML-face render groups).
/// Same failure mode as the scene surface pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_INITIAL_SLOTS: usize = 6;
const DIRECT_SURFACE_POOL_MAX_SLOTS: usize = 32;

#[derive(Clone)]
pub(crate) struct GroupCanvasFrame {
    pub surface: PublishedSurface,
    pub display_target: DisplayFaceTarget,
}

pub(crate) struct RenderGroupResult {
    pub scene_frame: ProducerFrame,
    pub group_canvases: Vec<(RenderGroupId, GroupCanvasFrame)>,
    pub active_group_canvas_ids: Vec<RenderGroupId>,
    pub led_sampling_strategy: LedSamplingStrategy,
    pub render_us: u32,
    pub sample_us: u32,
    pub scene_compose_us: u32,
    pub logical_layer_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("render group '{group_name}' effect '{effect_name}' ({effect_id}) failed: {error}")]
pub(crate) struct RenderGroupEffectError {
    pub(crate) effect_id: String,
    pub(crate) effect_name: String,
    pub(crate) group_id: RenderGroupId,
    pub(crate) group_name: String,
    pub(crate) error: String,
}

#[derive(Clone)]
struct RetainedRenderGroupFrame {
    dependency_key: SceneDependencyKey,
    scene_frame: ProducerFrame,
    active_group_canvas_ids: Vec<RenderGroupId>,
    led_sampling_strategy: RetainedLedSamplingStrategy,
    logical_layer_count: u32,
}

#[derive(Clone)]
struct RetainedDirectGroupFrame {
    frame: GroupCanvasFrame,
    rendered_at_ms: u32,
    dependency_key: SceneDependencyKey,
}

struct CachedGroupProjection {
    scene_width: u32,
    scene_height: u32,
    layout: SpatialLayout,
    zones: Vec<CachedZoneProjection>,
}

struct CachedZoneProjection {
    zone: DeviceZone,
    sampling_mode: SamplingMode,
    edge_behavior: EdgeBehavior,
    samples: Vec<ProjectionSample>,
}

#[derive(Clone, Copy)]
struct ProjectionSample {
    x: u32,
    y: u32,
    local_position: NormalizedPosition,
}

pub(crate) struct RenderGroupRuntime {
    effect_pool: EffectPool,
    target_canvases: HashMap<RenderGroupId, Canvas>,
    scene_projection_cache: HashMap<RenderGroupId, CachedGroupProjection>,
    spatial_engines: HashMap<RenderGroupId, SpatialEngine>,
    direct_surface_pools: HashMap<RenderGroupId, RenderSurfacePool>,
    retained_direct_group_frames: HashMap<RenderGroupId, RetainedDirectGroupFrame>,
    scene_surface_pool: RenderSurfacePool,
    reconciled_dependency_key: Option<SceneDependencyKey>,
    retained_frame: Option<RetainedRenderGroupFrame>,
    last_effect_error: Option<RenderGroupEffectError>,
    recovered_effect_error: Option<RenderGroupEffectError>,
    combined_led_layout: Arc<SpatialLayout>,
    combined_led_spatial_engine: SpatialEngine,
    scene_width: u32,
    scene_height: u32,
}

impl RenderGroupRuntime {
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Self {
        Self {
            effect_pool: EffectPool::new(),
            target_canvases: HashMap::new(),
            scene_projection_cache: HashMap::new(),
            spatial_engines: HashMap::new(),
            direct_surface_pools: HashMap::new(),
            retained_direct_group_frames: HashMap::new(),
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
            combined_led_layout: Arc::new(empty_group_layout(scene_width, scene_height)),
            combined_led_spatial_engine: SpatialEngine::new(empty_group_layout(
                scene_width,
                scene_height,
            )),
            scene_width,
            scene_height,
        }
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
    /// every direct-canvas group pool (one per HTML-face render group).
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
        self.target_canvases.clear();
        self.scene_projection_cache.clear();
        self.spatial_engines.clear();
        self.direct_surface_pools.clear();
        self.retained_direct_group_frames.clear();
        self.reconciled_dependency_key = None;
        self.retained_frame = None;
        self.last_effect_error = None;
        self.recovered_effect_error = None;
        self.combined_led_layout =
            Arc::new(empty_group_layout(self.scene_width, self.scene_height));
        self.combined_led_spatial_engine =
            SpatialEngine::new(self.combined_led_layout.as_ref().clone());
    }

    pub(crate) fn reuse_scene(
        &self,
        dependency_key: SceneDependencyKey,
    ) -> Option<RenderGroupResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.dependency_key != dependency_key {
            return None;
        }

        Some(RenderGroupResult {
            scene_frame: retained.scene_frame.clone(),
            group_canvases: Vec::new(),
            active_group_canvas_ids: retained.active_group_canvas_ids.clone(),
            led_sampling_strategy: LedSamplingStrategy::from_retained(
                &retained.led_sampling_strategy,
            ),
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
            || self.retained_frame.is_some()
            || self.reconciled_dependency_key.is_some()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "render-scene orchestration needs the full frame context plus reusable zone storage"
    )]
    pub(crate) fn render_scene(
        &mut self,
        groups: &[RenderGroup],
        dependency_key: SceneDependencyKey,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
        registry: &EffectRegistry,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<RenderGroupResult> {
        self.reconcile(groups, dependency_key, registry)?;

        if let Some(result) = self.render_single_full_scene_group(
            groups,
            elapsed_ms,
            display_group_target_fps,
            dependency_key,
            registry,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            zones,
        )? {
            self.clear_effect_error();
            self.retain_frame(dependency_key, &result, &[]);
            return Ok(result);
        }

        let mut render_us = 0_u32;
        let mut group_canvases = Vec::new();
        let mut active_group_canvas_ids = Vec::new();
        for group in groups {
            if !group.enabled || group.effect_id.is_none() {
                continue;
            }

            if group_publishes_direct_canvas(group) {
                if let Some(retained) = self.reuse_retained_direct_group_frame(
                    group,
                    elapsed_ms,
                    display_group_target_fps,
                    dependency_key,
                ) {
                    active_group_canvas_ids.push(group.id);
                    group_canvases.push((group.id, retained));
                    continue;
                }
                let Some(surface_pool) = self.direct_surface_pools.get_mut(&group.id) else {
                    continue;
                };
                let Some(mut lease) = surface_pool.dequeue() else {
                    continue;
                };
                let render_start = Instant::now();
                self.effect_pool
                    .render_group_into(
                        group,
                        delta_secs,
                        audio,
                        interaction,
                        screen,
                        sensors,
                        lease.canvas_mut(),
                    )
                    .map_err(|error| {
                        anyhow::Error::new(render_group_effect_error(group, registry, error))
                    })?;
                render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
                active_group_canvas_ids.push(group.id);
                let frame = GroupCanvasFrame {
                    surface: lease.submit(0, 0),
                    display_target: group
                        .display_target
                        .clone()
                        .expect("direct display group should carry a display target"),
                };
                self.retain_direct_group_frame(group.id, elapsed_ms, dependency_key, &frame);
                group_canvases.push((group.id, frame));
                continue;
            }

            let Some(target) = self.target_canvases.get_mut(&group.id) else {
                continue;
            };
            let render_start = Instant::now();
            self.effect_pool
                .render_group_into(
                    group,
                    delta_secs,
                    audio,
                    interaction,
                    screen,
                    sensors,
                    target,
                )
                .map_err(|error| {
                    anyhow::Error::new(render_group_effect_error(group, registry, error))
                })?;
            render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
        }
        zones.clear();
        let logical_layer_count = u32::try_from(
            groups
                .iter()
                .filter(|group| group_contributes_to_scene_canvas(group))
                .count(),
        )
        .unwrap_or(u32::MAX);
        let scene_compose_start = Instant::now();
        let scene_frame = self.compose_scene_frame(groups);
        let scene_compose_us = micros_u32(scene_compose_start.elapsed());

        let result = RenderGroupResult {
            scene_frame,
            group_canvases,
            active_group_canvas_ids,
            led_sampling_strategy: if self.combined_led_layout.zones.is_empty() {
                LedSamplingStrategy::PreSampled(Arc::clone(&self.combined_led_layout))
            } else {
                LedSamplingStrategy::SparkleFlinger(self.combined_led_spatial_engine.clone())
            },
            render_us,
            sample_us: 0,
            scene_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(dependency_key, &result, zones);
        Ok(result)
    }

    fn reconcile(
        &mut self,
        groups: &[RenderGroup],
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
    ) -> Result<()> {
        if self.reconciled_dependency_key == Some(dependency_key) {
            return Ok(());
        }

        self.effect_pool.reconcile(groups, registry)?;

        let desired_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
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

        self.combined_led_layout = Arc::new(combine_led_group_layouts(
            groups,
            self.scene_width,
            self.scene_height,
        ));
        self.combined_led_spatial_engine =
            SpatialEngine::new(self.combined_led_layout.as_ref().clone());
        self.reconciled_dependency_key = Some(dependency_key);

        Ok(())
    }

    fn ensure_group_canvas(&mut self, group: &RenderGroup) {
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

    fn ensure_scene_projection(&mut self, group: &RenderGroup) {
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

    fn ensure_direct_surface_pool(&mut self, group: &RenderGroup) {
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

    fn ensure_spatial_engine(&mut self, group: &RenderGroup) {
        let needs_engine = self
            .spatial_engines
            .get(&group.id)
            .is_none_or(|engine| engine.layout().as_ref() != &group.layout);
        if needs_engine {
            self.spatial_engines
                .insert(group.id, SpatialEngine::new(group.layout.clone()));
        }
    }

    fn render_single_full_scene_group(
        &mut self,
        groups: &[RenderGroup],
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<Option<RenderGroupResult>> {
        let Some(scene_group) = self.single_full_scene_group(groups) else {
            return Ok(None);
        };
        let Some(spatial_engine) = self.spatial_engines.get(&scene_group.id).cloned() else {
            return Ok(None);
        };

        let render_start = Instant::now();
        #[cfg(feature = "servo-gpu-import")]
        let scene_frame = match self
            .effect_pool
            .render_group_output(scene_group, delta_secs, audio, interaction, screen, sensors)
            .map_err(|error| {
                anyhow::Error::new(render_group_effect_error(scene_group, registry, error))
            })? {
            EffectRenderOutput::Cpu(canvas) => {
                let Some(mut lease) = self.scene_surface_pool.dequeue() else {
                    return Ok(None);
                };
                *lease.canvas_mut() = canvas;
                ProducerFrame::Surface(lease.submit(0, 0))
            }
            EffectRenderOutput::Gpu(frame) => ProducerFrame::Gpu(frame),
        };
        #[cfg(not(feature = "servo-gpu-import"))]
        let scene_frame = {
            let Some(mut lease) = self.scene_surface_pool.dequeue() else {
                return Ok(None);
            };
            if let Err(error) = self.effect_pool.render_group_into(
                scene_group,
                delta_secs,
                audio,
                interaction,
                screen,
                sensors,
                lease.canvas_mut(),
            ) {
                lease.release();
                return Err(anyhow::Error::new(render_group_effect_error(
                    scene_group,
                    registry,
                    error,
                )));
            }
            ProducerFrame::Surface(lease.submit(0, 0))
        };
        record_producer_frame(&scene_frame);
        let mut render_us = micros_u32(render_start.elapsed());

        let sample_us = 0_u32;
        if !scene_group.layout.zones.is_empty() {
            zones.clear();
        }
        let mut group_canvases = Vec::new();
        let mut active_group_canvas_ids = Vec::new();
        for group in groups {
            if !group.enabled || group.effect_id.is_none() || group.id == scene_group.id {
                continue;
            }
            if !group_publishes_direct_canvas(group) {
                continue;
            }

            if let Some(retained) = self.reuse_retained_direct_group_frame(
                group,
                elapsed_ms,
                display_group_target_fps,
                dependency_key,
            ) {
                active_group_canvas_ids.push(group.id);
                group_canvases.push((group.id, retained));
                continue;
            }

            let Some(surface_pool) = self.direct_surface_pools.get_mut(&group.id) else {
                continue;
            };
            let Some(mut group_lease) = surface_pool.dequeue() else {
                continue;
            };
            let render_start = Instant::now();
            self.effect_pool
                .render_group_into(
                    group,
                    delta_secs,
                    audio,
                    interaction,
                    screen,
                    sensors,
                    group_lease.canvas_mut(),
                )
                .map_err(|error| {
                    anyhow::Error::new(render_group_effect_error(group, registry, error))
                })?;
            render_us = render_us.saturating_add(micros_u32(render_start.elapsed()));
            active_group_canvas_ids.push(group.id);
            let frame = GroupCanvasFrame {
                surface: group_lease.submit(0, 0),
                display_target: group
                    .display_target
                    .clone()
                    .expect("direct display group should carry a display target"),
            };
            self.retain_direct_group_frame(group.id, elapsed_ms, dependency_key, &frame);
            group_canvases.push((group.id, frame));
        }
        zones.clear();

        Ok(Some(RenderGroupResult {
            scene_frame,
            group_canvases,
            active_group_canvas_ids,
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(spatial_engine),
            render_us,
            sample_us,
            scene_compose_us: 0,
            logical_layer_count: 1,
        }))
    }

    pub(crate) fn note_effect_error(
        &mut self,
        error: &RenderGroupEffectError,
    ) -> Option<RenderGroupEffectError> {
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

    pub(crate) fn take_recovered_effect_error(&mut self) -> Option<RenderGroupEffectError> {
        self.recovered_effect_error.take()
    }

    fn single_full_scene_group<'a>(&self, groups: &'a [RenderGroup]) -> Option<&'a RenderGroup> {
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
        result: &RenderGroupResult,
        zones: &[ZoneColors],
    ) {
        self.retained_frame = Some(RetainedRenderGroupFrame {
            dependency_key,
            scene_frame: result.scene_frame.clone(),
            active_group_canvas_ids: result.active_group_canvas_ids.clone(),
            led_sampling_strategy: result.led_sampling_strategy.retain(zones),
            logical_layer_count: result.logical_layer_count,
        });
    }

    fn reuse_retained_direct_group_frame(
        &self,
        group: &RenderGroup,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
        dependency_key: SceneDependencyKey,
    ) -> Option<GroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) || !group.layout.zones.is_empty() {
            return None;
        }

        let target_fps = *display_group_target_fps.get(&group.id)?;
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        if retained.dependency_key != dependency_key {
            return None;
        }
        let frame_interval_ms = 1000_u32.div_ceil(target_fps.max(1));
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    fn retain_direct_group_frame(
        &mut self,
        group_id: RenderGroupId,
        elapsed_ms: u32,
        dependency_key: SceneDependencyKey,
        frame: &GroupCanvasFrame,
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

    fn compose_scene_frame(&mut self, groups: &[RenderGroup]) -> ProducerFrame {
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

    #[cfg(test)]
    fn compose_preview_grid_for_test(&mut self, groups: &[RenderGroup]) -> ProducerFrame {
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

fn compose_authoritative_scene_canvas(
    scene_canvas: &mut Canvas,
    groups: &[RenderGroup],
    target_canvases: &HashMap<RenderGroupId, Canvas>,
    scene_width: u32,
    scene_height: u32,
    scene_projection_cache: &HashMap<RenderGroupId, CachedGroupProjection>,
) {
    scene_canvas.clear();

    for group in groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
    {
        let Some(source) = target_canvases.get(&group.id) else {
            continue;
        };

        let Some(projection) = scene_projection_cache.get(&group.id) else {
            for zone in &group.layout.zones {
                blit_zone_projection(
                    scene_canvas,
                    source,
                    zone,
                    &group.layout,
                    scene_width,
                    scene_height,
                );
            }
            continue;
        };

        for zone_projection in &projection.zones {
            for sample in &zone_projection.samples {
                scene_canvas.set_pixel(
                    sample.x,
                    sample.y,
                    sample_led(
                        source,
                        sample.local_position,
                        &zone_projection.zone,
                        &zone_projection.sampling_mode,
                        zone_projection.edge_behavior,
                    ),
                );
            }
        }
    }
}

fn build_group_projection(
    group: &RenderGroup,
    scene_width: u32,
    scene_height: u32,
) -> CachedGroupProjection {
    CachedGroupProjection {
        scene_width,
        scene_height,
        layout: group.layout.clone(),
        zones: group
            .layout
            .zones
            .iter()
            .map(|zone| build_zone_projection(zone, &group.layout, scene_width, scene_height))
            .collect(),
    }
}

fn build_zone_projection(
    zone: &DeviceZone,
    layout: &SpatialLayout,
    target_width: u32,
    target_height: u32,
) -> CachedZoneProjection {
    let sampling_mode = zone
        .sampling_mode
        .clone()
        .unwrap_or_else(|| layout.default_sampling_mode.clone());
    let edge_behavior = zone.edge_behavior.unwrap_or(layout.default_edge_behavior);
    let Some((x0, y0, x1, y1)) = zone_projection_bounds(zone, target_width, target_height) else {
        return CachedZoneProjection {
            zone: zone.clone(),
            sampling_mode,
            edge_behavior,
            samples: Vec::new(),
        };
    };

    let mut samples = Vec::with_capacity(
        usize::try_from(u64::from(x1 - x0) * u64::from(y1 - y0)).unwrap_or(usize::MAX),
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let Some(local_position) =
                zone_local_position_for_scene_pixel(x, y, target_width, target_height, zone)
            else {
                continue;
            };
            samples.push(ProjectionSample {
                x,
                y,
                local_position,
            });
        }
    }

    CachedZoneProjection {
        zone: zone.clone(),
        sampling_mode,
        edge_behavior,
        samples,
    }
}

fn blit_zone_projection(
    target: &mut Canvas,
    source: &Canvas,
    zone: &DeviceZone,
    layout: &SpatialLayout,
    target_width: u32,
    target_height: u32,
) {
    let projection = build_zone_projection(zone, layout, target_width, target_height);
    for sample in projection.samples {
        target.set_pixel(
            sample.x,
            sample.y,
            sample_led(
                source,
                sample.local_position,
                &projection.zone,
                &projection.sampling_mode,
                projection.edge_behavior,
            ),
        );
    }
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "zone projection rasterizes bounded normalized geometry into scene pixels"
)]
fn zone_projection_bounds(
    zone: &DeviceZone,
    target_width: u32,
    target_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let span_x = zone.size.x * zone.scale;
    let span_y = zone.size.y * zone.scale;
    if span_x <= 0.0 || span_y <= 0.0 || target_width == 0 || target_height == 0 {
        return None;
    }

    let half_x = span_x * 0.5;
    let half_y = span_y * 0.5;
    let cos_t = zone.rotation.cos();
    let sin_t = zone.rotation.sin();
    let corners = [
        (-half_x, -half_y),
        (half_x, -half_y),
        (half_x, half_y),
        (-half_x, half_y),
    ];
    let (mut start_x, mut end_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut start_y, mut end_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for (corner_x, corner_y) in corners {
        let rotated_x = corner_x.mul_add(cos_t, -corner_y * sin_t);
        let rotated_y = corner_x.mul_add(sin_t, corner_y * cos_t);
        let scene_x = zone.position.x + rotated_x;
        let scene_y = zone.position.y + rotated_y;
        start_x = start_x.min(scene_x);
        end_x = end_x.max(scene_x);
        start_y = start_y.min(scene_y);
        end_y = end_y.max(scene_y);
    }

    start_x = start_x.clamp(0.0, 1.0);
    start_y = start_y.clamp(0.0, 1.0);
    end_x = end_x.clamp(0.0, 1.0);
    end_y = end_y.clamp(0.0, 1.0);
    if end_x <= start_x || end_y <= start_y {
        return None;
    }

    let x0 = ((start_x * target_width as f32).floor() as u32).min(target_width);
    let x1 = ((end_x * target_width as f32).ceil() as u32).min(target_width);
    let y0 = ((start_y * target_height as f32).floor() as u32).min(target_height);
    let y1 = ((end_y * target_height as f32).ceil() as u32).min(target_height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    Some((x0, y0, x1, y1))
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "scene pixel centers are rasterized into normalized scene coordinates"
)]
fn zone_local_position_for_scene_pixel(
    x: u32,
    y: u32,
    target_width: u32,
    target_height: u32,
    zone: &DeviceZone,
) -> Option<hypercolor_types::spatial::NormalizedPosition> {
    if target_width == 0 || target_height == 0 {
        return None;
    }

    let span_x = zone.size.x * zone.scale;
    let span_y = zone.size.y * zone.scale;
    if span_x <= 0.0 || span_y <= 0.0 {
        return None;
    }

    let scene_x = (x as f32 + 0.5) / target_width as f32;
    let scene_y = (y as f32 + 0.5) / target_height as f32;
    let delta_x = scene_x - zone.position.x;
    let delta_y = scene_y - zone.position.y;
    let cos_t = zone.rotation.cos();
    let sin_t = zone.rotation.sin();
    let local_x = (delta_x.mul_add(cos_t, delta_y * sin_t) / span_x) + 0.5;
    let local_y = (delta_y.mul_add(cos_t, -delta_x * sin_t) / span_y) + 0.5;
    if !(0.0..=1.0).contains(&local_x) || !(0.0..=1.0).contains(&local_y) {
        return None;
    }

    Some(NormalizedPosition::new(local_x, local_y))
}

#[cfg(test)]
fn compose_preview_grid_canvas(
    preview: &mut Canvas,
    groups: &[RenderGroup],
    target_canvases: &HashMap<RenderGroupId, Canvas>,
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

fn group_is_active(group: &RenderGroup) -> bool {
    group.enabled && group.effect_id.is_some()
}

fn group_contributes_to_scene_canvas(group: &RenderGroup) -> bool {
    group_is_active(group) && group.display_target.is_none()
}

fn group_publishes_direct_canvas(group: &RenderGroup) -> bool {
    group_is_active(group) && group.display_target.is_some()
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

fn render_group_effect_error(
    group: &RenderGroup,
    registry: &EffectRegistry,
    error: anyhow::Error,
) -> RenderGroupEffectError {
    let effect_id = group
        .effect_id
        .map_or_else(|| "unknown".to_owned(), |effect_id| effect_id.to_string());
    let effect_name = group
        .effect_id
        .and_then(|effect_id| {
            registry
                .get(&effect_id)
                .map(|entry| entry.metadata.name.clone())
        })
        .unwrap_or_else(|| effect_id.clone());

    RenderGroupEffectError {
        effect_id,
        effect_name,
        group_id: group.id,
        group_name: group.name.clone(),
        error: error.to_string(),
    }
}

fn combine_led_group_layouts(groups: &[RenderGroup], width: u32, height: u32) -> SpatialLayout {
    let mut layout = empty_group_layout(width, height);
    layout.zones = groups
        .iter()
        .filter(|group| group_contributes_to_scene_canvas(group))
        .flat_map(|group| group.layout.zones.clone())
        .collect();
    layout
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::f32::consts::FRAC_PI_4;

    use hypercolor_core::effect::EffectRegistry;
    use hypercolor_core::effect::builtin::register_builtin_effects;
    use hypercolor_core::input::InteractionData;
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::canvas::Rgba;
    use hypercolor_types::effect::{ControlValue, EffectId};
    use hypercolor_types::scene::RenderGroupRole;
    use hypercolor_types::spatial::{DeviceZone, LedTopology, NormalizedPosition, StripDirection};
    use uuid::Uuid;

    use super::*;

    fn sample_group(width: u32, height: u32) -> RenderGroup {
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Preview Group".into(),
            description: None,
            effect_id: Some(EffectId::from(Uuid::now_v7())),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: SpatialLayout {
                id: "preview-group".into(),
                name: "Preview Group".into(),
                description: None,
                canvas_width: width,
                canvas_height: height,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
            controls_version: 0,
        }
    }

    fn sample_display_group(width: u32, height: u32) -> RenderGroup {
        let mut group = sample_group(width, height);
        group.display_target = Some(hypercolor_types::scene::DisplayFaceTarget {
            device_id: hypercolor_types::device::DeviceId::new(),
            blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Replace,
            opacity: 1.0,
        });
        group.role = RenderGroupRole::Display;
        group
    }

    fn point_zone(id: &str) -> DeviceZone {
        DeviceZone {
            id: id.into(),
            name: id.into(),
            device_id: id.into(),
            zone_name: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 0.2, y: 0.2 },
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 1,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }
    }

    fn rotated_zone(id: &str, rotation: f32, size: f32) -> DeviceZone {
        let mut zone = point_zone(id);
        zone.size = NormalizedPosition { x: size, y: size };
        zone.rotation = rotation;
        zone
    }

    fn point_zone_at(id: &str, x: f32, y: f32) -> DeviceZone {
        let mut zone = point_zone(id);
        zone.position = NormalizedPosition::new(x, y);
        zone.size = NormalizedPosition::new(0.4, 0.4);
        zone
    }

    fn builtin_registry() -> EffectRegistry {
        let mut registry = EffectRegistry::new(Vec::new());
        register_builtin_effects(&mut registry);
        registry
    }

    fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
        registry
            .iter()
            .find_map(|(id, entry)| {
                (entry.metadata.source.source_stem() == Some(stem)).then_some(*id)
            })
            .expect("builtin effect should exist")
    }

    fn builtin_entry(
        registry: &EffectRegistry,
        stem: &str,
    ) -> hypercolor_core::effect::EffectEntry {
        registry
            .iter()
            .find_map(|(_, entry)| {
                (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.clone())
            })
            .expect("builtin effect should exist")
    }

    fn render_scene_for_test(
        runtime: &mut RenderGroupRuntime,
        groups: &[RenderGroup],
        groups_revision: u64,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
        registry: &EffectRegistry,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<RenderGroupResult> {
        runtime.render_scene(
            groups,
            SceneDependencyKey::new(groups_revision, registry.generation()),
            elapsed_ms,
            display_group_target_fps,
            registry,
            1.0 / 60.0,
            &AudioData::silence(),
            &InteractionData::default(),
            None,
            &SystemSnapshot::empty(),
            zones,
        )
    }

    fn blit_general_zone_projection(
        target: &mut Canvas,
        source: &Canvas,
        zone: &DeviceZone,
        sampling_mode: &SamplingMode,
        edge_behavior: EdgeBehavior,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        target_width: u32,
        target_height: u32,
    ) {
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(local_position) =
                    zone_local_position_for_scene_pixel(x, y, target_width, target_height, zone)
                else {
                    continue;
                };
                target.set_pixel(
                    x,
                    y,
                    sample_led(source, local_position, zone, sampling_mode, edge_behavior),
                );
            }
        }
    }

    fn canvas_from_scene_frame(frame: &ProducerFrame) -> Canvas {
        match frame {
            ProducerFrame::Canvas(canvas) => canvas.clone(),
            ProducerFrame::Surface(surface) => {
                Canvas::from_rgba(surface.rgba_bytes(), surface.width(), surface.height())
            }
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => {
                panic!("GPU scene frames are sampled by SparkleFlinger before CPU materialization")
            }
        }
    }

    #[test]
    fn note_effect_error_dedupes_until_cleared() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let error = RenderGroupEffectError {
            effect_id: "effect-1".into(),
            effect_name: "Test Effect".into(),
            group_id: RenderGroupId::new(),
            group_name: "Test Group".into(),
            error: "boom".into(),
        };

        assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
        assert_eq!(runtime.note_effect_error(&error), None);

        runtime.clear_effect_error();

        assert_eq!(runtime.note_effect_error(&error), Some(error));
    }

    #[test]
    fn recovered_effect_error_is_reported_once_after_clear() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let error = RenderGroupEffectError {
            effect_id: "effect-1".into(),
            effect_name: "Test Effect".into(),
            group_id: RenderGroupId::new(),
            group_name: "Test Group".into(),
            error: "boom".into(),
        };

        assert_eq!(runtime.note_effect_error(&error), Some(error.clone()));
        runtime.clear_effect_error();

        assert_eq!(runtime.take_recovered_effect_error(), Some(error));
        assert_eq!(runtime.take_recovered_effect_error(), None);
    }

    #[test]
    fn clear_inactive_groups_releases_cached_group_state() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let group = sample_group(4, 4);
        runtime.target_canvases.insert(group.id, Canvas::new(4, 4));
        runtime
            .spatial_engines
            .insert(group.id, SpatialEngine::new(group.layout.clone()));
        runtime.reconciled_dependency_key = Some(SceneDependencyKey::new(1, 1));

        assert!(runtime.has_inactive_group_resources());

        runtime.clear_inactive_groups();

        assert!(!runtime.has_inactive_group_resources());
        assert!(runtime.combined_led_layout.zones.is_empty());
    }

    #[test]
    fn single_group_preview_publishes_surface_frame() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let group = sample_group(4, 4);
        let mut source = Canvas::new(4, 4);
        source.fill(Rgba::new(12, 34, 56, 255));
        runtime.target_canvases.insert(group.id, source);

        let preview = runtime.compose_preview_grid_for_test(&[group]);
        let ProducerFrame::Surface(surface) = preview else {
            panic!("single-group preview should publish a pooled surface");
        };

        assert_eq!(surface.width(), 4);
        assert_eq!(surface.height(), 4);
        assert_eq!(surface.get_pixel(0, 0), Rgba::new(12, 34, 56, 255));
        assert_eq!(surface.get_pixel(3, 3), Rgba::new(12, 34, 56, 255));
    }

    #[test]
    fn single_group_preview_scales_group_canvas_to_preview_extent() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let group = sample_group(2, 2);
        let mut source = Canvas::new(2, 2);
        source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
        source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
        source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
        source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
        runtime.target_canvases.insert(group.id, source);

        let preview = runtime.compose_preview_grid_for_test(&[group]);
        let ProducerFrame::Surface(surface) = preview else {
            panic!("scaled single-group preview should publish a pooled surface");
        };

        let top_left = surface.get_pixel(0, 0);
        let top_right = surface.get_pixel(3, 0);
        let bottom_left = surface.get_pixel(0, 3);
        let bottom_right = surface.get_pixel(3, 3);

        assert_eq!(surface.width(), 4);
        assert_eq!(surface.height(), 4);
        assert!(top_left.r > top_left.g && top_left.r > top_left.b);
        assert!(top_right.g > top_right.r && top_right.g > top_right.b);
        assert!(bottom_left.b > bottom_left.r && bottom_left.b > bottom_left.g);
        assert!(bottom_right.r > 180 && bottom_right.g > 180 && bottom_right.b < 120);
    }

    #[test]
    fn compose_preview_ignores_display_groups() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let preview_group = sample_group(4, 4);
        let display_group = sample_display_group(4, 4);
        let mut preview_canvas = Canvas::new(4, 4);
        preview_canvas.fill(Rgba::new(255, 0, 0, 255));
        let mut display_canvas = Canvas::new(4, 4);
        display_canvas.fill(Rgba::new(0, 0, 255, 255));
        runtime
            .target_canvases
            .insert(preview_group.id, preview_canvas);
        runtime
            .target_canvases
            .insert(display_group.id, display_canvas);

        let preview = runtime.compose_preview_grid_for_test(&[preview_group, display_group]);
        let ProducerFrame::Surface(surface) = preview else {
            panic!("mixed preview should publish a pooled surface");
        };

        assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(surface.get_pixel(3, 3), Rgba::new(255, 0, 0, 255));
    }

    #[test]
    fn authoritative_scene_canvas_clips_rotated_zone_geometry() {
        let mut runtime = RenderGroupRuntime::new(8, 8);
        let mut group = sample_group(8, 8);
        group.layout.zones = vec![rotated_zone("zone_rotated", FRAC_PI_4, 0.5)];
        let mut source = Canvas::new(8, 8);
        source.fill(Rgba::new(255, 0, 0, 255));
        runtime.target_canvases.insert(group.id, source);

        let scene_frame = runtime.compose_scene_frame(&[group]);
        let ProducerFrame::Surface(surface) = scene_frame else {
            panic!("authoritative scene canvas should publish a pooled surface");
        };

        assert_eq!(
            surface.get_pixel(1, 1),
            Rgba::new(0, 0, 0, 255),
            "pixels outside the rotated zone should remain untouched"
        );
        assert_eq!(
            surface.get_pixel(3, 3),
            Rgba::new(255, 0, 0, 255),
            "pixels inside the rotated zone should sample the source canvas"
        );
    }

    #[test]
    fn authoritative_scene_canvas_preserves_group_overlap_order() {
        let mut runtime = RenderGroupRuntime::new(8, 8);
        let mut back_group = sample_group(8, 8);
        back_group.layout.zones = vec![rotated_zone("zone_back", FRAC_PI_4, 0.5)];
        let mut front_group = sample_group(8, 8);
        front_group.layout.zones = vec![point_zone("zone_front")];
        front_group.layout.zones[0].size = NormalizedPosition { x: 0.25, y: 0.25 };

        let mut back_source = Canvas::new(8, 8);
        back_source.fill(Rgba::new(255, 0, 0, 255));
        let mut front_source = Canvas::new(8, 8);
        front_source.fill(Rgba::new(0, 0, 255, 255));
        runtime.target_canvases.insert(back_group.id, back_source);
        runtime.target_canvases.insert(front_group.id, front_source);

        let scene_frame = runtime.compose_scene_frame(&[back_group, front_group]);
        let ProducerFrame::Surface(surface) = scene_frame else {
            panic!("authoritative scene canvas should publish a pooled surface");
        };

        assert_eq!(
            surface.get_pixel(4, 4),
            Rgba::new(0, 0, 255, 255),
            "later groups should overwrite earlier groups in overlapping regions"
        );
        assert_eq!(
            surface.get_pixel(2, 4),
            Rgba::new(255, 0, 0, 255),
            "pixels only covered by the back group should keep its content"
        );
    }

    #[test]
    fn authoritative_scene_canvas_uses_zone_sampling_mode() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let mut group = sample_group(2, 2);
        group.layout.zones = vec![point_zone("zone_sampling")];
        group.layout.zones[0].size = NormalizedPosition { x: 1.0, y: 1.0 };
        group.layout.zones[0].sampling_mode = Some(SamplingMode::Nearest);
        let mut source = Canvas::new(2, 2);
        source.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
        source.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
        source.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
        source.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
        runtime.target_canvases.insert(group.id, source);

        let scene_frame = runtime.compose_scene_frame(&[group]);
        let ProducerFrame::Surface(surface) = scene_frame else {
            panic!("authoritative scene canvas should publish a pooled surface");
        };

        assert_eq!(surface.get_pixel(1, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(surface.get_pixel(2, 0), Rgba::new(0, 255, 0, 255));
        assert_eq!(surface.get_pixel(1, 3), Rgba::new(0, 0, 255, 255));
        assert_eq!(surface.get_pixel(2, 3), Rgba::new(255, 255, 0, 255));
    }

    #[test]
    fn render_scene_reuses_projection_cache_until_layout_changes() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut group = sample_group(2, 2);
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        group.layout.zones = vec![point_zone_at("zone_cached", 0.25, 0.5)];
        let display_group_target_fps = HashMap::new();
        let mut zones = Vec::new();

        render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("first render should build the projection cache");
        let cached_samples = runtime
            .scene_projection_cache
            .get(&group.id)
            .expect("scene group should have a cached projection")
            .zones[0]
            .samples
            .as_ptr();

        render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            16,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("same dependency key should keep the projection cache");

        assert_eq!(
            runtime
                .scene_projection_cache
                .get(&group.id)
                .expect("scene group should keep a cached projection")
                .zones[0]
                .samples
                .as_ptr(),
            cached_samples
        );

        group.layout.zones[0].size = NormalizedPosition::new(1.0, 1.0);
        render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            2,
            32,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("layout changes should rebuild the projection cache");

        assert!(
            runtime
                .scene_projection_cache
                .get(&group.id)
                .expect("scene group should rebuild a cached projection")
                .zones[0]
                .samples
                .len()
                > 4
        );
    }

    #[test]
    fn axis_aligned_bilinear_fast_path_matches_general_projection() {
        let mut zone = point_zone("zone_fast_bilinear");
        zone.position = NormalizedPosition::new(0.5, 0.5);
        zone.size = NormalizedPosition::new(0.75, 0.5);
        zone.scale = 1.0;
        zone.rotation = 0.0;
        zone.sampling_mode = Some(SamplingMode::Bilinear);
        let layout = SpatialLayout {
            id: "fast-path-layout".into(),
            name: "Fast Path Layout".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![zone.clone()],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        };
        let mut source = Canvas::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                source.set_pixel(
                    x,
                    y,
                    Rgba::new((x * 40) as u8, (y * 50) as u8, ((x + y) * 30) as u8, 255),
                );
            }
        }
        let mut fast = Canvas::new(8, 8);
        let mut general = Canvas::new(8, 8);

        blit_zone_projection(&mut fast, &source, &zone, &layout, 8, 8);
        blit_general_zone_projection(
            &mut general,
            &source,
            &zone,
            zone.sampling_mode
                .as_ref()
                .expect("sampling mode should be set"),
            EdgeBehavior::Clamp,
            0,
            0,
            8,
            8,
            8,
            8,
        );

        assert_eq!(fast.as_rgba_bytes(), general.as_rgba_bytes());
    }

    #[test]
    fn axis_aligned_nearest_fast_path_matches_general_projection() {
        let mut zone = point_zone("zone_fast_nearest");
        zone.position = NormalizedPosition::new(0.35, 0.6);
        zone.size = NormalizedPosition::new(0.5, 0.5);
        zone.scale = 1.0;
        zone.rotation = 0.0;
        zone.sampling_mode = Some(SamplingMode::Nearest);
        let layout = SpatialLayout {
            id: "fast-path-layout-nearest".into(),
            name: "Fast Path Layout Nearest".into(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones: vec![zone.clone()],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        };
        let mut source = Canvas::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                source.set_pixel(
                    x,
                    y,
                    Rgba::new((x * 60) as u8, (y * 70) as u8, ((x + y) * 20) as u8, 255),
                );
            }
        }
        let mut fast = Canvas::new(8, 8);
        let mut general = Canvas::new(8, 8);

        blit_zone_projection(&mut fast, &source, &zone, &layout, 8, 8);
        blit_general_zone_projection(
            &mut general,
            &source,
            &zone,
            zone.sampling_mode
                .as_ref()
                .expect("sampling mode should be set"),
            EdgeBehavior::Clamp,
            0,
            0,
            8,
            8,
            8,
            8,
        );

        assert_eq!(fast.as_rgba_bytes(), general.as_rgba_bytes());
    }

    #[test]
    fn single_full_scene_group_renders_directly_into_surface() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let producer_counts_before = crate::render_thread::producer_frame_counts();
        let group = RenderGroup {
            id: RenderGroupId::new(),
            name: "Direct".into(),
            description: None,
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: SpatialLayout {
                id: "direct-group".into(),
                name: "Direct Group".into(),
                description: None,
                canvas_width: 4,
                canvas_height: 4,
                zones: vec![point_zone("zone_direct")],
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
            controls_version: 0,
        };
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("single group should render");

        let ProducerFrame::Surface(surface) = &result.scene_frame else {
            panic!("single full-size group should render into a surface");
        };
        let LedSamplingStrategy::SparkleFlinger(spatial_engine) =
            result.led_sampling_strategy.clone()
        else {
            panic!("single full-size group should hand LED sampling to SparkleFlinger");
        };
        let sampled = spatial_engine.sample(&Canvas::from_rgba(
            surface.rgba_bytes(),
            surface.width(),
            surface.height(),
        ));

        assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(result.sample_us, 0);
        assert!(zones.is_empty());
        assert_eq!(sampled.len(), 1);
        assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
        assert!(
            crate::render_thread::producer_frame_counts().cpu_frames_total
                > producer_counts_before.cpu_frames_total
        );
        assert_eq!(
            runtime
                .target_canvases
                .get(&group.id)
                .expect("reconcile should provision a group canvas")
                .get_pixel(0, 0),
            Rgba::new(0, 0, 0, 255)
        );
    }

    #[test]
    fn single_full_display_group_keeps_shared_scene_canvas_blank() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut group = sample_display_group(4, 4);
        group.name = "Display".into();
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("single display group should render");

        let ProducerFrame::Surface(scene_surface) = result.scene_frame else {
            panic!("single display group should render into a surface");
        };
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a surface-backed direct canvas");
        };

        assert_eq!(result.logical_layer_count, 0);
        assert_eq!(scene_surface.get_pixel(0, 0), Rgba::new(0, 0, 0, 255));
        assert_eq!(
            group_canvas_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
        );
        assert!(zones.is_empty());
    }

    #[test]
    fn full_scene_group_with_display_group_keeps_display_faces_out_of_led_sampling() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut scene_group = sample_group(4, 4);
        scene_group.effect_id = Some(solid_id);
        scene_group.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        scene_group.layout.zones = vec![point_zone("zone_preview")];
        let mut display_group = sample_display_group(4, 4);
        display_group.effect_id = Some(solid_id);
        display_group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
        display_group.layout.zones = vec![point_zone("zone_display")];
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            &[scene_group.clone(), display_group.clone()],
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("mixed scene and display groups should render");

        let ProducerFrame::Surface(scene_surface) = &result.scene_frame else {
            panic!("mixed full-scene render should publish a surface-backed scene canvas");
        };
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };
        let LedSamplingStrategy::SparkleFlinger(spatial_engine) =
            result.led_sampling_strategy.clone()
        else {
            panic!("single scene group should hand LED sampling to SparkleFlinger");
        };
        let sampled = spatial_engine.sample(&Canvas::from_rgba(
            scene_surface.rgba_bytes(),
            scene_surface.width(),
            scene_surface.height(),
        ));
        let reused = runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .expect("retained scene should be reusable");
        let LedSamplingStrategy::SparkleFlinger(reused_spatial_engine) =
            reused.led_sampling_strategy
        else {
            panic!("retained single-scene render should stay SparkleFlinger-owned");
        };

        assert_eq!(scene_surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(
            group_canvas_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
        );
        assert_eq!(result.sample_us, 0);
        assert!(zones.is_empty());
        assert_eq!(sampled.len(), 1);
        assert_eq!(sampled[0].zone_id, "zone_preview");
        assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(reused_spatial_engine.layout().zones.len(), 1);
        assert_eq!(reused_spatial_engine.layout().zones[0].id, "zone_preview");
    }

    #[test]
    fn multiple_custom_groups_render_distinct_zone_colors() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let groups = vec![
            RenderGroup {
                id: RenderGroupId::new(),
                name: "Left".into(),
                description: None,
                effect_id: Some(solid_id),
                controls: HashMap::from([(
                    "color".into(),
                    ControlValue::Color([1.0, 0.0, 0.0, 1.0]),
                )]),
                control_bindings: HashMap::new(),
                preset_id: None,
                layout: SpatialLayout {
                    id: "left-group".into(),
                    name: "Left Group".into(),
                    description: None,
                    canvas_width: 4,
                    canvas_height: 4,
                    zones: vec![point_zone_at("zone_left", 0.25, 0.5)],
                    default_sampling_mode: SamplingMode::Bilinear,
                    default_edge_behavior: EdgeBehavior::Clamp,
                    spaces: None,
                    version: 1,
                },
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: None,
                role: RenderGroupRole::Custom,
                controls_version: 0,
            },
            RenderGroup {
                id: RenderGroupId::new(),
                name: "Right".into(),
                description: None,
                effect_id: Some(solid_id),
                controls: HashMap::from([(
                    "color".into(),
                    ControlValue::Color([0.0, 0.0, 1.0, 1.0]),
                )]),
                control_bindings: HashMap::new(),
                preset_id: None,
                layout: SpatialLayout {
                    id: "right-group".into(),
                    name: "Right Group".into(),
                    description: None,
                    canvas_width: 4,
                    canvas_height: 4,
                    zones: vec![point_zone_at("zone_right", 0.75, 0.5)],
                    default_sampling_mode: SamplingMode::Bilinear,
                    default_edge_behavior: EdgeBehavior::Clamp,
                    spaces: None,
                    version: 1,
                },
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: None,
                role: RenderGroupRole::Custom,
                controls_version: 0,
            },
        ];
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            &groups,
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("multiple groups should render");

        let LedSamplingStrategy::SparkleFlinger(spatial_engine) =
            result.led_sampling_strategy.clone()
        else {
            panic!("multi-group LED scenes should now sample from the canonical scene canvas");
        };
        let sampled = spatial_engine.sample(&canvas_from_scene_frame(&result.scene_frame));

        assert_eq!(result.logical_layer_count, 2);
        assert!(zones.is_empty());
        assert_eq!(sampled.len(), 2);
        assert_eq!(sampled[0].zone_id, "zone_left");
        assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(sampled[1].zone_id, "zone_right");
        assert_eq!(sampled[1].colors.first().copied(), Some([0, 0, 255]));
    }

    #[test]
    fn multiple_custom_groups_with_display_group_exclude_display_faces_from_led_sampling() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut left = sample_group(4, 4);
        left.name = "Left".into();
        left.effect_id = Some(solid_id);
        left.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        left.layout.zones = vec![point_zone_at("zone_left", 0.25, 0.5)];
        let mut right = sample_group(4, 4);
        right.name = "Right".into();
        right.effect_id = Some(solid_id);
        right.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        right.layout.zones = vec![point_zone_at("zone_right", 0.75, 0.5)];
        let mut display = sample_display_group(4, 4);
        display.name = "Display".into();
        display.effect_id = Some(solid_id);
        display.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
        display.layout.zones = vec![point_zone("zone_display")];
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            &[left, right, display],
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("mixed scene and display groups should render");
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };
        let LedSamplingStrategy::SparkleFlinger(spatial_engine) =
            result.led_sampling_strategy.clone()
        else {
            panic!("multi-group scene renders should sample LEDs from the canonical scene canvas");
        };
        let sampled = spatial_engine.sample(&canvas_from_scene_frame(&result.scene_frame));

        assert_eq!(result.logical_layer_count, 2);
        assert_eq!(
            group_canvas_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
        );
        assert!(zones.is_empty());
        assert_eq!(sampled.len(), 2);
        assert_eq!(sampled[0].zone_id, "zone_left");
        assert_eq!(sampled[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(sampled[1].zone_id, "zone_right");
        assert_eq!(sampled[1].colors.first().copied(), Some([0, 255, 0]));
        let reused = runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .expect("retained multi-group scene should be reusable");
        let LedSamplingStrategy::SparkleFlinger(reused_spatial_engine) =
            reused.led_sampling_strategy
        else {
            panic!("retained multi-group scene should stay scene-canvas owned");
        };
        assert_eq!(reused_spatial_engine.layout().zones.len(), 2);
        assert_eq!(reused_spatial_engine.layout().zones[0].id, "zone_left");
        assert_eq!(reused_spatial_engine.layout().zones[1].id, "zone_right");
    }

    #[test]
    fn multiple_display_groups_publish_surface_backed_direct_canvases() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut left = sample_display_group(4, 4);
        left.name = "Left Display".into();
        left.effect_id = Some(solid_id);
        left.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        left.layout.zones = vec![point_zone("zone_left")];
        let mut right = sample_display_group(4, 4);
        right.name = "Right Display".into();
        right.effect_id = Some(solid_id);
        right.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
        right.layout.zones = vec![point_zone("zone_right")];
        let groups = vec![left.clone(), right.clone()];
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            &groups,
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display groups should render");

        assert!(runtime.target_canvases.is_empty());
        assert_eq!(result.group_canvases.len(), 2);
        assert!(
            result
                .group_canvases
                .iter()
                .all(|(_, frame)| frame.surface.width() > 0 && frame.surface.height() > 0)
        );
        assert!(zones.is_empty());
        let reused = runtime
            .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
            .expect("display-only scene should keep an empty retained LED layout");
        let LedSamplingStrategy::RetainedPreSampled { layout, zones } =
            reused.led_sampling_strategy
        else {
            panic!("display-only scene should keep an empty retained LED layout");
        };
        assert!(layout.zones.is_empty());
        assert!(zones.is_empty());
    }

    #[test]
    fn zero_zone_scene_groups_keep_empty_presampled_led_strategy() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut left = sample_group(2, 2);
        left.name = "Left".into();
        left.effect_id = Some(solid_id);
        left.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        let mut right = sample_group(2, 2);
        right.name = "Right".into();
        right.effect_id = Some(solid_id);
        right.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        let mut zones = Vec::new();

        let result = render_scene_for_test(
            &mut runtime,
            &[left, right],
            1,
            0,
            &HashMap::new(),
            &registry,
            &mut zones,
        )
        .expect("zero-zone scene groups should render");

        let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
            panic!("scene groups without LED zones should keep the empty pre-sampled path");
        };
        assert!(layout.zones.is_empty());
        assert!(zones.is_empty());
    }

    #[test]
    fn retained_scene_invalidates_when_registry_generation_changes() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let mut registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut replacement = builtin_entry(&registry, "rainbow");
        replacement.metadata.id = solid_id;
        let mut group = sample_group(4, 4);
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        let display_group_target_fps = HashMap::new();
        let mut zones = Vec::new();

        let first = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("single group should render");
        let ProducerFrame::Surface(first_surface) = &first.scene_frame else {
            panic!("single group should publish a surface-backed scene frame");
        };

        assert!(
            runtime
                .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
                .is_some(),
            "retained scene should be reusable before the registry changes"
        );

        registry.register(replacement);

        assert!(
            runtime
                .reuse_scene(SceneDependencyKey::new(1, registry.generation()))
                .is_none(),
            "registry generation changes should invalidate retained scene reuse"
        );

        let second = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            1,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("registry generation change should force a rerender");
        let ProducerFrame::Surface(second_surface) = &second.scene_frame else {
            panic!("single group should keep publishing a surface-backed scene frame");
        };

        assert_ne!(
            second_surface.get_pixel(0, 0),
            first_surface.get_pixel(0, 0),
            "same group revision should still rebuild when the registry entry changes"
        );
    }

    #[test]
    fn retained_direct_canvas_invalidates_when_registry_generation_changes() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let mut registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut replacement = builtin_entry(&registry, "rainbow");
        replacement.metadata.id = solid_id;
        let mut group = sample_display_group(4, 4);
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        let display_group_target_fps = HashMap::from([(group.id, 30)]);
        let mut zones = Vec::new();

        let first = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display group should render");
        let [(_, first_frame)] = &first.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };

        registry.register(replacement);

        let second = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            10,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("registry generation change should bypass retained direct-canvas reuse");
        let [(_, second_frame)] = &second.group_canvases[..] else {
            panic!("display group should keep publishing a direct surface");
        };

        assert_ne!(
            second_frame.surface.get_pixel(0, 0),
            first_frame.surface.get_pixel(0, 0),
            "direct canvases should rerender immediately when the active registry entry changes"
        );
        assert!(
            second_frame.surface.storage_identity() != first_frame.surface.storage_identity()
                || second_frame.surface.generation() != first_frame.surface.generation(),
            "the retained direct surface should not be reused across registry generations"
        );
    }

    #[test]
    fn retained_direct_canvas_invalidates_when_groups_revision_changes() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut group = sample_display_group(4, 4);
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        let display_group_target_fps = HashMap::from([(group.id, 30)]);
        let mut zones = Vec::new();

        let first = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display group should render");
        let [(_, first_frame)] = &first.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };

        let second = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            2,
            10,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("group revision change should bypass retained direct-canvas reuse");
        let [(_, second_frame)] = &second.group_canvases[..] else {
            panic!("display group should keep publishing a direct surface");
        };

        assert!(
            second_frame.surface.storage_identity() != first_frame.surface.storage_identity()
                || second_frame.surface.generation() != first_frame.surface.generation(),
            "the retained direct surface should not be reused across group revisions"
        );
    }

    #[test]
    fn zero_zone_display_group_reuses_retained_surface_until_target_interval() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut group = sample_display_group(4, 4);
        group.effect_id = Some(solid_id);
        group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        let display_group_target_fps = HashMap::from([(group.id, 30)]);
        let mut zones = Vec::new();

        let first = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display group should render");
        let [(_, first_frame)] = &first.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };

        let second = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            10,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display group should reuse retained surface");
        let [(_, second_frame)] = &second.group_canvases[..] else {
            panic!("display group should keep publishing a direct surface");
        };

        let third = render_scene_for_test(
            &mut runtime,
            std::slice::from_ref(&group),
            1,
            40,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("display group should rerender once its interval elapses");
        let [(_, third_frame)] = &third.group_canvases[..] else {
            panic!("display group should keep publishing a direct surface");
        };

        assert_eq!(
            first_frame.surface.storage_identity(),
            second_frame.surface.storage_identity()
        );
        assert_eq!(
            first_frame.surface.generation(),
            second_frame.surface.generation()
        );
        assert_eq!(
            first_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 255, 0, 255)
        );
        assert_eq!(
            third_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 255, 0, 255)
        );
        assert!(
            third_frame.surface.storage_identity() != second_frame.surface.storage_identity()
                || third_frame.surface.generation() != second_frame.surface.generation()
        );
    }
}
