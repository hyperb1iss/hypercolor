use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

use hypercolor_core::effect::{EffectPool, EffectRegistry};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
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
    groups: Vec<RenderGroup>,
    preview_frame: ProducerFrame,
    zones: Vec<ZoneColors>,
    layout: Arc<SpatialLayout>,
    logical_layer_count: u32,
}

pub(crate) struct RenderGroupRuntime {
    effect_pool: EffectPool,
    target_canvases: HashMap<RenderGroupId, Canvas>,
    spatial_engines: HashMap<RenderGroupId, SpatialEngine>,
    preview_canvas: Option<Canvas>,
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
            preview_canvas: None,
            retained_frame: None,
            combined_layout: Arc::new(empty_group_layout(preview_width, preview_height)),
            preview_width,
            preview_height,
        }
    }

    pub(crate) fn reuse_scene(&self, groups: &[RenderGroup]) -> Option<RenderGroupResult> {
        let retained = self.retained_frame.as_ref()?;
        if retained.groups != groups {
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
        let preview_frame = ProducerFrame::Canvas(self.compose_preview(groups));
        let layout = Arc::clone(&self.combined_layout);

        self.retained_frame = Some(RetainedRenderGroupFrame {
            groups: groups.to_vec(),
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

    fn compose_preview(&mut self, groups: &[RenderGroup]) -> Canvas {
        let preview_ids = groups
            .iter()
            .filter(|group| group.enabled && group.effect_id.is_some())
            .map(|group| group.id)
            .collect::<Vec<_>>();

        if preview_ids.len() == 1
            && let Some(canvas) = self.target_canvases.get(&preview_ids[0])
        {
            return canvas.clone();
        }

        let mut preview = self
            .preview_canvas
            .take()
            .filter(|canvas| {
                canvas.width() == self.preview_width && canvas.height() == self.preview_height
            })
            .unwrap_or_else(|| Canvas::new(self.preview_width, self.preview_height));
        preview.clear();

        if preview_ids.is_empty() {
            self.preview_canvas = Some(preview.clone());
            return preview;
        }

        let columns = tile_columns(preview_ids.len());
        let rows = preview_ids.len().div_ceil(columns);
        for (index, group_id) in preview_ids.iter().enumerate() {
            let Some(source) = self.target_canvases.get(group_id) else {
                continue;
            };

            let column = index % columns;
            let row = index / columns;
            let x0 = tile_origin(column, columns, self.preview_width);
            let x1 = tile_origin(column + 1, columns, self.preview_width);
            let y0 = tile_origin(row, rows, self.preview_height);
            let y1 = tile_origin(row + 1, rows, self.preview_height);
            blit_scaled_tile(&mut preview, source, x0, y0, x1, y1);
        }

        self.preview_canvas = Some(preview.clone());
        preview
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
