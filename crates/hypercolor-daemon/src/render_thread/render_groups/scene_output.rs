#[cfg(test)]
use std::collections::HashMap;

use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::Zone;
#[cfg(test)]
use hypercolor_types::scene::ZoneId;

use super::super::producer_queue::{ProducerFrame, record_producer_frame};
use super::super::sparkleflinger::{CompositionLayer, CompositionPlan, SparkleFlinger};
use super::ZoneRuntime;
use super::frame_helpers::composed_frame_to_producer_frame;
use super::group_state::group_contributes_to_scene_canvas;
use super::projection::compose_authoritative_scene_canvas;

impl ZoneRuntime {
    pub(super) fn sample_scene_group_led_zones(
        &self,
        groups: &[Zone],
        zones: &mut Vec<ZoneColors>,
    ) {
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

    pub(super) fn compose_scene_frame(&mut self, groups: &[Zone]) -> ProducerFrame {
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

    pub(super) fn compose_projected_scene_frame(
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
    pub(super) fn compose_preview_grid_for_test(&mut self, groups: &[Zone]) -> ProducerFrame {
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
