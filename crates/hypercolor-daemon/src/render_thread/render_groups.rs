use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use hypercolor_core::asset::AssetLibrary;
#[cfg(test)]
use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
use hypercolor_core::effect::EffectPool;
#[cfg(test)]
use hypercolor_core::input::ScreenData;
use hypercolor_core::spatial::SpatialEngine;
#[cfg(test)]
use hypercolor_core::spatial::sample_led;
use hypercolor_types::asset::AssetId;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::canvas::{Canvas, RenderSurfacePool, SurfaceDescriptor};
use hypercolor_types::event::HypercolorEvent;
#[cfg(test)]
use hypercolor_types::event::LayerHealth;
#[cfg(test)]
use hypercolor_types::event::ZoneColors;
#[cfg(test)]
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
};
#[cfg(test)]
use hypercolor_types::scene::DisplayFaceTarget;
use hypercolor_types::scene::{Zone, ZoneId};
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
use super::producer_queue::record_producer_frame;
use super::scene_dependency::SceneDependencyKey;
use super::sparkleflinger::SparkleFlinger;
#[cfg(test)]
use super::sparkleflinger::{CompositionPlan, PreviewSurfaceRequest};
use crate::performance::FullFrameCopyMetrics;
#[cfg(all(test, feature = "wgpu"))]
use frame_helpers::media_mime_prefers_gpu_texture;
#[cfg(test)]
use frame_helpers::surface_backed_frame;
use frame_helpers::{passthrough_effect_layer, transparent_black_frame};
use group_state::{combined_led_state, empty_group_layout, enabled_layer_count};
use model::{
    CachedMediaProducer, GroupFrameContext, GroupFrameRequirements, RetainedDirectGroupFrame,
    RetainedMaterializedGroupFrame, RetainedRenderGroupFrame,
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
    scene_surface_pool: RenderSurfacePool,
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
    pub(crate) fn new(scene_width: u32, scene_height: u32) -> Self {
        let (combined_led_layout, combined_led_spatial_engine) =
            combined_led_state(empty_group_layout(scene_width, scene_height));
        Self {
            asset_library: None,
            effect_pool: EffectPool::new(),
            media_producers: HashMap::new(),
            target_canvases: HashMap::new(),
            scene_projection_cache: HashMap::new(),
            spatial_engines: HashMap::new(),
            direct_surface_pools: HashMap::new(),
            retained_direct_group_frames: HashMap::new(),
            retained_materialized_group_frames: HashMap::new(),
            // 8 slots absorbs typical downstream fan-out (watch channel +
            // display-output dispatch + one pin per display worker mid-
            // encode). The higher cap lets preview/display bursts settle
            // into a larger working set instead of reallocating per frame.
            scene_surface_pool: RenderSurfacePool::with_slot_count_and_cap(
                SurfaceDescriptor::rgba8888(scene_width, scene_height),
                SCENE_SURFACE_POOL_INITIAL_SLOTS,
                SCENE_SURFACE_POOL_MAX_SLOTS,
            ),
            reconciled_dependency_key: None,
            retained_frame: None,
            last_effect_error: None,
            recovered_effect_error: None,
            layer_runtime: LayerRuntimeRegistry::default(),
            combined_led_layout,
            combined_led_spatial_engine,
            scene_width,
            scene_height,
        }
    }

    pub(crate) fn with_asset_library(
        scene_width: u32,
        scene_height: u32,
        asset_library: Arc<RwLock<AssetLibrary>>,
    ) -> Self {
        let mut runtime = Self::new(scene_width, scene_height);
        runtime
            .effect_pool
            .set_asset_library(Arc::clone(&asset_library));
        runtime.asset_library = Some(asset_library);
        runtime
    }

    pub(crate) fn asset_library(&self) -> Option<Arc<RwLock<AssetLibrary>>> {
        self.asset_library.clone()
    }

    pub(crate) fn drain_layer_runtime_events(&mut self) -> Vec<HypercolorEvent> {
        self.layer_runtime.drain_events()
    }

    fn render_direct_group_frame(
        &mut self,
        group: &Zone,
        context: GroupFrameContext<'_>,
        sparkleflinger: &mut SparkleFlinger,
        full_frame_copy: &mut FullFrameCopyMetrics,
    ) -> Result<Option<PendingGroupCanvasFrame>> {
        let display_target = group
            .display_target
            .clone()
            .expect("direct display group should carry a display target");

        let empty_direct_shell = enabled_layer_count(group) == 0;
        let frame = if empty_direct_shell {
            self.effect_pool.remove_group(group.id);
            self.retained_materialized_group_frames.remove(&group.id);
            transparent_black_frame(group.layout.canvas_width, group.layout.canvas_height)
        } else if passthrough_effect_layer(group).is_some() {
            let Some(frame) = self.render_passthrough_effect_layer_frame(group, context)? else {
                return Ok(None);
            };
            frame
        } else {
            let Some(frame) = self.render_group_frame(
                group,
                context,
                sparkleflinger,
                GroupFrameRequirements {
                    requires_cpu_sampling_canvas: true,
                    requires_published_surface: true,
                },
            )?
            else {
                return Ok(None);
            };
            frame
        };
        let Some(frame) = self.surface_backed_direct_frame(group.id, frame, full_frame_copy) else {
            return Ok(None);
        };
        record_producer_frame(&frame);
        Ok(Some(PendingGroupCanvasFrame {
            frame,
            display_target,
            empty_direct_shell,
        }))
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
