use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::effect::{EffectRegistry, PreparedEffectPoolReconcile};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use super::super::layer_runtime::PreparedLayerRuntimeRegistry;
use super::ZoneRuntime;
use super::group_state::{
    combine_led_group_layouts, combined_led_state, desired_media_asset_ids,
    group_contributes_to_scene_canvas, group_publishes_direct_canvas,
};
use super::projection::{build_group_projection, projection_supports_composition};
use crate::render_thread::scene_dependency::SceneDependencyKey;
use crate::render_thread::sparkleflinger::{ProjectedGroupTextureRequirement, SparkleFlinger};

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
    layer_runtime: PreparedLayerRuntimeRegistry,
    spatial: PreparedSpatialState,
    target_canvases: HashMap<ZoneId, Canvas>,
    scene_projection_cache: HashMap<ZoneId, super::projection::CachedGroupProjection>,
    direct_surface_pools: HashMap<ZoneId, RenderSurfacePool>,
    media_producers: HashMap<hypercolor_types::asset::AssetId, super::model::CachedMediaProducer>,
    retained_direct_group_frames: HashMap<ZoneId, super::model::RetainedDirectGroupFrame>,
    retained_materialized_group_frames:
        HashMap<ZoneId, super::model::RetainedMaterializedGroupFrame>,
    desired_media_ids: HashSet<hypercolor_types::asset::AssetId>,
    scene_group_ids: HashSet<ZoneId>,
    direct_group_ids: HashSet<ZoneId>,
    projected_group_texture_requirements: Vec<ProjectedGroupTextureRequirement>,
    dependency_key: SceneDependencyKey,
}

impl PreparedZoneReconcile {
    pub(crate) fn projected_group_texture_requirements(
        &self,
    ) -> &[ProjectedGroupTextureRequirement] {
        &self.projected_group_texture_requirements
    }
}

impl ZoneRuntime {
    pub(crate) fn needs_reconcile(&self, dependency_key: SceneDependencyKey) -> bool {
        self.reconciled_dependency_key != Some(dependency_key)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "frame-boundary admission needs the complete candidate scene state"
    )]
    pub(crate) fn admit_reconcile(
        &mut self,
        groups: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
        sparkleflinger: &mut SparkleFlinger,
    ) -> Result<()> {
        if !self.needs_reconcile(dependency_key) {
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
        let gpu_projection_admitted = sparkleflinger.supports_gpu_output_frames();
        let projected = sparkleflinger.prepare_projected_scene_resources(
            prepared.projected_group_texture_requirements(),
            gpu_projection_admitted,
        );
        sparkleflinger.apply_projected_scene_resources(projected);
        self.commit_reconcile(prepared);
        Ok(())
    }

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

    #[cfg(test)]
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
        self.commit_reconcile(prepared);
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
        let layer_runtime = self
            .layer_runtime
            .prepare_reconcile(active_scene_id, groups)?;
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
        let mut target_canvases = HashMap::new();
        target_canvases.try_reserve(scene_group_ids.len())?;
        let mut scene_projection_cache = HashMap::new();
        scene_projection_cache.try_reserve(scene_group_ids.len())?;
        let mut direct_surface_pools = HashMap::new();
        direct_surface_pools.try_reserve(direct_group_ids.len())?;
        let mut media_producers = HashMap::new();
        media_producers.try_reserve(desired_media_ids.len())?;
        let mut retained_direct_group_frames = HashMap::new();
        retained_direct_group_frames.try_reserve(direct_group_ids.len())?;
        let mut retained_materialized_group_frames = HashMap::new();
        retained_materialized_group_frames.try_reserve(direct_group_ids.len())?;
        let mut gpu_projection_eligible = !scene_group_ids.is_empty();

        for group in groups {
            if group_contributes_to_scene_canvas(group) {
                let needs_canvas = self.target_canvases.get(&group.id).is_none_or(|canvas| {
                    canvas.width() != group.layout.canvas_width
                        || canvas.height() != group.layout.canvas_height
                });
                if needs_canvas {
                    target_canvases.insert(
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
                    scene_projection_cache.insert(
                        group.id,
                        build_group_projection(group, scene_width, scene_height)?,
                    );
                }
                let projection = scene_projection_cache
                    .get(&group.id)
                    .or_else(|| self.scene_projection_cache.get(&group.id))
                    .expect("scene contributors must have prepared projection metadata");
                gpu_projection_eligible &= projection_supports_composition(projection);
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
                    direct_surface_pools.insert(group.id, pool);
                }
            }
        }

        let projected_group_texture_requirements = if gpu_projection_eligible {
            let mut requirements = Vec::new();
            requirements.try_reserve(scene_group_ids.len())?;
            requirements.extend(
                groups
                    .iter()
                    .filter(|group| group_contributes_to_scene_canvas(group))
                    .map(|group| ProjectedGroupTextureRequirement {
                        group_id: group.id,
                        width: group.layout.canvas_width,
                        height: group.layout.canvas_height,
                    }),
            );
            requirements
        } else {
            Vec::new()
        };

        Ok(PreparedZoneReconcile {
            effect_pool,
            layer_runtime,
            spatial,
            target_canvases,
            scene_projection_cache,
            direct_surface_pools,
            media_producers,
            retained_direct_group_frames,
            retained_materialized_group_frames,
            desired_media_ids,
            scene_group_ids,
            direct_group_ids,
            projected_group_texture_requirements,
            dependency_key,
        })
    }

    pub(crate) fn commit_reconcile(&mut self, prepared: PreparedZoneReconcile) {
        let PreparedZoneReconcile {
            effect_pool,
            layer_runtime,
            spatial,
            mut target_canvases,
            mut scene_projection_cache,
            mut direct_surface_pools,
            mut media_producers,
            mut retained_direct_group_frames,
            mut retained_materialized_group_frames,
            desired_media_ids,
            scene_group_ids,
            direct_group_ids,
            projected_group_texture_requirements: _,
            dependency_key,
        } = prepared;
        self.effect_pool.commit_reconcile(effect_pool);
        self.layer_runtime.commit_reconcile(layer_runtime);
        for (asset_id, producer) in std::mem::take(&mut self.media_producers) {
            if desired_media_ids.contains(&asset_id) {
                media_producers.insert(asset_id, producer);
            }
        }
        for (group_id, canvas) in std::mem::take(&mut self.target_canvases) {
            if scene_group_ids.contains(&group_id) && !target_canvases.contains_key(&group_id) {
                target_canvases.insert(group_id, canvas);
            }
        }
        for (group_id, projection) in std::mem::take(&mut self.scene_projection_cache) {
            if scene_group_ids.contains(&group_id)
                && !scene_projection_cache.contains_key(&group_id)
            {
                scene_projection_cache.insert(group_id, projection);
            }
        }
        for (group_id, pool) in std::mem::take(&mut self.direct_surface_pools) {
            if direct_group_ids.contains(&group_id) && !direct_surface_pools.contains_key(&group_id)
            {
                direct_surface_pools.insert(group_id, pool);
            }
        }
        for (group_id, frame) in std::mem::take(&mut self.retained_direct_group_frames) {
            if direct_group_ids.contains(&group_id) {
                retained_direct_group_frames.insert(group_id, frame);
            }
        }
        for (group_id, frame) in std::mem::take(&mut self.retained_materialized_group_frames) {
            if direct_group_ids.contains(&group_id) {
                retained_materialized_group_frames.insert(group_id, frame);
            }
        }
        self.media_producers = media_producers;
        self.target_canvases = target_canvases;
        self.scene_projection_cache = scene_projection_cache;
        self.direct_surface_pools = direct_surface_pools;
        self.retained_direct_group_frames = retained_direct_group_frames;
        self.retained_materialized_group_frames = retained_materialized_group_frames;
        self.spatial_engines = spatial.engines;
        self.combined_led_layout = spatial.combined_layout;
        self.combined_led_spatial_engine = spatial.combined_engine;
        self.reconciled_dependency_key = Some(dependency_key);
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
