use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use hypercolor_core::asset::AssetLibrary;
#[cfg(test)]
use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
use hypercolor_core::effect::{EffectPool, EffectRegistry};
#[cfg(test)]
use hypercolor_core::input::ScreenData;
use hypercolor_core::spatial::SpatialEngine;
#[cfg(test)]
use hypercolor_core::spatial::sample_led;
use hypercolor_types::asset::AssetId;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::canvas::{
    Canvas, RenderSurfacePool, SurfaceDescriptor, SurfaceResourceError,
};
use hypercolor_types::event::HypercolorEvent;
#[cfg(test)]
use hypercolor_types::event::LayerHealth;
#[cfg(test)]
use hypercolor_types::event::ZoneColors;
#[cfg(test)]
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
};
use hypercolor_types::scene::ZoneId;
#[cfg(test)]
use hypercolor_types::scene::{DisplayFaceTarget, Zone};
#[cfg(test)]
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;
#[cfg(test)]
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode};

#[cfg(test)]
use super::frame_sampling::LedSamplingStrategy;
use super::layer_runtime::LayerRuntimeRegistry;
#[cfg(test)]
use super::producer_queue::ProducerFrame;
use super::scene_dependency::SceneDependencyKey;
#[cfg(test)]
use super::sparkleflinger::SparkleFlinger;
#[cfg(test)]
use super::sparkleflinger::{CompositionPlan, PreviewSurfaceRequest};
#[cfg(test)]
use crate::performance::FullFrameCopyMetrics;
use frame_helpers::StaticLayerSurfaceCache;
#[cfg(all(test, feature = "wgpu"))]
use frame_helpers::media_mime_prefers_gpu_texture;
#[cfg(test)]
use frame_helpers::passthrough_effect_layer;
#[cfg(test)]
use frame_helpers::surface_backed_frame;
#[cfg(test)]
use frame_helpers::{color_fill_frame, transparent_black_frame};
use group_state::{combined_led_state, empty_group_layout};
use model::{
    CachedMediaProducer, RetainedDirectGroupFrame, RetainedMaterializedGroupFrame,
    RetainedRenderGroupFrame,
};
pub(crate) use model::{
    GroupCanvasFrame, PendingGroupCanvasFrame, RenderSceneContext, ZoneEffectError,
    ZoneFrameInputs, ZoneResult,
};
use projection::CachedGroupProjection;
#[cfg(test)]
use projection::{
    blit_zone_projection, copy_full_scene_identity_projection,
    projection_composition_layers_for_group, zone_local_position_for_scene_pixel,
};

/// Initial slot count for the full-resolution scene surface pool. Sized to absorb
/// typical downstream pins: the canvas watch channel, display-output
/// dispatch, and one in-flight JPEG encode per HTML-face worker. Undersizing
/// forces `begin_dequeue` to reallocate a fresh canvas every frame whenever
/// all slots are still shared downstream, which shows up as producer-stage
/// stalls proportional to `canvas_width * canvas_height * 4` bytes.
const SCENE_SURFACE_POOL_INITIAL_SLOTS: usize = 8;
const SCENE_SURFACE_POOL_MAX_SLOTS: usize = 64;
const PREVIEW_SCENE_SURFACE_POOL_INITIAL_SLOTS: usize = 2;
const PREVIEW_SCENE_SURFACE_POOL_MAX_SLOTS: usize = 16;

pub(crate) struct ZoneRuntime {
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    effect_pool: EffectPool,
    media_producers: HashMap<AssetId, CachedMediaProducer>,
    target_canvases: HashMap<ZoneId, Canvas>,
    scene_projection_cache: HashMap<ZoneId, CachedGroupProjection>,
    spatial_engines: HashMap<ZoneId, SpatialEngine>,
    direct_surface_pools: HashMap<ZoneId, RenderSurfacePool>,
    retained_direct_group_frames: HashMap<ZoneId, RetainedDirectGroupFrame>,
    retained_materialized_group_frames: HashMap<ZoneId, RetainedMaterializedGroupFrame>,
    effect_registry_snapshot: Option<Arc<EffectRegistry>>,
    static_layer_surface_cache: StaticLayerSurfaceCache,
    scene_surface_pool: RenderSurfacePool,
    scene_surface_pool_initial_slots: usize,
    scene_surface_pool_max_slots: usize,
    reconciled_dependency_key: Option<SceneDependencyKey>,
    retained_frame: Option<RetainedRenderGroupFrame>,
    last_effect_error: Option<ZoneEffectError>,
    recovered_effect_error: Option<ZoneEffectError>,
    layer_runtime: LayerRuntimeRegistry,
    combined_led_layout: Arc<SpatialLayout>,
    combined_led_spatial_engine: SpatialEngine,
    scene_width: u32,
    scene_height: u32,
}

impl ZoneRuntime {
    #[cfg(any(test, feature = "wgpu"))]
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Self {
        Self::try_new(scene_width, scene_height)
            .expect("default scene dimensions must fit available memory")
    }

    pub(crate) fn try_new(
        scene_width: u32,
        scene_height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        Self::try_with_scene_surface_pool(
            scene_width,
            scene_height,
            SCENE_SURFACE_POOL_INITIAL_SLOTS,
            SCENE_SURFACE_POOL_MAX_SLOTS,
        )
    }

    pub(crate) fn try_new_preview(
        scene_width: u32,
        scene_height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        Self::try_with_scene_surface_pool(
            scene_width,
            scene_height,
            PREVIEW_SCENE_SURFACE_POOL_INITIAL_SLOTS,
            PREVIEW_SCENE_SURFACE_POOL_MAX_SLOTS,
        )
    }

    fn try_with_scene_surface_pool(
        scene_width: u32,
        scene_height: u32,
        initial_slots: usize,
        max_slots: usize,
    ) -> Result<Self, SurfaceResourceError> {
        let (combined_led_layout, combined_led_spatial_engine) =
            combined_led_state(empty_group_layout(scene_width, scene_height));
        let mut scene_surface_pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(
            SurfaceDescriptor::rgba8888(scene_width, scene_height),
            initial_slots,
            max_slots,
        )?;
        scene_surface_pool
            .try_dequeue()?
            .expect("new scene surface pool must expose an initial slot")
            .release();
        Ok(Self {
            asset_library: None,
            effect_pool: EffectPool::new(),
            media_producers: HashMap::new(),
            target_canvases: HashMap::new(),
            scene_projection_cache: HashMap::new(),
            spatial_engines: HashMap::new(),
            direct_surface_pools: HashMap::new(),
            retained_direct_group_frames: HashMap::new(),
            retained_materialized_group_frames: HashMap::new(),
            effect_registry_snapshot: None,
            static_layer_surface_cache: StaticLayerSurfaceCache::default(),
            // 8 slots absorbs typical downstream fan-out (watch channel +
            // display-output dispatch + one pin per display worker mid-
            // encode). The higher cap lets preview/display bursts settle
            // into a larger working set instead of reallocating per frame.
            scene_surface_pool,
            scene_surface_pool_initial_slots: initial_slots,
            scene_surface_pool_max_slots: max_slots,
            reconciled_dependency_key: None,
            retained_frame: None,
            last_effect_error: None,
            recovered_effect_error: None,
            layer_runtime: LayerRuntimeRegistry::default(),
            combined_led_layout,
            combined_led_spatial_engine,
            scene_width,
            scene_height,
        })
    }

    pub(crate) fn try_resize_scene(
        &mut self,
        scene_width: u32,
        scene_height: u32,
    ) -> Result<(), SurfaceResourceError> {
        if self.scene_width == scene_width && self.scene_height == scene_height {
            return Ok(());
        }

        let mut scene_surface_pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(
            SurfaceDescriptor::rgba8888(scene_width, scene_height),
            self.scene_surface_pool_initial_slots,
            self.scene_surface_pool_max_slots,
        )?;
        scene_surface_pool
            .try_dequeue()?
            .expect("replacement scene surface pool must expose an initial slot")
            .release();

        self.scene_width = scene_width;
        self.scene_height = scene_height;
        self.target_canvases.clear();
        self.scene_projection_cache.clear();
        self.scene_surface_pool = scene_surface_pool;
        self.retained_frame = None;
        self.reconciled_dependency_key = None;
        let (layout, engine) = combined_led_state(empty_group_layout(scene_width, scene_height));
        self.combined_led_layout = layout;
        self.combined_led_spatial_engine = engine;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn with_asset_library(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Self {
        Self::try_with_asset_library(scene_width, scene_height, asset_library)
            .expect("default scene dimensions must fit available memory")
    }

    pub(crate) fn try_with_asset_library(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Result<Self, SurfaceResourceError> {
        let mut runtime = Self::try_new(scene_width, scene_height)?;
        runtime
            .effect_pool
            .set_asset_library(Arc::clone(&asset_library));
        runtime.asset_library = Some(asset_library);
        Ok(runtime)
    }

    pub(crate) fn try_with_asset_library_preview(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Result<Self, SurfaceResourceError> {
        let mut runtime = Self::try_new_preview(scene_width, scene_height)?;
        runtime
            .effect_pool
            .set_asset_library(Arc::clone(&asset_library));
        runtime.asset_library = Some(asset_library);
        Ok(runtime)
    }

    pub(crate) fn asset_library(&self) -> Option<Arc<RwLock<AssetLibrary>>> {
        self.asset_library.clone()
    }

    pub(crate) fn drain_layer_runtime_events(&mut self) -> Vec<HypercolorEvent> {
        self.layer_runtime.drain_events()
    }
}

mod display_retention;
mod effect_errors;
mod frame_helpers;
mod group_state;
mod layer_rendering;
mod model;
mod projection;
mod reconcile;
mod render_pass;
mod scene_assembly;
mod scene_output;
mod scene_retention;
mod surface_pools;
#[cfg(test)]
mod tests;
