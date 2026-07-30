use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use super::ZoneRuntime;
use super::group_state::{
    combine_led_group_layouts, combined_led_state, desired_media_asset_ids,
    group_contributes_to_scene_canvas, group_publishes_direct_canvas,
};
use super::projection::build_group_projection;
use crate::render_thread::scene_dependency::SceneDependencyKey;

/// Initial slot count for per-group direct-canvas pools (HTML-face zones).
/// Same failure mode as the scene surface pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_INITIAL_SLOTS: usize = 6;
const DIRECT_SURFACE_POOL_MAX_SLOTS: usize = 32;

struct PreparedSpatialState {
    engines: HashMap<ZoneId, SpatialEngine>,
    combined_layout: Arc<hypercolor_types::spatial::SpatialLayout>,
    combined_engine: SpatialEngine,
}

impl ZoneRuntime {
    pub(crate) fn effect_registry_snapshot(
        &mut self,
        registry: &EffectRegistry,
    ) -> Arc<EffectRegistry> {
        if let Some(snapshot) = self.effect_registry_snapshot.as_ref()
            && snapshot.generation() == registry.generation()
        {
            return Arc::clone(snapshot);
        }

        let snapshot = Arc::new(registry.clone());
        self.effect_registry_snapshot = Some(Arc::clone(&snapshot));
        snapshot
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
        self.combined_led_layout = self.empty_led_spatial_engine.layout();
        self.combined_led_spatial_engine = self.empty_led_spatial_engine.clone();
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
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
    ) -> Result<()> {
        if self.reconciled_dependency_key == Some(dependency_key) {
            return Ok(());
        }

        let prepared_spatial_state =
            self.prepare_spatial_state(groups, authoritative_spatial_engine)?;

        self.effect_pool
            .reconcile(groups, registry, display_descriptors)?;
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
                self.ensure_group_canvas(group)?;
                self.ensure_scene_projection(group);
            }
            if group_publishes_direct_canvas(group) {
                self.ensure_direct_surface_pool(group)?;
            }
        }

        self.spatial_engines = prepared_spatial_state.engines;
        self.combined_led_layout = prepared_spatial_state.combined_layout;
        self.combined_led_spatial_engine = prepared_spatial_state.combined_engine;
        self.reconciled_dependency_key = Some(dependency_key);

        Ok(())
    }

    fn ensure_group_canvas(&mut self, group: &Zone) -> Result<()> {
        let needs_canvas = self.target_canvases.get(&group.id).is_none_or(|canvas| {
            canvas.width() != group.layout.canvas_width
                || canvas.height() != group.layout.canvas_height
        });
        if needs_canvas {
            let canvas = Canvas::try_new(group.layout.canvas_width, group.layout.canvas_height)?;
            self.target_canvases.insert(group.id, canvas);
        }
        Ok(())
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

    fn ensure_direct_surface_pool(&mut self, group: &Zone) -> Result<()> {
        let descriptor =
            SurfaceDescriptor::rgba8888(group.layout.canvas_width, group.layout.canvas_height);
        let needs_pool = self
            .direct_surface_pools
            .get(&group.id)
            .is_none_or(|pool| pool.descriptor() != descriptor);
        if needs_pool {
            let pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(
                descriptor,
                DIRECT_SURFACE_POOL_INITIAL_SLOTS,
                DIRECT_SURFACE_POOL_MAX_SLOTS,
            )?;
            self.direct_surface_pools.insert(group.id, pool);
        }
        Ok(())
    }

    fn prepare_spatial_state(
        &self,
        groups: &[Zone],
        authoritative_spatial_engine: Option<&SpatialEngine>,
    ) -> Result<PreparedSpatialState> {
        let mut engines = HashMap::with_capacity(groups.len());
        for group in groups {
            let engine = if let Some(authoritative_engine) = authoritative_spatial_engine
                .filter(|engine| engine.layout().as_ref() == &group.layout)
            {
                authoritative_engine.clone()
            } else if let Some(existing) = self
                .spatial_engines
                .get(&group.id)
                .filter(|engine| engine.layout().as_ref() == &group.layout)
            {
                existing.clone()
            } else {
                SpatialEngine::try_new(group.layout.clone())?
            };
            engines.insert(group.id, engine);
        }

        let mut contributing_groups = groups
            .iter()
            .filter(|group| group_contributes_to_scene_canvas(group));
        if let Some(group) = contributing_groups.next()
            && contributing_groups.next().is_none()
            && let Some(engine) = engines.get(&group.id)
        {
            let engine = engine.clone();
            return Ok(PreparedSpatialState {
                engines,
                combined_layout: engine.layout(),
                combined_engine: engine,
            });
        }

        let (layout, engine) = combined_led_state(combine_led_group_layouts(
            groups,
            self.scene_width,
            self.scene_height,
        ))?;
        Ok(PreparedSpatialState {
            engines,
            combined_layout: layout,
            combined_engine: engine,
        })
    }
}
