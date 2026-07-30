use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::effect::{EffectRegistry, PreparedEffectPoolReconcile};
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

pub(crate) struct PreparedZoneReconcile {
    effect_pool: PreparedEffectPoolReconcile,
    spatial: PreparedSpatialState,
    replacement_canvases: HashMap<ZoneId, Canvas>,
    replacement_projections: HashMap<ZoneId, super::projection::CachedGroupProjection>,
    replacement_direct_pools: HashMap<ZoneId, RenderSurfacePool>,
    desired_group_ids: HashSet<ZoneId>,
    desired_media_ids: HashSet<hypercolor_types::asset::AssetId>,
    scene_group_ids: HashSet<ZoneId>,
    direct_group_ids: HashSet<ZoneId>,
    active_scene_id: Option<SceneId>,
    dependency_key: SceneDependencyKey,
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

        let prepared = self.prepare_reconcile(
            groups,
            active_scene_id,
            dependency_key,
            registry,
            display_descriptors,
            authoritative_spatial_engine,
        )?;
        self.commit_reconcile(prepared, groups);
        Ok(())
    }

    pub(crate) fn prepare_reconcile(
        &self,
        groups: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
    ) -> Result<PreparedZoneReconcile> {
        self.prepare_reconcile_for_scene_dimensions(
            groups,
            active_scene_id,
            dependency_key,
            registry,
            display_descriptors,
            authoritative_spatial_engine,
            self.scene_width,
            self.scene_height,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "layout admission needs candidate groups, registry, spatial plan, and geometry"
    )]
    pub(crate) fn prepare_reconcile_for_scene_dimensions(
        &self,
        groups: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
        scene_width: u32,
        scene_height: u32,
    ) -> Result<PreparedZoneReconcile> {
        let spatial = self.prepare_spatial_state(
            groups,
            authoritative_spatial_engine,
            scene_width,
            scene_height,
        )?;
        let effect_pool =
            self.effect_pool
                .prepare_reconcile(groups, registry, display_descriptors)?;
        let desired_group_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
        let desired_media_ids = desired_media_asset_ids(groups);
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
        let mut replacement_canvases = HashMap::new();
        let mut replacement_projections = HashMap::new();
        let mut replacement_direct_pools = HashMap::new();

        for group in groups {
            if group_contributes_to_scene_canvas(group) {
                let needs_canvas = self.target_canvases.get(&group.id).is_none_or(|canvas| {
                    canvas.width() != group.layout.canvas_width
                        || canvas.height() != group.layout.canvas_height
                });
                if needs_canvas {
                    replacement_canvases.insert(
                        group.id,
                        Canvas::try_new(group.layout.canvas_width, group.layout.canvas_height)?,
                    );
                }

                let needs_projection =
                    self.scene_projection_cache
                        .get(&group.id)
                        .is_none_or(|projection| {
                            projection.scene_width != scene_width
                                || projection.scene_height != scene_height
                                || projection.layout != group.layout
                        });
                if needs_projection {
                    replacement_projections.insert(
                        group.id,
                        build_group_projection(group, scene_width, scene_height),
                    );
                }
            }

            if group_publishes_direct_canvas(group) {
                let descriptor = SurfaceDescriptor::rgba8888(
                    group.layout.canvas_width,
                    group.layout.canvas_height,
                );
                let needs_pool = self
                    .direct_surface_pools
                    .get(&group.id)
                    .is_none_or(|pool| pool.descriptor() != descriptor);
                if needs_pool {
                    let mut pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(
                        descriptor,
                        DIRECT_SURFACE_POOL_INITIAL_SLOTS,
                        DIRECT_SURFACE_POOL_MAX_SLOTS,
                    )?;
                    pool.try_dequeue()?
                        .expect("new direct surface pool must expose an initial slot")
                        .release();
                    replacement_direct_pools.insert(group.id, pool);
                }
            }
        }

        Ok(PreparedZoneReconcile {
            effect_pool,
            spatial,
            replacement_canvases,
            replacement_projections,
            replacement_direct_pools,
            desired_group_ids,
            desired_media_ids,
            scene_group_ids,
            direct_group_ids,
            active_scene_id,
            dependency_key,
        })
    }

    pub(crate) fn commit_reconcile(&mut self, prepared: PreparedZoneReconcile, groups: &[Zone]) {
        self.effect_pool.commit_reconcile(prepared.effect_pool);
        self.layer_runtime
            .reconcile(prepared.active_scene_id, groups);
        self.media_producers
            .retain(|asset_id, _| prepared.desired_media_ids.contains(asset_id));
        self.target_canvases
            .retain(|group_id, _| prepared.scene_group_ids.contains(group_id));
        self.scene_projection_cache
            .retain(|group_id, _| prepared.scene_group_ids.contains(group_id));
        self.spatial_engines
            .retain(|group_id, _| prepared.desired_group_ids.contains(group_id));
        self.direct_surface_pools
            .retain(|group_id, _| prepared.direct_group_ids.contains(group_id));
        self.retained_direct_group_frames
            .retain(|group_id, _| prepared.direct_group_ids.contains(group_id));
        self.retained_materialized_group_frames
            .retain(|group_id, _| prepared.direct_group_ids.contains(group_id));
        self.target_canvases.extend(prepared.replacement_canvases);
        self.scene_projection_cache
            .extend(prepared.replacement_projections);
        self.direct_surface_pools
            .extend(prepared.replacement_direct_pools);
        self.spatial_engines = prepared.spatial.engines;
        self.combined_led_layout = prepared.spatial.combined_layout;
        self.combined_led_spatial_engine = prepared.spatial.combined_engine;
        self.reconciled_dependency_key = Some(prepared.dependency_key);
    }

    fn prepare_spatial_state(
        &self,
        groups: &[Zone],
        authoritative_spatial_engine: Option<&SpatialEngine>,
        scene_width: u32,
        scene_height: u32,
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

        let (layout, engine) =
            combined_led_state(combine_led_group_layouts(groups, scene_width, scene_height))?;
        Ok(PreparedSpatialState {
            engines,
            combined_layout: layout,
            combined_engine: engine,
        })
    }
}
