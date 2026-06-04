use std::collections::HashSet;

use anyhow::Result;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::scene::{SceneId, Zone};

use super::ZoneRuntime;
use super::group_state::{
    combine_led_group_layouts, combined_led_state, desired_media_asset_ids, empty_group_layout,
    group_contributes_to_scene_canvas, group_publishes_direct_canvas,
};
use super::projection::build_group_projection;
use crate::render_thread::scene_dependency::SceneDependencyKey;

/// Initial slot count for per-group direct-canvas pools (HTML-face zones).
/// Same failure mode as the scene surface pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_INITIAL_SLOTS: usize = 6;
const DIRECT_SURFACE_POOL_MAX_SLOTS: usize = 32;

impl ZoneRuntime {
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

    pub(super) fn has_inactive_group_resources(&self) -> bool {
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

    pub(super) fn reconcile(
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
}
