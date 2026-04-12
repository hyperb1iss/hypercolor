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
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use super::micros_u32;
use super::producer_queue::ProducerFrame;

#[derive(Clone)]
pub(crate) enum GroupCanvasFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
}

pub(crate) struct RenderGroupResult {
    pub preview_frame: ProducerFrame,
    pub group_canvases: Vec<(RenderGroupId, GroupCanvasFrame)>,
    pub active_group_canvas_ids: Vec<RenderGroupId>,
    pub layout: Arc<SpatialLayout>,
    pub sample_us: u32,
    pub logical_layer_count: u32,
    pub reuse_published_zones: bool,
}

#[derive(Clone)]
struct RetainedRenderGroupFrame {
    groups_revision: u64,
    preview_frame: ProducerFrame,
    active_group_canvas_ids: Vec<RenderGroupId>,
    layout: Arc<SpatialLayout>,
    logical_layer_count: u32,
}

pub(crate) struct RenderGroupRuntime {
    effect_pool: EffectPool,
    target_canvases: HashMap<RenderGroupId, Canvas>,
    spatial_engines: HashMap<RenderGroupId, SpatialEngine>,
    preview_surface_pool: RenderSurfacePool,
    reconciled_groups_revision: Option<u64>,
    retained_frame: Option<RetainedRenderGroupFrame>,
    combined_layout: Arc<SpatialLayout>,
    preview_width: u32,
    preview_height: u32,
}

impl RenderGroupRuntime {
    pub(crate) fn new(preview_width: u32, preview_height: u32) -> Self {
        Self {
            effect_pool: EffectPool::new(),
            target_canvases: HashMap::new(),
            spatial_engines: HashMap::new(),
            preview_surface_pool: RenderSurfacePool::new(SurfaceDescriptor::rgba8888(
                preview_width,
                preview_height,
            )),
            reconciled_groups_revision: None,
            retained_frame: None,
            combined_layout: Arc::new(empty_group_layout(preview_width, preview_height)),
            preview_width,
            preview_height,
        }
    }

    pub(crate) fn reuse_scene(&self, groups_revision: u64) -> Option<RenderGroupResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.groups_revision != groups_revision {
            return None;
        }

        Some(RenderGroupResult {
            preview_frame: retained.preview_frame.clone(),
            group_canvases: Vec::new(),
            active_group_canvas_ids: retained.active_group_canvas_ids.clone(),
            layout: Arc::clone(&retained.layout),
            sample_us: 0,
            logical_layer_count: retained.logical_layer_count,
            reuse_published_zones: true,
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
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            zones,
        )? {
            self.retain_frame(groups_revision, &result);
            return Ok(result);
        }

        for group in groups {
            let Some(target) = self.target_canvases.get_mut(&group.id) else {
                continue;
            };
            self.effect_pool.render_group_into(
                group,
                delta_secs,
                audio,
                interaction,
                screen,
                sensors,
                target,
            )?;
        }

        let sample_start = Instant::now();
        let mut next_index = 0;
        for group in groups {
            if !group.enabled || group.effect_id.is_none() || group.layout.zones.is_empty() {
                continue;
            }

            let Some(target) = self.target_canvases.get(&group.id) else {
                continue;
            };
            let Some(spatial_engine) = self.spatial_engines.get(&group.id) else {
                continue;
            };
            next_index = spatial_engine.sample_append_into_at(target, zones, next_index);
        }
        zones.truncate(next_index);
        let sample_us = micros_u32(sample_start.elapsed());
        let logical_layer_count = u32::try_from(
            groups
                .iter()
                .filter(|group| group.enabled && group.effect_id.is_some())
                .count(),
        )
        .unwrap_or(u32::MAX);
        let preview_frame = self.compose_preview(groups);
        let active_group_canvas_ids = groups
            .iter()
            .filter(|group| {
                group.enabled && group.effect_id.is_some() && group.display_target.is_some()
            })
            .map(|group| group.id)
            .collect::<Vec<_>>();
        let group_canvases = active_group_canvas_ids
            .iter()
            .filter_map(|group| {
                self.target_canvases
                    .get(group)
                    .map(|canvas| (*group, GroupCanvasFrame::Canvas(canvas.clone())))
            })
            .collect();
        let layout = Arc::clone(&self.combined_layout);

        let result = RenderGroupResult {
            preview_frame,
            group_canvases,
            active_group_canvas_ids,
            layout,
            sample_us,
            logical_layer_count,
            reuse_published_zones: false,
        };
        self.retain_frame(groups_revision, &result);
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
        self.target_canvases
            .retain(|group_id, _| desired_ids.contains(group_id));
        self.spatial_engines
            .retain(|group_id, _| desired_ids.contains(group_id));

        for group in groups {
            self.ensure_group_canvas(group);
            self.ensure_spatial_engine(group);
        }

        self.combined_layout = Arc::new(combine_group_layouts(
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
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<Option<RenderGroupResult>> {
        let Some(group) = self.single_full_preview_group(groups) else {
            return Ok(None);
        };
        let Some(spatial_engine) = self.spatial_engines.get(&group.id) else {
            return Ok(None);
        };
        let Some(mut lease) = self.preview_surface_pool.dequeue() else {
            return Ok(None);
        };

        if let Err(error) = self.effect_pool.render_group_into(
            group,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            lease.canvas_mut(),
        ) {
            lease.release();
            return Err(error);
        }

        let sample_start = Instant::now();
        let next_index = spatial_engine.sample_append_into_at(lease.canvas_mut(), zones, 0);
        zones.truncate(next_index);
        let sample_us = micros_u32(sample_start.elapsed());
        let preview_surface = lease.submit(0, 0);
        let active_group_canvas_ids = group
            .display_target
            .as_ref()
            .map_or_else(Vec::new, |_| vec![group.id]);
        let group_canvases = group.display_target.as_ref().map_or_else(Vec::new, |_| {
            vec![(group.id, GroupCanvasFrame::Surface(preview_surface.clone()))]
        });

        Ok(Some(RenderGroupResult {
            preview_frame: ProducerFrame::Surface(preview_surface),
            group_canvases,
            active_group_canvas_ids,
            layout: Arc::clone(&self.combined_layout),
            sample_us,
            logical_layer_count: 1,
            reuse_published_zones: false,
        }))
    }

    fn single_full_preview_group<'a>(&self, groups: &'a [RenderGroup]) -> Option<&'a RenderGroup> {
        let mut active_groups = groups
            .iter()
            .filter(|group| group.enabled && group.effect_id.is_some());
        let group = active_groups.next()?;
        if active_groups.next().is_some() {
            return None;
        }
        if group.layout.canvas_width != self.preview_width
            || group.layout.canvas_height != self.preview_height
        {
            return None;
        }
        Some(group)
    }

    fn retain_frame(&mut self, groups_revision: u64, result: &RenderGroupResult) {
        self.retained_frame = Some(RetainedRenderGroupFrame {
            groups_revision,
            preview_frame: result.preview_frame.clone(),
            active_group_canvas_ids: result.active_group_canvas_ids.clone(),
            layout: Arc::clone(&result.layout),
            logical_layer_count: result.logical_layer_count,
        });
    }

    fn compose_preview(&mut self, groups: &[RenderGroup]) -> ProducerFrame {
        let Some(mut lease) = self.preview_surface_pool.dequeue() else {
            let mut preview = Canvas::new(self.preview_width, self.preview_height);
            compose_preview_canvas(
                &mut preview,
                groups,
                &self.target_canvases,
                self.preview_width,
                self.preview_height,
            );
            return ProducerFrame::Canvas(preview);
        };

        compose_preview_canvas(
            lease.canvas_mut(),
            groups,
            &self.target_canvases,
            self.preview_width,
            self.preview_height,
        );

        ProducerFrame::Surface(lease.submit(0, 0))
    }
}

fn compose_preview_canvas(
    preview: &mut Canvas,
    groups: &[RenderGroup],
    target_canvases: &HashMap<RenderGroupId, Canvas>,
    preview_width: u32,
    preview_height: u32,
) {
    let preview_count = groups
        .iter()
        .filter(|group| group.enabled && group.effect_id.is_some())
        .count();

    if preview_count == 0 {
        preview.clear();
        return;
    }

    if preview_count == 1
        && let Some(source) = groups
            .iter()
            .find(|group| group.enabled && group.effect_id.is_some())
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
        .filter(|group| group.enabled && group.effect_id.is_some())
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

fn combine_group_layouts(groups: &[RenderGroup], width: u32, height: u32) -> SpatialLayout {
    let mut layout = empty_group_layout(width, height);
    layout.zones = groups
        .iter()
        .filter(|group| group.enabled && group.effect_id.is_some())
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
        }
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

    #[test]
    fn single_group_preview_publishes_surface_frame() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let group = sample_group(4, 4);
        let mut source = Canvas::new(4, 4);
        source.fill(Rgba::new(12, 34, 56, 255));
        runtime.target_canvases.insert(group.id, source);

        let preview = runtime.compose_preview(&[group]);
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

        let preview = runtime.compose_preview(&[group]);
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
        };
        let mut zones = Vec::new();

        let result = runtime
            .render_scene(
                std::slice::from_ref(&group),
                1,
                &registry,
                1.0 / 60.0,
                &AudioData::silence(),
                &InteractionData::default(),
                None,
                &SystemSnapshot::empty(),
                &mut zones,
            )
            .expect("single group should render");

        let ProducerFrame::Surface(surface) = result.preview_frame else {
            panic!("single full-size group should render into a surface");
        };

        assert_eq!(surface.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].colors.first().copied(), Some([255, 0, 0]));
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
    fn single_full_preview_display_group_reuses_surface_for_direct_canvas() {
        let mut runtime = RenderGroupRuntime::new(4, 4);
        let registry = builtin_registry();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let group = RenderGroup {
            id: RenderGroupId::new(),
            name: "Display".into(),
            description: None,
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
            preset_id: None,
            layout: SpatialLayout {
                id: "display-group".into(),
                name: "Display Group".into(),
                description: None,
                canvas_width: 4,
                canvas_height: 4,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(hypercolor_types::scene::DisplayFaceTarget {
                device_id: hypercolor_types::device::DeviceId::new(),
            }),
        };
        let mut zones = Vec::new();

        let result = runtime
            .render_scene(
                std::slice::from_ref(&group),
                1,
                &registry,
                1.0 / 60.0,
                &AudioData::silence(),
                &InteractionData::default(),
                None,
                &SystemSnapshot::empty(),
                &mut zones,
            )
            .expect("single display group should render");

        let ProducerFrame::Surface(preview_surface) = result.preview_frame else {
            panic!("single display group should render into a surface");
        };
        let [(_, GroupCanvasFrame::Surface(group_surface))] = &result.group_canvases[..] else {
            panic!("display group should publish a surface-backed direct canvas");
        };

        assert_eq!(
            preview_surface.storage_identity(),
            group_surface.storage_identity()
        );
        assert_eq!(preview_surface.generation(), group_surface.generation());
    }
}
