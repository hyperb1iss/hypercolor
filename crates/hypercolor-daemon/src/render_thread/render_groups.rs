use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

use hypercolor_core::effect::{EffectPool, EffectRegistry};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use super::micros_u32;
use super::producer_queue::ProducerFrame;

pub(crate) struct RenderGroupResult {
    pub preview_frame: ProducerFrame,
    pub zones: Vec<ZoneColors>,
    pub layout: Arc<SpatialLayout>,
    pub sample_us: u32,
    pub logical_layer_count: u32,
}

#[derive(Clone)]
struct RetainedRenderGroupFrame {
    groups_revision: u64,
    preview_frame: ProducerFrame,
    zones: Vec<ZoneColors>,
    layout: Arc<SpatialLayout>,
    logical_layer_count: u32,
}

pub(crate) struct RenderGroupRuntime {
    effect_pool: EffectPool,
    target_canvases: HashMap<RenderGroupId, Canvas>,
    spatial_engines: HashMap<RenderGroupId, SpatialEngine>,
    preview_surface_pool: RenderSurfacePool,
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
            zones: retained.zones.clone(),
            layout: Arc::clone(&retained.layout),
            sample_us: 0,
            logical_layer_count: retained.logical_layer_count,
        })
    }

    pub(crate) fn render_scene(
        &mut self,
        groups: &[RenderGroup],
        groups_revision: u64,
        registry: &EffectRegistry,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
    ) -> Result<RenderGroupResult> {
        self.reconcile(groups, registry)?;

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
                target,
            )?;
        }

        let sample_start = Instant::now();
        let mut zones = Vec::with_capacity(
            groups
                .iter()
                .filter(|group| group.enabled && group.effect_id.is_some())
                .map(|group| group.layout.zones.len())
                .sum(),
        );
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
            zones.extend(spatial_engine.sample(target));
        }
        let sample_us = micros_u32(sample_start.elapsed());
        let logical_layer_count = u32::try_from(
            groups
                .iter()
                .filter(|group| group.enabled && group.effect_id.is_some())
                .count(),
        )
        .unwrap_or(u32::MAX);
        let preview_frame = self.compose_preview(groups);
        let layout = Arc::clone(&self.combined_layout);

        self.retained_frame = Some(RetainedRenderGroupFrame {
            groups_revision,
            preview_frame: preview_frame.clone(),
            zones: zones.clone(),
            layout: Arc::clone(&layout),
            logical_layer_count,
        });

        Ok(RenderGroupResult {
            preview_frame,
            zones,
            layout,
            sample_us,
            logical_layer_count,
        })
    }

    fn reconcile(&mut self, groups: &[RenderGroup], registry: &EffectRegistry) -> Result<()> {
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

    fn compose_preview(&mut self, groups: &[RenderGroup]) -> ProducerFrame {
        let preview_ids = groups
            .iter()
            .filter(|group| group.enabled && group.effect_id.is_some())
            .map(|group| group.id)
            .collect::<Vec<_>>();

        let Some(mut lease) = self.preview_surface_pool.dequeue() else {
            let mut preview = Canvas::new(self.preview_width, self.preview_height);
            compose_preview_canvas(
                &mut preview,
                &preview_ids,
                &self.target_canvases,
                self.preview_width,
                self.preview_height,
            );
            return ProducerFrame::Canvas(preview);
        };

        compose_preview_canvas(
            lease.canvas_mut(),
            &preview_ids,
            &self.target_canvases,
            self.preview_width,
            self.preview_height,
        );

        ProducerFrame::Surface(lease.submit(0, 0))
    }
}

fn compose_preview_canvas(
    preview: &mut Canvas,
    preview_ids: &[RenderGroupId],
    target_canvases: &HashMap<RenderGroupId, Canvas>,
    preview_width: u32,
    preview_height: u32,
) {
    preview.clear();

    if preview_ids.is_empty() {
        return;
    }

    if preview_ids.len() == 1
        && let Some(source) = target_canvases.get(&preview_ids[0])
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

    let columns = tile_columns(preview_ids.len());
    let rows = preview_ids.len().div_ceil(columns);
    for (index, group_id) in preview_ids.iter().enumerate() {
        let Some(source) = target_canvases.get(group_id) else {
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
    let side = (count as f32).sqrt().ceil() as usize;
    side.max(1)
}

fn tile_origin(index: usize, count: usize, extent: u32) -> u32 {
    let numerator = u64::try_from(index)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(extent));
    let denominator = u64::try_from(count.max(1)).unwrap_or(1);
    u32::try_from(numerator / denominator).unwrap_or(extent)
}

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

    use hypercolor_types::canvas::Rgba;
    use hypercolor_types::effect::EffectId;
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
        }
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
}
