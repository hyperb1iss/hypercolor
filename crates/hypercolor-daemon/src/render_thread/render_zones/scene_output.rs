#[cfg(test)]
use std::collections::HashMap;

use anyhow::Result;
use hypercolor_core::spatial::SpatialSamplingError;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::Zone;
#[cfg(test)]
use hypercolor_types::scene::ZoneId;

use super::super::producer_queue::{ProducerFrame, record_producer_frame};
#[cfg(feature = "wgpu")]
use super::super::sparkleflinger::{CompositionLayer, SparkleFlinger};
use super::ZoneRuntime;
#[cfg(feature = "wgpu")]
use super::frame_helpers::composed_frame_to_producer_frame;
use super::projection::compose_authoritative_scene_canvas;
use super::zone_state::zone_contributes_to_scene_canvas;

impl ZoneRuntime {
    pub(super) fn sample_scene_zone_led_zones(
        &mut self,
        zones: &[Zone],
        zone_colors: &mut Vec<ZoneColors>,
    ) -> Result<(), SpatialSamplingError> {
        self.zone_sampling_scratch.clear();
        for zone in zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone))
        {
            let Some(target) = self.target_canvases.get(&zone.id) else {
                continue;
            };
            let Some(spatial_engine) = self.spatial_engines.get(&zone.id) else {
                continue;
            };
            let start = self.zone_sampling_scratch.len();
            if let Err(error) = spatial_engine.try_sample_append_into_at(
                target,
                &mut self.zone_sampling_scratch,
                start,
            ) {
                self.zone_sampling_scratch.clear();
                return Err(error);
            }
        }

        std::mem::swap(zone_colors, &mut self.zone_sampling_scratch);
        Ok(())
    }

    /// Admit what composing a scene on the CPU needs, a target canvas per
    /// scene zone and the scene surface pool, for a frame that cannot stay
    /// GPU-resident although reconcile admitted the scene that way and left
    /// both out. Reconcile drops them again the next time it admits the
    /// scene GPU-resident, so this warns once per admission.
    pub(super) fn admit_cpu_scene_fallback(&mut self, zones: &[Zone]) -> Result<()> {
        let mut admitted_targets = 0_usize;
        for zone in zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone))
        {
            let (width, height) = (zone.layout.canvas_width, zone.layout.canvas_height);
            if self
                .target_canvases
                .get(&zone.id)
                .is_some_and(|target| target.width() == width && target.height() == height)
            {
                continue;
            }
            self.target_canvases.try_reserve(1)?;
            self.target_canvases
                .insert(zone.id, Canvas::try_new(width, height)?);
            admitted_targets = admitted_targets.saturating_add(1);
        }
        let admitted_scene_surface = self.admit_cpu_scene_surface_pool()?;
        if admitted_targets > 0 || admitted_scene_surface {
            tracing::warn!(
                admitted_targets,
                admitted_scene_surface,
                "scene zones lost GPU residency; composing them on the CPU"
            );
        }
        Ok(())
    }

    /// Admit the CPU scene surface pool when reconcile left the scene
    /// without one, returning whether it had to.
    fn admit_cpu_scene_surface_pool(&mut self) -> Result<bool> {
        if self.scene_surface_pool.is_some() {
            return Ok(false);
        }
        self.scene_surface_pool = Some(Self::prepare_scene_surface_pool(
            self.scene_width,
            self.scene_height,
            self.scene_surface_pool_initial_slots,
            self.scene_surface_pool_max_slots,
        )?);
        Ok(true)
    }

    /// Reconstruct the scene canvas on the CPU from the zones' target
    /// canvases. A GPU-admitted scene arrives here without a pool only when
    /// its projection could not compose, so the pool is admitted on demand
    /// rather than failing the frame.
    pub(super) fn compose_scene_frame(&mut self, zones: &[Zone]) -> Result<ProducerFrame> {
        if self.admit_cpu_scene_surface_pool()? {
            tracing::warn!("scene projection unavailable; composing the scene canvas on the CPU");
        }
        let scene_surface_pool = self
            .scene_surface_pool
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("CPU scene composition was not admitted"))?;
        let Some(mut lease) = scene_surface_pool.try_dequeue()? else {
            let mut scene_canvas = Canvas::try_new(self.scene_width, self.scene_height)?;
            compose_authoritative_scene_canvas(
                &mut scene_canvas,
                zones,
                &self.target_canvases,
                self.scene_width,
                self.scene_height,
                &self.scene_projection_cache,
            );
            let frame = ProducerFrame::Canvas(scene_canvas);
            record_producer_frame(&frame);
            return Ok(frame);
        };

        compose_authoritative_scene_canvas(
            lease.canvas_mut(),
            zones,
            &self.target_canvases,
            self.scene_width,
            self.scene_height,
            &self.scene_projection_cache,
        );

        let frame = ProducerFrame::Surface(lease.submit(0, 0));
        record_producer_frame(&frame);
        Ok(frame)
    }

    #[cfg(feature = "wgpu")]
    pub(super) fn compose_projected_scene_frame(
        &mut self,
        mut layers: Vec<CompositionLayer>,
        sparkleflinger: &mut SparkleFlinger,
    ) -> Option<ProducerFrame> {
        #[cfg(all(test, feature = "wgpu"))]
        if std::mem::take(&mut self.fail_next_projected_scene_composition) {
            layers.clear();
            self.projected_scene_layers = layers;
            return None;
        }
        if layers.is_empty() {
            self.projected_scene_layers = layers;
            return None;
        }

        let composed = sparkleflinger.compose_projected_scene_layers(
            self.scene_width,
            self.scene_height,
            &mut layers,
        );
        let frame = composed_frame_to_producer_frame(composed, sparkleflinger, true);
        layers.clear();
        self.projected_scene_layers = layers;
        if let Some(frame) = frame {
            record_producer_frame(&frame);
            return Some(frame);
        }

        None
    }

    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn fail_next_projected_scene_composition_for_test(&mut self) {
        self.fail_next_projected_scene_composition = true;
    }

    /// The next `render_scene` fails before rendering any zone, the way an
    /// effect or resource error mid-scene does.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn fail_next_scene_render_for_test(&mut self) {
        self.fail_next_scene_render = true;
    }

    /// Every scene render fails with `error` once its zones rendered, the
    /// way an effect that keeps failing partway through the scene does,
    /// until a call with `None`.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) fn fail_scene_renders_with_effect_error_for_test(
        &mut self,
        error: Option<super::model::ZoneEffectError>,
    ) {
        self.failing_scene_render_effect_error = error;
    }

    /// Whether an injected scene failure is still waiting for a render.
    #[cfg(all(test, feature = "wgpu"))]
    pub(super) const fn injected_scene_failure_pending_for_test(&self) -> bool {
        self.fail_next_scene_render || self.fail_next_projected_scene_composition
    }

    #[cfg(test)]
    pub(super) fn compose_preview_grid_for_test(&mut self, zones: &[Zone]) -> ProducerFrame {
        let Some(scene_surface_pool) = &mut self.scene_surface_pool else {
            let mut preview_grid = Canvas::new(self.scene_width, self.scene_height);
            compose_preview_grid_canvas(
                &mut preview_grid,
                zones,
                &self.target_canvases,
                self.scene_width,
                self.scene_height,
            );
            return ProducerFrame::Canvas(preview_grid);
        };
        let Some(mut lease) = scene_surface_pool.dequeue() else {
            let mut preview_grid = Canvas::new(self.scene_width, self.scene_height);
            compose_preview_grid_canvas(
                &mut preview_grid,
                zones,
                &self.target_canvases,
                self.scene_width,
                self.scene_height,
            );
            return ProducerFrame::Canvas(preview_grid);
        };

        compose_preview_grid_canvas(
            lease.canvas_mut(),
            zones,
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
    zones: &[Zone],
    target_canvases: &HashMap<ZoneId, Canvas>,
    preview_width: u32,
    preview_height: u32,
) {
    let preview_count = zones
        .iter()
        .filter(|zone| zone_contributes_to_scene_canvas(zone))
        .count();

    if preview_count == 0 {
        preview.clear();
        return;
    }

    if preview_count == 1
        && let Some(source) = zones
            .iter()
            .find(|zone| zone_contributes_to_scene_canvas(zone))
            .and_then(|zone| target_canvases.get(&zone.id))
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
    for (index, zone) in zones
        .iter()
        .filter(|zone| zone_contributes_to_scene_canvas(zone))
        .enumerate()
    {
        let Some(source) = target_canvases.get(&zone.id) else {
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
