use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

use hypercolor_core::effect::{EffectPool, EffectRegistry};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, PublishedSurface, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{DisplayFaceTarget, RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use super::micros_u32;
use super::producer_queue::ProducerFrame;

/// Slot count for the full-resolution preview surface pool. Sized to absorb
/// typical downstream pins: the canvas watch channel, display-output
/// dispatch, and one in-flight JPEG encode per HTML-face worker. Undersizing
/// forces `begin_dequeue` to reallocate a fresh canvas every frame whenever
/// all slots are still shared downstream, which shows up as producer-stage
/// stalls proportional to `canvas_width * canvas_height * 4` bytes.
const PREVIEW_SURFACE_POOL_SLOTS: usize = 6;

/// Slot count for per-group direct-canvas pools (HTML-face render groups).
/// Same failure mode as the preview pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_SLOTS: usize = 4;

#[derive(Clone)]
pub(crate) struct GroupCanvasFrame {
    pub surface: PublishedSurface,
    pub display_target: DisplayFaceTarget,
}

#[derive(Clone)]
pub(crate) enum LedSamplingStrategy {
    SparkleFlinger(SpatialEngine),
    PreSampled(Arc<SpatialLayout>),
    RetainedPreSampled {
        layout: Arc<SpatialLayout>,
        zones: Arc<[ZoneColors]>,
    },
    ReusePublished(Arc<SpatialLayout>),
}

pub(crate) struct RenderGroupResult {
    pub ui_preview_frame: ProducerFrame,
    pub group_canvases: Vec<(RenderGroupId, GroupCanvasFrame)>,
    pub active_group_canvas_ids: Vec<RenderGroupId>,
    pub led_sampling_strategy: LedSamplingStrategy,
    pub render_us: u32,
    pub sample_us: u32,
    pub preview_compose_us: u32,
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
    groups_revision: u64,
    ui_preview_frame: ProducerFrame,
    active_group_canvas_ids: Vec<RenderGroupId>,
    led_sampling_strategy: RetainedLedSamplingStrategy,
    logical_layer_count: u32,
}

#[derive(Clone)]
enum RetainedLedSamplingStrategy {
    SparkleFlinger(SpatialEngine),
    PreSampled {
        layout: Arc<SpatialLayout>,
        zones: Arc<[ZoneColors]>,
    },
}

#[derive(Clone)]
struct RetainedDirectGroupFrame {
    frame: GroupCanvasFrame,
    rendered_at_ms: u32,
}

pub(crate) struct RenderGroupRuntime {
    effect_pool: EffectPool,
    target_canvases: HashMap<RenderGroupId, Canvas>,
    spatial_engines: HashMap<RenderGroupId, SpatialEngine>,
    direct_surface_pools: HashMap<RenderGroupId, RenderSurfacePool>,
    retained_direct_group_frames: HashMap<RenderGroupId, RetainedDirectGroupFrame>,
    preview_surface_pool: RenderSurfacePool,
    reconciled_groups_revision: Option<u64>,
    retained_frame: Option<RetainedRenderGroupFrame>,
    last_effect_error: Option<RenderGroupEffectError>,
    combined_led_layout: Arc<SpatialLayout>,
    preview_width: u32,
    preview_height: u32,
}

impl RenderGroupRuntime {
    pub(crate) fn new(preview_width: u32, preview_height: u32) -> Self {
        Self {
            effect_pool: EffectPool::new(),
            target_canvases: HashMap::new(),
            spatial_engines: HashMap::new(),
            direct_surface_pools: HashMap::new(),
            retained_direct_group_frames: HashMap::new(),
            // 6 slots absorbs typical downstream fan-out (watch channel +
            // display-output dispatch + one pin per display worker mid-
            // encode). 3 is enough when the UI isn't running but starves
            // as soon as HTML-face devices JPEG-encode in parallel,
            // causing every dequeue to realloc a fresh full-res canvas.
            preview_surface_pool: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(preview_width, preview_height),
                PREVIEW_SURFACE_POOL_SLOTS,
            ),
            reconciled_groups_revision: None,
            retained_frame: None,
            last_effect_error: None,
            combined_led_layout: Arc::new(empty_group_layout(preview_width, preview_height)),
            preview_width,
            preview_height,
        }
    }

    /// Total count of times `preview_surface_pool.dequeue()` had to reuse
    /// a still-shared Published slot (and therefore allocate a fresh
    /// canvas). Monotonically increasing; non-zero growth means the pool
    /// is undersized for current downstream fan-out.
    #[must_use]
    pub(crate) fn preview_surface_pool_saturation_reallocs(&self) -> u64 {
        self.preview_surface_pool.saturation_reallocs()
    }

    /// Same as `preview_surface_pool_saturation_reallocs` but summed across
    /// every direct-canvas group pool (one per HTML-face render group).
    #[must_use]
    pub(crate) fn direct_surface_pool_saturation_reallocs(&self) -> u64 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::saturation_reallocs)
            .sum()
    }

    /// Count of slots the preview pool has appended above its initial
    /// capacity since construction. Non-zero values are benign and
    /// reflect the pool settling at its working-set size.
    #[must_use]
    pub(crate) fn preview_surface_pool_grown_slots(&self) -> u32 {
        self.preview_surface_pool.grown_slots()
    }

    /// Total grown slots across every direct-canvas group pool.
    #[must_use]
    pub(crate) fn direct_surface_pool_grown_slots(&self) -> u32 {
        self.direct_surface_pools
            .values()
            .map(RenderSurfacePool::grown_slots)
            .sum()
    }

    pub(crate) fn reuse_scene(&self, groups_revision: u64) -> Option<RenderGroupResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.groups_revision != groups_revision {
            return None;
        }

        Some(RenderGroupResult {
            ui_preview_frame: retained.ui_preview_frame.clone(),
            group_canvases: Vec::new(),
            active_group_canvas_ids: retained.active_group_canvas_ids.clone(),
            led_sampling_strategy: match &retained.led_sampling_strategy {
                RetainedLedSamplingStrategy::SparkleFlinger(spatial_engine) => {
                    LedSamplingStrategy::SparkleFlinger(spatial_engine.clone())
                }
                RetainedLedSamplingStrategy::PreSampled { layout, zones } => {
                    LedSamplingStrategy::RetainedPreSampled {
                        layout: Arc::clone(layout),
                        zones: Arc::clone(zones),
                    }
                }
            },
            render_us: 0,
            sample_us: 0,
            preview_compose_us: 0,
            logical_layer_count: retained.logical_layer_count,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "render-scene orchestration needs the full frame context plus reusable zone storage"
    )]
    pub(crate) fn render_scene(
        &mut self,
        groups: &[RenderGroup],
        groups_revision: u64,
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
        self.reconcile(groups, groups_revision, registry)?;

        if let Some(result) = self.render_single_full_preview_group(
            groups,
            elapsed_ms,
            display_group_target_fps,
            registry,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            zones,
        )? {
            self.clear_effect_error();
            self.retain_frame(groups_revision, &result, &[]);
            return Ok(result);
        }

        let mut next_index = 0;
        let mut render_us = 0_u32;
        let mut sample_us = 0_u32;
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
                self.retain_direct_group_frame(group.id, elapsed_ms, &frame);
                group_canvases.push((group.id, frame));
                continue;
            }

            let Some(spatial_engine) = self.spatial_engines.get(&group.id) else {
                continue;
            };
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
            if !group.layout.zones.is_empty() {
                let sample_start = Instant::now();
                next_index = spatial_engine.sample_append_into_at(target, zones, next_index);
                sample_us = sample_us.saturating_add(micros_u32(sample_start.elapsed()));
            }
        }
        zones.truncate(next_index);
        let logical_layer_count = u32::try_from(
            groups
                .iter()
                .filter(|group| group_contributes_to_preview(group))
                .count(),
        )
        .unwrap_or(u32::MAX);
        let preview_compose_start = Instant::now();
        let ui_preview_frame = self.compose_ui_preview(groups);
        let preview_compose_us = micros_u32(preview_compose_start.elapsed());

        let result = RenderGroupResult {
            ui_preview_frame,
            group_canvases,
            active_group_canvas_ids,
            led_sampling_strategy: LedSamplingStrategy::PreSampled(Arc::clone(
                &self.combined_led_layout,
            )),
            render_us,
            sample_us,
            preview_compose_us,
            logical_layer_count,
        };
        self.clear_effect_error();
        self.retain_frame(groups_revision, &result, zones);
        Ok(result)
    }

    fn reconcile(
        &mut self,
        groups: &[RenderGroup],
        groups_revision: u64,
        registry: &EffectRegistry,
    ) -> Result<()> {
        if self.reconciled_groups_revision == Some(groups_revision) {
            return Ok(());
        }

        self.effect_pool.reconcile(groups, registry)?;

        let desired_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
        let preview_group_ids = groups
            .iter()
            .filter(|group| group_contributes_to_preview(group))
            .map(|group| group.id)
            .collect::<HashSet<_>>();
        let direct_group_ids = groups
            .iter()
            .filter(|group| group_publishes_direct_canvas(group))
            .map(|group| group.id)
            .collect::<HashSet<_>>();
        self.target_canvases
            .retain(|group_id, _| preview_group_ids.contains(group_id));
        self.spatial_engines
            .retain(|group_id, _| desired_ids.contains(group_id));
        self.direct_surface_pools
            .retain(|group_id, _| direct_group_ids.contains(group_id));
        self.retained_direct_group_frames
            .retain(|group_id, _| direct_group_ids.contains(group_id));

        for group in groups {
            if group_contributes_to_preview(group) {
                self.ensure_group_canvas(group);
            }
            if group_publishes_direct_canvas(group) {
                self.ensure_direct_surface_pool(group);
            }
            self.ensure_spatial_engine(group);
        }

        self.combined_led_layout = Arc::new(combine_led_group_layouts(
            groups,
            self.preview_width,
            self.preview_height,
        ));
        self.reconciled_groups_revision = Some(groups_revision);

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
                RenderSurfacePool::with_slot_count(descriptor, DIRECT_SURFACE_POOL_SLOTS),
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

    fn render_single_full_preview_group(
        &mut self,
        groups: &[RenderGroup],
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
        registry: &EffectRegistry,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<Option<RenderGroupResult>> {
        let Some(preview_group) = self.single_full_preview_group(groups) else {
            return Ok(None);
        };
        let Some(spatial_engine) = self.spatial_engines.get(&preview_group.id).cloned() else {
            return Ok(None);
        };
        let Some(mut lease) = self.preview_surface_pool.dequeue() else {
            return Ok(None);
        };

        let render_start = Instant::now();
        if let Err(error) = self.effect_pool.render_group_into(
            preview_group,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            lease.canvas_mut(),
        ) {
            lease.release();
            return Err(anyhow::Error::new(render_group_effect_error(
                preview_group,
                registry,
                error,
            )));
        }
        let mut render_us = micros_u32(render_start.elapsed());

        let sample_us = 0_u32;
        if !preview_group.layout.zones.is_empty() {
            zones.clear();
        }
        let preview_surface = lease.submit(0, 0);

        let mut group_canvases = Vec::new();
        let mut active_group_canvas_ids = Vec::new();
        for group in groups {
            if !group.enabled || group.effect_id.is_none() || group.id == preview_group.id {
                continue;
            }
            if !group_publishes_direct_canvas(group) {
                continue;
            }

            if let Some(retained) =
                self.reuse_retained_direct_group_frame(group, elapsed_ms, display_group_target_fps)
            {
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
            self.retain_direct_group_frame(group.id, elapsed_ms, &frame);
            group_canvases.push((group.id, frame));
        }
        zones.clear();

        Ok(Some(RenderGroupResult {
            ui_preview_frame: ProducerFrame::Surface(preview_surface),
            group_canvases,
            active_group_canvas_ids,
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(spatial_engine),
            render_us,
            sample_us,
            preview_compose_us: 0,
            logical_layer_count: 1,
        }))
    }

    pub(crate) fn note_effect_error(
        &mut self,
        error: &RenderGroupEffectError,
    ) -> Option<RenderGroupEffectError> {
        if self.last_effect_error.as_ref() == Some(error) {
            return None;
        }

        self.last_effect_error = Some(error.clone());
        Some(error.clone())
    }

    fn clear_effect_error(&mut self) {
        self.last_effect_error = None;
    }

    fn single_full_preview_group<'a>(&self, groups: &'a [RenderGroup]) -> Option<&'a RenderGroup> {
        let mut preview_groups = groups
            .iter()
            .filter(|group| group_contributes_to_preview(group));
        let group = preview_groups.next()?;
        if preview_groups.next().is_some() {
            return None;
        }
        if group.layout.canvas_width != self.preview_width
            || group.layout.canvas_height != self.preview_height
        {
            return None;
        }
        Some(group)
    }

    fn retain_frame(
        &mut self,
        groups_revision: u64,
        result: &RenderGroupResult,
        zones: &[ZoneColors],
    ) {
        self.retained_frame = Some(RetainedRenderGroupFrame {
            groups_revision,
            ui_preview_frame: result.ui_preview_frame.clone(),
            active_group_canvas_ids: result.active_group_canvas_ids.clone(),
            led_sampling_strategy: match &result.led_sampling_strategy {
                LedSamplingStrategy::PreSampled(layout) => RetainedLedSamplingStrategy::PreSampled {
                    layout: Arc::clone(layout),
                    zones: zones.to_vec().into(),
                },
                LedSamplingStrategy::SparkleFlinger(spatial_engine) => {
                    RetainedLedSamplingStrategy::SparkleFlinger(spatial_engine.clone())
                }
                LedSamplingStrategy::RetainedPreSampled { layout, zones } => {
                    RetainedLedSamplingStrategy::PreSampled {
                        layout: Arc::clone(layout),
                        zones: Arc::clone(zones),
                    }
                }
                LedSamplingStrategy::ReusePublished(layout) => {
                    RetainedLedSamplingStrategy::PreSampled {
                        layout: Arc::clone(layout),
                        zones: Arc::new([]),
                    }
                }
            },
            logical_layer_count: result.logical_layer_count,
        });
    }

    fn reuse_retained_direct_group_frame(
        &self,
        group: &RenderGroup,
        elapsed_ms: u32,
        display_group_target_fps: &HashMap<RenderGroupId, u32>,
    ) -> Option<GroupCanvasFrame> {
        if !group_publishes_direct_canvas(group) || !group.layout.zones.is_empty() {
            return None;
        }

        let target_fps = *display_group_target_fps.get(&group.id)?;
        let retained = self.retained_direct_group_frames.get(&group.id)?;
        let frame_interval_ms = 1000_u32.div_ceil(target_fps.max(1));
        (elapsed_ms.saturating_sub(retained.rendered_at_ms) < frame_interval_ms)
            .then(|| retained.frame.clone())
    }

    fn retain_direct_group_frame(
        &mut self,
        group_id: RenderGroupId,
        elapsed_ms: u32,
        frame: &GroupCanvasFrame,
    ) {
        self.retained_direct_group_frames.insert(
            group_id,
            RetainedDirectGroupFrame {
                frame: frame.clone(),
                rendered_at_ms: elapsed_ms,
            },
        );
    }

    fn compose_ui_preview(&mut self, groups: &[RenderGroup]) -> ProducerFrame {
        let Some(mut lease) = self.preview_surface_pool.dequeue() else {
            let mut preview = Canvas::new(self.preview_width, self.preview_height);
            compose_ui_preview_canvas(
                &mut preview,
                groups,
                &self.target_canvases,
                self.preview_width,
                self.preview_height,
            );
            return ProducerFrame::Canvas(preview);
        };

        compose_ui_preview_canvas(
            lease.canvas_mut(),
            groups,
            &self.target_canvases,
            self.preview_width,
            self.preview_height,
        );

        ProducerFrame::Surface(lease.submit(0, 0))
    }
}

fn compose_ui_preview_canvas(
    preview: &mut Canvas,
    groups: &[RenderGroup],
    target_canvases: &HashMap<RenderGroupId, Canvas>,
    preview_width: u32,
    preview_height: u32,
) {
    let preview_count = groups
        .iter()
        .filter(|group| group_contributes_to_preview(group))
        .count();

    if preview_count == 0 {
        preview.clear();
        return;
    }

    if preview_count == 1
        && let Some(source) = groups
            .iter()
            .find(|group| group_contributes_to_preview(group))
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
        .filter(|group| group_contributes_to_preview(group))
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

fn group_contributes_to_preview(group: &RenderGroup) -> bool {
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
        .filter(|group| group_contributes_to_preview(group))
        .flat_map(|group| group.layout.zones.clone())
        .collect();
    layout
}

fn tile_columns(count: usize) -> usize {
    let mut side = 1_usize;
    while side.saturating_mul(side) < count.max(1) {
        side = side.saturating_add(1);
    }
    side.max(1)
}

fn tile_origin(index: usize, count: usize, extent: u32) -> u32 {
    let numerator = u64::try_from(index)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(extent));
    let denominator = u64::try_from(count.max(1)).unwrap_or(1);
    u32::try_from(numerator / denominator).unwrap_or(extent)
}

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
        }
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
            groups_revision,
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
    fn single_group_preview_publishes_surface_frame() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let group = sample_group(4, 4);
        let mut source = Canvas::new(4, 4);
        source.fill(Rgba::new(12, 34, 56, 255));
        runtime.target_canvases.insert(group.id, source);

        let preview = runtime.compose_ui_preview(&[group]);
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

        let preview = runtime.compose_ui_preview(&[group]);
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

        let preview = runtime.compose_ui_preview(&[preview_group, display_group]);
        let ProducerFrame::Surface(surface) = preview else {
            panic!("mixed preview should publish a pooled surface");
        };

        assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(surface.get_pixel(3, 3), Rgba::new(255, 0, 0, 255));
    }

    #[test]
    fn single_full_preview_group_renders_directly_into_surface() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
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

        let ProducerFrame::Surface(surface) = &result.ui_preview_frame else {
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
    fn single_full_preview_display_group_keeps_shared_preview_blank() {
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

        let ProducerFrame::Surface(preview_surface) = result.ui_preview_frame else {
            panic!("single display group should render into a surface");
        };
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a surface-backed direct canvas");
        };

        assert_eq!(result.logical_layer_count, 0);
        assert_eq!(preview_surface.get_pixel(0, 0), Rgba::new(0, 0, 0, 255));
        assert_eq!(
            group_canvas_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
        );
        assert!(zones.is_empty());
    }

    #[test]
    fn full_preview_group_with_display_group_keeps_display_faces_out_of_led_sampling() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let mut preview_group = sample_group(4, 4);
        preview_group.effect_id = Some(solid_id);
        preview_group.controls =
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]);
        preview_group.layout.zones = vec![point_zone("zone_preview")];
        let mut display_group = sample_display_group(4, 4);
        display_group.effect_id = Some(solid_id);
        display_group.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]);
        display_group.layout.zones = vec![point_zone("zone_display")];
        let mut zones = Vec::new();
        let display_group_target_fps = HashMap::new();

        let result = render_scene_for_test(
            &mut runtime,
            &[preview_group.clone(), display_group.clone()],
            1,
            0,
            &display_group_target_fps,
            &registry,
            &mut zones,
        )
        .expect("mixed preview and display groups should render");

        let ProducerFrame::Surface(preview_surface) = &result.ui_preview_frame else {
            panic!("mixed full-preview scene should publish a surface-backed preview");
        };
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };
        let LedSamplingStrategy::SparkleFlinger(spatial_engine) =
            result.led_sampling_strategy.clone()
        else {
            panic!("single preview scene should hand LED sampling to SparkleFlinger");
        };
        let sampled = spatial_engine.sample(&Canvas::from_rgba(
            preview_surface.rgba_bytes(),
            preview_surface.width(),
            preview_surface.height(),
        ));
        let reused = runtime
            .reuse_scene(1)
            .expect("retained scene should be reusable");
        let LedSamplingStrategy::SparkleFlinger(reused_spatial_engine) =
            reused.led_sampling_strategy
        else {
            panic!("retained single-preview scene should stay SparkleFlinger-owned");
        };

        assert_eq!(preview_surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
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
                    zones: vec![point_zone("zone_left")],
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
                    zones: vec![point_zone("zone_right")],
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

        assert_eq!(result.logical_layer_count, 2);
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].zone_id, "zone_left");
        assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(zones[1].zone_id, "zone_right");
        assert_eq!(zones[1].colors.first().copied(), Some([0, 0, 255]));
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
        left.layout.zones = vec![point_zone("zone_left")];
        let mut right = sample_group(4, 4);
        right.name = "Right".into();
        right.effect_id = Some(solid_id);
        right.controls =
            HashMap::from([("color".into(), ControlValue::Color([0.0, 1.0, 0.0, 1.0]))]);
        right.layout.zones = vec![point_zone("zone_right")];
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
        .expect("mixed preview and display groups should render");
        let [(_, group_canvas_frame)] = &result.group_canvases[..] else {
            panic!("display group should publish a direct surface");
        };
        let LedSamplingStrategy::PreSampled(layout) = result.led_sampling_strategy else {
            panic!("generic multi-group path should keep using pre-sampled LED zones");
        };

        assert_eq!(result.logical_layer_count, 2);
        assert_eq!(
            group_canvas_frame.surface.get_pixel(0, 0),
            Rgba::new(0, 0, 255, 255)
        );
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].zone_id, "zone_left");
        assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(zones[1].zone_id, "zone_right");
        assert_eq!(zones[1].colors.first().copied(), Some([0, 255, 0]));
        assert_eq!(layout.zones.len(), 2);
        assert_eq!(layout.zones[0].id, "zone_left");
        assert_eq!(layout.zones[1].id, "zone_right");
        let reused = runtime
            .reuse_scene(1)
            .expect("retained multi-group scene should be reusable");
        let LedSamplingStrategy::RetainedPreSampled { layout, zones } = reused.led_sampling_strategy
        else {
            panic!("retained multi-group scene should keep its raw pre-sampled zones");
        };
        assert_eq!(layout.zones.len(), 2);
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].zone_id, "zone_left");
        assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
        assert_eq!(zones[1].zone_id, "zone_right");
        assert_eq!(zones[1].colors.first().copied(), Some([0, 255, 0]));
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
            .reuse_scene(1)
            .expect("display-only scene should keep an empty retained LED layout");
        let LedSamplingStrategy::RetainedPreSampled { layout, zones } = reused.led_sampling_strategy else {
            panic!("display-only scene should keep an empty retained LED layout");
        };
        assert!(layout.zones.is_empty());
        assert!(zones.is_empty());
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
