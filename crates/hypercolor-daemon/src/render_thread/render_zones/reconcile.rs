use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_core::effect::{EffectRegistry, PreparedEffectPoolReconcile};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use super::super::layer_runtime::PreparedLayerRuntimeRegistry;
use super::ZoneRuntime;
use super::projection::{build_zone_projection, projection_supports_composition};
use super::zone_state::{
    combine_led_zone_layouts, combined_led_state, desired_media_asset_ids,
    zone_contributes_to_scene_canvas, zone_publishes_direct_canvas,
};
use crate::render_thread::scene_dependency::SceneDependencyKey;
use crate::render_thread::sparkleflinger::CompositionLayer;
use crate::render_thread::sparkleflinger::{ProjectedZoneTextureRequirement, SparkleFlinger};

/// Initial slot count for per-zone direct-canvas pools (HTML-face zones).
/// Same failure mode as the scene surface pool, but at smaller canvas sizes; still
/// needs room for watch channel + in-flight display encode.
const DIRECT_SURFACE_POOL_INITIAL_SLOTS: usize = 6;
const DIRECT_SURFACE_POOL_MAX_SLOTS: usize = 32;

struct PreparedSpatialState {
    engines: HashMap<ZoneId, SpatialEngine>,
    combined_layout: Arc<hypercolor_types::spatial::SpatialLayout>,
    combined_engine: SpatialEngine,
}

enum PreparedSceneBacking {
    Unresolved,
    GpuOnly {
        projected_scene_layers: Option<Vec<CompositionLayer>>,
    },
    Cpu {
        scene_surface_pool: Option<RenderSurfacePool>,
    },
}

pub(crate) struct PreparedZoneReconcile {
    effect_pool: PreparedEffectPoolReconcile,
    layer_runtime: PreparedLayerRuntimeRegistry,
    spatial: PreparedSpatialState,
    target_canvases: HashMap<ZoneId, Canvas>,
    scene_canvas_requirements: Vec<ProjectedZoneTextureRequirement>,
    scene_projection_cache: HashMap<ZoneId, super::projection::CachedZoneProjection>,
    direct_surface_pools: HashMap<ZoneId, RenderSurfacePool>,
    media_producers: HashMap<hypercolor_types::asset::AssetId, super::model::CachedMediaProducer>,
    retained_direct_zone_frames: HashMap<ZoneId, super::model::RetainedDirectZoneFrame>,
    retained_materialized_zone_frames: HashMap<ZoneId, super::model::RetainedMaterializedZoneFrame>,
    desired_media_ids: HashSet<hypercolor_types::asset::AssetId>,
    scene_zone_ids: HashSet<ZoneId>,
    direct_zone_ids: HashSet<ZoneId>,
    projected_zone_texture_requirements: Vec<ProjectedZoneTextureRequirement>,
    projected_scene_layer_capacity: usize,
    scene_width: u32,
    scene_height: u32,
    scene_backing: PreparedSceneBacking,
    dependency_key: SceneDependencyKey,
}

impl PreparedZoneReconcile {
    pub(crate) fn projected_zone_texture_requirements(&self) -> &[ProjectedZoneTextureRequirement] {
        &self.projected_zone_texture_requirements
    }

    pub(crate) const fn scene_dimensions(&self) -> (u32, u32) {
        (self.scene_width, self.scene_height)
    }

    pub(crate) fn resolve_scene_backing(
        &mut self,
        runtime: &ZoneRuntime,
        gpu_projection_admitted: bool,
        scene_pool_prepared_by_resize: bool,
    ) -> Result<()> {
        if gpu_projection_admitted {
            let projected_scene_layers = if gpu_projection_admitted
                && runtime.projected_scene_layers.capacity() < self.projected_scene_layer_capacity
            {
                let mut layers = Vec::new();
                layers.try_reserve_exact(self.projected_scene_layer_capacity)?;
                Some(layers)
            } else {
                None
            };
            self.scene_backing = PreparedSceneBacking::GpuOnly {
                projected_scene_layers,
            };
            return Ok(());
        }

        for requirement in &self.scene_canvas_requirements {
            let reusable = runtime
                .target_canvases
                .get(&requirement.zone_id)
                .is_some_and(|canvas| {
                    canvas.width() == requirement.width && canvas.height() == requirement.height
                });
            if !reusable {
                self.target_canvases.insert(
                    requirement.zone_id,
                    Canvas::try_new(requirement.width, requirement.height)?,
                );
            }
        }
        let descriptor = SurfaceDescriptor::rgba8888(self.scene_width, self.scene_height);
        let scene_surface_pool = if scene_pool_prepared_by_resize
            || runtime
                .scene_surface_pool
                .as_ref()
                .is_some_and(|pool| pool.descriptor() == descriptor)
        {
            None
        } else {
            Some(ZoneRuntime::prepare_scene_surface_pool(
                self.scene_width,
                self.scene_height,
                runtime.scene_surface_pool_initial_slots,
                runtime.scene_surface_pool_max_slots,
            )?)
        };
        self.scene_backing = PreparedSceneBacking::Cpu { scene_surface_pool };
        Ok(())
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
        zones: &[Zone],
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
        let mut prepared = self.prepare_reconcile(
            zones,
            active_scene_id,
            dependency_key,
            registry,
            display_descriptors,
            authoritative_spatial_engine,
        )?;
        let gpu_projection_admitted = sparkleflinger.supports_gpu_output_frames();
        let (scene_width, scene_height) = prepared.scene_dimensions();
        let projected = sparkleflinger.prepare_projected_scene_resources(
            prepared.projected_zone_texture_requirements(),
            gpu_projection_admitted,
            scene_width,
            scene_height,
            None,
        );
        prepared.resolve_scene_backing(self, projected.gpu_projection_admitted(), false)?;
        self.commit_reconcile(prepared)?;
        sparkleflinger.apply_projected_scene_resources(projected);
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

    pub(crate) fn clear_inactive_zones(&mut self) {
        if !self.has_inactive_zone_resources() {
            return;
        }

        self.effect_pool.clear();
        self.media_producers.clear();
        self.target_canvases.clear();
        self.scene_surface_pool = None;
        self.projected_scene_layers.clear();
        self.scene_projection_cache.clear();
        self.spatial_engines.clear();
        self.direct_surface_pools.clear();
        self.retained_direct_zone_frames.clear();
        self.retained_materialized_zone_frames.clear();
        self.reconciled_dependency_key = None;
        self.retained_frame = None;
        self.last_effect_error = None;
        self.recovered_effect_error = None;
        self.layer_runtime.clear();
        self.combined_led_layout = self.empty_led_spatial_engine.layout();
        self.combined_led_spatial_engine = self.empty_led_spatial_engine.clone();
    }

    pub(super) fn has_inactive_zone_resources(&self) -> bool {
        self.effect_pool.slot_count() > 0
            || !self.target_canvases.is_empty()
            || !self.scene_projection_cache.is_empty()
            || !self.spatial_engines.is_empty()
            || !self.direct_surface_pools.is_empty()
            || !self.retained_direct_zone_frames.is_empty()
            || !self.retained_materialized_zone_frames.is_empty()
            || self.retained_frame.is_some()
            || self.reconciled_dependency_key.is_some()
    }

    #[cfg(test)]
    pub(super) fn reconcile(
        &mut self,
        zones: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
    ) -> Result<()> {
        if self.reconciled_dependency_key == Some(dependency_key) {
            return Ok(());
        }

        let mut prepared = self.prepare_reconcile(
            zones,
            active_scene_id,
            dependency_key,
            registry,
            display_descriptors,
            authoritative_spatial_engine,
        )?;
        prepared.resolve_scene_backing(self, false, false)?;
        self.commit_reconcile(prepared)
    }

    pub(crate) fn prepare_reconcile(
        &self,
        zones: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
    ) -> Result<PreparedZoneReconcile> {
        self.prepare_reconcile_for_scene_dimensions(
            zones,
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
        reason = "layout admission needs candidate zones, registry, spatial plan, and geometry"
    )]
    pub(crate) fn prepare_reconcile_for_scene_dimensions(
        &self,
        zones: &[Zone],
        active_scene_id: Option<SceneId>,
        dependency_key: SceneDependencyKey,
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
        authoritative_spatial_engine: Option<&SpatialEngine>,
        scene_width: u32,
        scene_height: u32,
    ) -> Result<PreparedZoneReconcile> {
        let spatial = self.prepare_spatial_state(
            zones,
            authoritative_spatial_engine,
            scene_width,
            scene_height,
        )?;
        let effect_pool =
            self.effect_pool
                .prepare_reconcile(zones, registry, display_descriptors)?;
        let layer_runtime = self
            .layer_runtime
            .prepare_reconcile(active_scene_id, zones)?;
        let desired_media_ids = desired_media_asset_ids(zones);
        let scene_zone_ids = zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone))
            .map(|zone| zone.id)
            .collect::<HashSet<_>>();
        let direct_zone_ids = zones
            .iter()
            .filter(|zone| zone_publishes_direct_canvas(zone))
            .map(|zone| zone.id)
            .collect::<HashSet<_>>();
        let mut target_canvases = HashMap::new();
        target_canvases.try_reserve(scene_zone_ids.len())?;
        let mut scene_canvas_requirements = Vec::new();
        scene_canvas_requirements.try_reserve_exact(scene_zone_ids.len())?;
        let mut scene_projection_cache = HashMap::new();
        scene_projection_cache.try_reserve(scene_zone_ids.len())?;
        let mut direct_surface_pools = HashMap::new();
        direct_surface_pools.try_reserve(direct_zone_ids.len())?;
        let mut media_producers = HashMap::new();
        media_producers.try_reserve(desired_media_ids.len())?;
        let mut retained_direct_zone_frames = HashMap::new();
        retained_direct_zone_frames.try_reserve(direct_zone_ids.len())?;
        let mut retained_materialized_zone_frames = HashMap::new();
        retained_materialized_zone_frames.try_reserve(direct_zone_ids.len())?;
        let mut gpu_projection_eligible = !scene_zone_ids.is_empty();
        let mut projected_scene_layer_capacity = 1_usize;

        for zone in zones {
            if zone_contributes_to_scene_canvas(zone) {
                scene_canvas_requirements.push(ProjectedZoneTextureRequirement {
                    zone_id: zone.id,
                    width: zone.layout.canvas_width,
                    height: zone.layout.canvas_height,
                });

                let needs_projection =
                    self.scene_projection_cache
                        .get(&zone.id)
                        .is_none_or(|projection| {
                            projection.scene_width != scene_width
                                || projection.scene_height != scene_height
                                || projection.layout != zone.layout
                        });
                if needs_projection {
                    scene_projection_cache.insert(
                        zone.id,
                        build_zone_projection(zone, scene_width, scene_height)?,
                    );
                }
                let projection = scene_projection_cache
                    .get(&zone.id)
                    .or_else(|| self.scene_projection_cache.get(&zone.id))
                    .expect("scene contributors must have prepared projection metadata");
                gpu_projection_eligible &= projection_supports_composition(projection);
                projected_scene_layer_capacity = projected_scene_layer_capacity
                    .checked_add(projection.zones.len())
                    .context("projected scene layer cardinality overflowed")?;
            }

            if zone_publishes_direct_canvas(zone) {
                let descriptor = SurfaceDescriptor::rgba8888(
                    zone.layout.canvas_width,
                    zone.layout.canvas_height,
                );
                let needs_pool = self
                    .direct_surface_pools
                    .get(&zone.id)
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
                    direct_surface_pools.insert(zone.id, pool);
                }
            }
        }

        let projected_zone_texture_requirements = if gpu_projection_eligible {
            let mut requirements = Vec::new();
            requirements.try_reserve_exact(scene_canvas_requirements.len())?;
            requirements.extend(scene_canvas_requirements.iter().copied());
            requirements
        } else {
            projected_scene_layer_capacity = 0;
            Vec::new()
        };

        Ok(PreparedZoneReconcile {
            effect_pool,
            layer_runtime,
            spatial,
            target_canvases,
            scene_canvas_requirements,
            scene_projection_cache,
            direct_surface_pools,
            media_producers,
            retained_direct_zone_frames,
            retained_materialized_zone_frames,
            desired_media_ids,
            scene_zone_ids,
            direct_zone_ids,
            projected_zone_texture_requirements,
            projected_scene_layer_capacity,
            scene_width,
            scene_height,
            scene_backing: PreparedSceneBacking::Unresolved,
            dependency_key,
        })
    }

    pub(crate) fn commit_reconcile(&mut self, prepared: PreparedZoneReconcile) -> Result<()> {
        let PreparedZoneReconcile {
            effect_pool,
            layer_runtime,
            spatial,
            mut target_canvases,
            scene_canvas_requirements: _,
            mut scene_projection_cache,
            mut direct_surface_pools,
            mut media_producers,
            mut retained_direct_zone_frames,
            mut retained_materialized_zone_frames,
            desired_media_ids,
            scene_zone_ids,
            direct_zone_ids,
            projected_zone_texture_requirements: _,
            projected_scene_layer_capacity: _,
            scene_width: _,
            scene_height: _,
            scene_backing,
            dependency_key,
        } = prepared;
        self.effect_pool.commit_reconcile(effect_pool)?;
        self.layer_runtime.commit_reconcile(layer_runtime);
        for (asset_id, producer) in std::mem::take(&mut self.media_producers) {
            if desired_media_ids.contains(&asset_id) {
                media_producers.insert(asset_id, producer);
            }
        }
        if matches!(scene_backing, PreparedSceneBacking::Cpu { .. }) {
            for (zone_id, canvas) in std::mem::take(&mut self.target_canvases) {
                if scene_zone_ids.contains(&zone_id) && !target_canvases.contains_key(&zone_id) {
                    target_canvases.insert(zone_id, canvas);
                }
            }
        }
        for (zone_id, projection) in std::mem::take(&mut self.scene_projection_cache) {
            if scene_zone_ids.contains(&zone_id) && !scene_projection_cache.contains_key(&zone_id) {
                scene_projection_cache.insert(zone_id, projection);
            }
        }
        for (zone_id, pool) in std::mem::take(&mut self.direct_surface_pools) {
            if direct_zone_ids.contains(&zone_id) && !direct_surface_pools.contains_key(&zone_id) {
                direct_surface_pools.insert(zone_id, pool);
            }
        }
        for (zone_id, frame) in std::mem::take(&mut self.retained_direct_zone_frames) {
            if direct_zone_ids.contains(&zone_id) {
                retained_direct_zone_frames.insert(zone_id, frame);
            }
        }
        for (zone_id, frame) in std::mem::take(&mut self.retained_materialized_zone_frames) {
            if direct_zone_ids.contains(&zone_id) {
                retained_materialized_zone_frames.insert(zone_id, frame);
            }
        }
        self.media_producers = media_producers;
        self.target_canvases = target_canvases;
        match scene_backing {
            PreparedSceneBacking::GpuOnly {
                projected_scene_layers,
            } => {
                self.scene_surface_pool = None;
                if let Some(layers) = projected_scene_layers {
                    self.projected_scene_layers = layers;
                    #[cfg(all(test, feature = "wgpu"))]
                    {
                        self.projected_scene_layer_allocation_count = self
                            .projected_scene_layer_allocation_count
                            .saturating_add(1);
                    }
                }
                self.projected_scene_layers.clear();
            }
            PreparedSceneBacking::Cpu { scene_surface_pool } => {
                if let Some(pool) = scene_surface_pool {
                    self.scene_surface_pool = Some(pool);
                }
                self.projected_scene_layers.clear();
            }
            PreparedSceneBacking::Unresolved => {
                unreachable!("scene backing must be resolved before reconcile commit")
            }
        }
        self.scene_projection_cache = scene_projection_cache;
        self.direct_surface_pools = direct_surface_pools;
        self.retained_direct_zone_frames = retained_direct_zone_frames;
        self.retained_materialized_zone_frames = retained_materialized_zone_frames;
        self.spatial_engines = spatial.engines;
        self.combined_led_layout = spatial.combined_layout;
        self.combined_led_spatial_engine = spatial.combined_engine;
        self.reconciled_dependency_key = Some(dependency_key);
        Ok(())
    }

    fn prepare_spatial_state(
        &self,
        zones: &[Zone],
        authoritative_spatial_engine: Option<&SpatialEngine>,
        scene_width: u32,
        scene_height: u32,
    ) -> Result<PreparedSpatialState> {
        let mut engines = HashMap::with_capacity(zones.len());
        for zone in zones {
            let engine = if let Some(authoritative_engine) = authoritative_spatial_engine
                .filter(|engine| engine.layout().as_ref() == &zone.layout)
            {
                authoritative_engine.clone()
            } else if let Some(existing) = self
                .spatial_engines
                .get(&zone.id)
                .filter(|engine| engine.layout().as_ref() == &zone.layout)
            {
                existing.clone()
            } else {
                SpatialEngine::try_new(zone.layout.clone())?
            };
            engines.insert(zone.id, engine);
        }

        let mut contributing_zones = zones
            .iter()
            .filter(|zone| zone_contributes_to_scene_canvas(zone));
        if let Some(zone) = contributing_zones.next()
            && contributing_zones.next().is_none()
            && let Some(engine) = engines.get(&zone.id)
        {
            let engine = engine.clone();
            return Ok(PreparedSpatialState {
                engines,
                combined_layout: engine.layout(),
                combined_engine: engine,
            });
        }

        let (layout, engine) =
            combined_led_state(combine_led_zone_layouts(zones, scene_width, scene_height))?;
        Ok(PreparedSpatialState {
            engines,
            combined_layout: layout,
            combined_engine: engine,
        })
    }
}
