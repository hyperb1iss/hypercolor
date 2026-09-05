use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::bus::{DisplayZoneOutputRoute, DisplayZoneViewport};
use hypercolor_core::scene::ScenePlanSnapshot;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::display::{DisplayDescriptor, DisplayPixelFormat};
use hypercolor_types::layer::{BindingSource, LayerSource};
use hypercolor_types::scene::{ColorInterpolation, SceneId, UnassignedBehavior, Zone, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SpatialLayout};

use crate::output_power::OutputPowerState;

use super::RenderThreadState;
use super::scene_dependency::SceneDependencyKey;
use super::scene_state::{RenderSceneState, TransitionFrame};
use crate::display_output::{DISPLAY_FACE_DEFAULT_FPS, capped_zone_direct_display_target_fps};

#[derive(Debug, Clone)]
pub(crate) struct SceneTransitionSnapshot {
    pub epoch: u64,
    pub from_scene: Option<SceneId>,
    pub to_scene: Option<SceneId>,
    pub progress: f32,
    pub eased_progress: f32,
    pub color_interpolation: ColorInterpolation,
}

impl Default for SceneTransitionSnapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            from_scene: None,
            to_scene: None,
            progress: 0.0,
            eased_progress: 0.0,
            color_interpolation: ColorInterpolation::Srgb,
        }
    }
}

#[derive(Debug, Clone, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the `active_` prefix keeps the runtime snapshot aligned with scene manager terminology"
)]
pub(crate) struct SceneRuntimeSnapshot {
    pub active_scene_id: Option<SceneId>,
    pub active_scene_name: Option<String>,
    pub active_transition: Option<SceneTransitionSnapshot>,
    pub resolved_zones: Arc<[Zone]>,
    pub resolved_zones_revision: u64,
    pub zone_layout_preview_generation: u64,
    pub active_render_zone_count: u32,
    pub active_display_zone_target_fps: HashMap<ZoneId, u32>,
    pub active_display_zone_output_routes: HashMap<ZoneId, DisplayZoneOutputRoute>,
    pub active_display_zone_descriptors: HashMap<ZoneId, DisplayDescriptor>,
    pub unassigned_behavior: UnassignedBehavior,
    pub device_registry_generation: u64,
}

impl SceneRuntimeSnapshot {
    pub(crate) fn active_render_zone_count(&self) -> u32 {
        self.active_render_zone_count
    }

    pub(crate) fn dependency_key(&self, dependency_generation: u64) -> SceneDependencyKey {
        scene_dependency_key(
            self.resolved_zones_revision,
            dependency_generation,
            self.device_registry_generation,
            self.zone_layout_preview_generation,
            &self.unassigned_behavior,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectDemand {
    pub(crate) effect_running: bool,
    pub(crate) audio_capture_active: bool,
    pub(crate) screen_capture_active: bool,
    pub(crate) interaction_capture_active: bool,
    pub(crate) media_input_active: bool,
    pub(crate) network_input_active: bool,
    pub(crate) sensor_input_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectSceneSnapshot {
    pub(crate) demand: EffectDemand,
    pub(crate) dependency_key: SceneDependencyKey,
}

#[derive(Debug, Clone)]
pub(crate) struct FrameSceneSnapshot {
    pub frame_token: u64,
    pub elapsed_ms: u64,
    pub budget_us: u32,
    pub output_power: OutputPowerState,
    pub effect_demand: EffectDemand,
    pub effect_dependency_key: SceneDependencyKey,
    pub scene_runtime: SceneRuntimeSnapshot,
    pub spatial_engine: SpatialEngine,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderLoopSnapshot {
    pub(crate) frame_token: u64,
    pub(crate) elapsed_ms: u64,
    pub(crate) budget_us: u32,
}

#[derive(Debug, Clone, Default)]
struct CachedDisplayZoneTargetMetadata {
    dependency_key: SceneDependencyKey,
    target_fps: HashMap<ZoneId, u32>,
    output_routes: HashMap<ZoneId, DisplayZoneOutputRoute>,
}

#[derive(Debug, Clone, Copy)]
struct CachedEffectDemand {
    dependency_key: SceneDependencyKey,
    screen_capture_configured: bool,
    demand: EffectDemand,
}

#[derive(Debug, Default)]
pub(crate) struct SceneSnapshotCache {
    cached_display_zone_target_metadata: Option<CachedDisplayZoneTargetMetadata>,
    cached_effect_demand: Option<CachedEffectDemand>,
}

impl SceneSnapshotCache {
    pub const fn new() -> Self {
        Self {
            cached_display_zone_target_metadata: None,
            cached_effect_demand: None,
        }
    }

    pub(crate) fn cached_display_zone_target_metadata(
        &self,
        dependency_key: SceneDependencyKey,
    ) -> Option<(
        HashMap<ZoneId, u32>,
        HashMap<ZoneId, DisplayZoneOutputRoute>,
    )> {
        self.cached_display_zone_target_metadata
            .as_ref()
            .filter(|cache| cache.dependency_key == dependency_key)
            .map(|cache| (cache.target_fps.clone(), cache.output_routes.clone()))
    }

    pub(crate) fn cache_display_zone_target_metadata(
        &mut self,
        dependency_key: SceneDependencyKey,
        target_fps: &HashMap<ZoneId, u32>,
        output_routes: &HashMap<ZoneId, DisplayZoneOutputRoute>,
    ) {
        self.cached_display_zone_target_metadata = Some(CachedDisplayZoneTargetMetadata {
            dependency_key,
            target_fps: target_fps.clone(),
            output_routes: output_routes.clone(),
        });
    }

    pub(crate) fn cached_effect_demand(
        &self,
        dependency_key: SceneDependencyKey,
        screen_capture_configured: bool,
    ) -> Option<EffectDemand> {
        self.cached_effect_demand
            .filter(|cache| {
                cache.dependency_key == dependency_key
                    && cache.screen_capture_configured == screen_capture_configured
            })
            .map(|cache| cache.demand)
    }

    pub(crate) fn cache_effect_demand(
        &mut self,
        dependency_key: SceneDependencyKey,
        screen_capture_configured: bool,
        demand: EffectDemand,
    ) {
        self.cached_effect_demand = Some(CachedEffectDemand {
            dependency_key,
            screen_capture_configured,
            demand,
        });
    }
}

pub(crate) async fn build_frame_scene_snapshot(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    render_scene_state: &mut RenderSceneState,
    delta_secs: f32,
) -> FrameSceneSnapshot {
    let scene_runtime =
        current_scene_runtime_snapshot(state, scene_snapshot_cache, render_scene_state, delta_secs)
            .await;
    let effect_scene = current_effect_scene_snapshot(
        state,
        scene_snapshot_cache,
        &scene_runtime,
        render_scene_state.screen_capture_configured(),
    )
    .await;
    let render_loop_snapshot = render_loop_snapshot(state).await;
    FrameSceneSnapshot {
        frame_token: render_loop_snapshot.frame_token,
        elapsed_ms: render_loop_snapshot.elapsed_ms,
        budget_us: render_loop_snapshot.budget_us,
        output_power: *state.power_state.borrow(),
        effect_demand: effect_scene.demand,
        effect_dependency_key: effect_scene.dependency_key,
        scene_runtime,
        spatial_engine: render_scene_state.spatial_engine().clone(),
    }
}

pub(crate) async fn refresh_effect_scene_snapshot(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    render_scene_state: &RenderSceneState,
    scene_snapshot: &mut FrameSceneSnapshot,
) -> bool {
    let refreshed = current_effect_scene_snapshot(
        state,
        scene_snapshot_cache,
        &scene_snapshot.scene_runtime,
        render_scene_state.screen_capture_configured(),
    )
    .await;
    let changed = refreshed.demand != scene_snapshot.effect_demand
        || refreshed.dependency_key != scene_snapshot.effect_dependency_key;
    scene_snapshot.effect_demand = refreshed.demand;
    scene_snapshot.effect_dependency_key = refreshed.dependency_key;
    changed
}

async fn current_scene_runtime_snapshot(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    render_scene_state: &mut RenderSceneState,
    delta_secs: f32,
) -> SceneRuntimeSnapshot {
    let plan = state.scene_plan.load();
    let transition_frame = render_scene_state
        .frame_state_mut()
        .reconcile(plan.transition.as_ref(), delta_secs);
    snapshot_scene_runtime(state, scene_snapshot_cache, &plan, transition_frame).await
}

async fn snapshot_scene_runtime(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    plan: &ScenePlanSnapshot,
    transition_frame: Option<TransitionFrame>,
) -> SceneRuntimeSnapshot {
    let active_scene_id = plan.active_scene_id;
    let mut resolved_zones = Arc::clone(&plan.zones);
    let resolved_zones_revision = plan.zones_revision;
    let zone_layout_preview_generation = if let Some(scene_id) = active_scene_id {
        let (generation, overrides) = state
            .zone_layout_previews
            .scene_overrides_with_generation(scene_id)
            .await;
        resolved_zones = apply_zone_layout_previews(resolved_zones, &overrides);
        generation
    } else {
        state.zone_layout_previews.generation()
    };
    let active_scene_name = plan.active_scene_name.clone();
    let unassigned_behavior = plan.unassigned_behavior.clone();
    let device_registry_generation = state.device_registry.generation();
    let (active_display_zone_target_fps, active_display_zone_output_routes) =
        snapshot_display_zone_target_metadata(
            &state.device_registry,
            scene_snapshot_cache,
            resolved_zones_revision,
            resolved_zones.as_ref(),
            state.face_fps_cap,
        )
        .await;
    let active_display_zone_descriptors = display_descriptors_for_zones(
        &active_display_zone_target_fps,
        &active_display_zone_output_routes,
    );
    let active_render_zone_count = u32::try_from(
        resolved_zones
            .iter()
            .filter(|zone| zone_has_enabled_layer(zone))
            .count(),
    )
    .unwrap_or(u32::MAX);
    SceneRuntimeSnapshot {
        active_scene_id,
        active_scene_name,
        active_transition: plan.transition.as_ref().zip(transition_frame).map(
            |(transition, frame)| SceneTransitionSnapshot {
                epoch: transition.epoch,
                from_scene: Some(transition.from_scene),
                to_scene: Some(transition.to_scene),
                progress: frame.progress,
                eased_progress: frame.eased_progress,
                color_interpolation: transition.spec.color_interpolation.clone(),
            },
        ),
        resolved_zones,
        resolved_zones_revision,
        zone_layout_preview_generation,
        active_render_zone_count,
        active_display_zone_target_fps,
        active_display_zone_output_routes,
        active_display_zone_descriptors,
        unassigned_behavior,
        device_registry_generation,
    }
}

pub(super) fn apply_zone_layout_previews(
    resolved_zones: Arc<[Zone]>,
    overrides: &HashMap<ZoneId, SpatialLayout>,
) -> Arc<[Zone]> {
    if overrides.is_empty() {
        return resolved_zones;
    }

    let mut changed = false;
    let zones = resolved_zones
        .iter()
        .cloned()
        .map(|mut zone| {
            if let Some(layout) = overrides.get(&zone.id)
                && zone.layout != *layout
            {
                zone.layout = layout.clone();
                changed = true;
            }
            zone
        })
        .collect::<Vec<_>>();

    if changed {
        zones.into()
    } else {
        resolved_zones
    }
}

fn combine_scene_dependency_generation(
    dependency_generation: u64,
    device_registry_generation: u64,
    zone_layout_preview_generation: u64,
    unassigned_behavior: &UnassignedBehavior,
) -> u64 {
    dependency_generation
        ^ device_registry_generation.rotate_left(21)
        ^ zone_layout_preview_generation.rotate_left(37)
        ^ unassigned_behavior_generation(unassigned_behavior).rotate_left(42)
}

pub(super) fn scene_dependency_key(
    resolved_zones_revision: u64,
    registry_generation: u64,
    device_registry_generation: u64,
    zone_layout_preview_generation: u64,
    unassigned_behavior: &UnassignedBehavior,
) -> SceneDependencyKey {
    SceneDependencyKey::new(
        resolved_zones_revision,
        combine_scene_dependency_generation(
            registry_generation,
            device_registry_generation,
            zone_layout_preview_generation,
            unassigned_behavior,
        ),
    )
}

fn unassigned_behavior_generation(unassigned_behavior: &UnassignedBehavior) -> u64 {
    match unassigned_behavior {
        UnassignedBehavior::Off => 0,
        UnassignedBehavior::Hold => 1,
        UnassignedBehavior::Fallback(zone_id) => {
            let raw = zone_id.0.as_u128();
            2 ^ ((raw >> 64) as u64) ^ (raw as u64)
        }
    }
}

pub(super) async fn snapshot_display_zone_target_metadata(
    device_registry: &hypercolor_core::device::DeviceRegistry,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    zones_revision: u64,
    zones: &[Zone],
    face_fps_cap: u32,
) -> (
    HashMap<ZoneId, u32>,
    HashMap<ZoneId, DisplayZoneOutputRoute>,
) {
    let dependency_key = SceneDependencyKey::new(zones_revision, device_registry.generation());
    if let Some(cached) = scene_snapshot_cache.cached_display_zone_target_metadata(dependency_key) {
        return cached;
    }

    let display_devices = device_registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.state.is_renderable())
        .map(|tracked| {
            (
                tracked.info.id,
                (
                    tracked.info.capabilities.max_fps,
                    display_zone_output_route_for_device(
                        &tracked.info,
                        tracked.user_settings.brightness,
                    ),
                ),
            )
        })
        .collect::<HashMap<DeviceId, (u32, Option<DisplayZoneOutputRoute>)>>();

    let target_fps = zones
        .iter()
        .filter_map(|zone| {
            let target = zone.display_target.as_ref()?;
            let device_max_fps = display_devices
                .get(&target.device_id)
                .map(|(max_fps, _)| *max_fps)
                .unwrap_or(0);
            Some((
                zone.id,
                capped_zone_direct_display_target_fps(device_max_fps, face_fps_cap),
            ))
        })
        .collect();
    let output_routes = zones
        .iter()
        .filter_map(|zone| {
            let target = zone.display_target.as_ref()?;
            let route = display_devices.get(&target.device_id)?.1.clone()?;
            Some((zone.id, route))
        })
        .collect();
    scene_snapshot_cache.cache_display_zone_target_metadata(
        dependency_key,
        &target_fps,
        &output_routes,
    );
    (target_fps, output_routes)
}

fn display_zone_output_route_for_device(
    info: &DeviceInfo,
    brightness: f32,
) -> Option<DisplayZoneOutputRoute> {
    let surface = info.display_surface()?;

    Some(DisplayZoneOutputRoute {
        device_id: info.id,
        width: surface.width,
        height: surface.height,
        circular: surface.circular,
        brightness: brightness.clamp(0.0, 1.0),
        frame_format: surface.format,
        viewport: default_display_zone_viewport(),
    })
}

/// Build the per-zone display descriptors handed to face renderers at
/// load. Pixel format maps the transport: raw RGB stays `Rgb`; JPEG
/// transports apply 4:2:0 chroma subsampling, surfaced as `Yuv420` so
/// face authors can avoid one-pixel colored hairlines.
pub(super) fn display_descriptors_for_zones(
    target_fps: &HashMap<ZoneId, u32>,
    routes: &HashMap<ZoneId, DisplayZoneOutputRoute>,
) -> HashMap<ZoneId, DisplayDescriptor> {
    routes
        .iter()
        .map(|(zone_id, route)| {
            let fps = target_fps
                .get(zone_id)
                .copied()
                .unwrap_or(DISPLAY_FACE_DEFAULT_FPS);
            let pixel_format = DisplayPixelFormat::from(route.frame_format);
            (
                *zone_id,
                DisplayDescriptor::derive(
                    route.width,
                    route.height,
                    route.circular,
                    None,
                    fps,
                    pixel_format,
                ),
            )
        })
        .collect()
}

fn default_display_zone_viewport() -> DisplayZoneViewport {
    DisplayZoneViewport {
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        edge_behavior: EdgeBehavior::Clamp,
    }
}

async fn current_effect_scene_snapshot(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    scene_runtime: &SceneRuntimeSnapshot,
    screen_capture_configured: bool,
) -> EffectSceneSnapshot {
    let registry = state.effect_registry.read().await;
    let dependency_key = scene_runtime.dependency_key(registry.generation());
    if let Some(demand) =
        scene_snapshot_cache.cached_effect_demand(dependency_key, screen_capture_configured)
    {
        return EffectSceneSnapshot {
            demand,
            dependency_key,
        };
    }

    let mut effect_running = false;
    let mut audio_capture_active = false;
    let mut screen_capture_active = false;
    let mut interaction_capture_active = false;
    let mut media_input_active = false;
    let mut network_input_active = false;
    let mut sensor_input_active = false;

    for zone in scene_runtime.resolved_zones.iter() {
        if !zone.enabled {
            continue;
        }

        for layer in &zone.layers {
            if !layer.enabled {
                continue;
            }
            effect_running = true;
            sensor_input_active |= layer
                .bindings
                .iter()
                .any(|binding| matches!(&binding.source, BindingSource::Sensor { .. }));

            match &layer.source {
                LayerSource::Effect {
                    effect_id,
                    control_bindings,
                    ..
                } => {
                    sensor_input_active |= !control_bindings.is_empty();
                    if let Some(entry) = registry.get(effect_id) {
                        audio_capture_active |= entry.metadata.audio_reactive;
                        screen_capture_active |= entry.metadata.screen_reactive;
                        interaction_capture_active |= entry.metadata.requires_interaction();
                        media_input_active |= effect_has_tag(&entry.metadata.tags, "media");
                        network_input_active |= effect_has_tag(&entry.metadata.tags, "net");
                        sensor_input_active |= entry.metadata.requires_sensors();
                    }
                }
                LayerSource::ScreenRegion { .. } => {
                    screen_capture_active |= screen_capture_configured;
                }
                LayerSource::Media { .. }
                | LayerSource::WebViewport { .. }
                | LayerSource::ColorFill { .. } => {}
            }
        }
    }

    let demand = EffectDemand {
        effect_running,
        audio_capture_active,
        screen_capture_active,
        interaction_capture_active,
        media_input_active,
        network_input_active,
        sensor_input_active,
    };
    scene_snapshot_cache.cache_effect_demand(dependency_key, screen_capture_configured, demand);

    EffectSceneSnapshot {
        demand,
        dependency_key,
    }
}

fn effect_has_tag(tags: &[String], expected: &str) -> bool {
    tags.iter().any(|tag| tag.eq_ignore_ascii_case(expected))
}

fn zone_has_enabled_layer(zone: &Zone) -> bool {
    zone.enabled && zone.layers.iter().any(|layer| layer.enabled)
}

async fn render_loop_snapshot(state: &RenderThreadState) -> RenderLoopSnapshot {
    let render_loop = state.render_loop.read().await;
    RenderLoopSnapshot {
        frame_token: render_loop.frame_number(),
        elapsed_ms: super::millis_u64(render_loop.elapsed()),
        budget_us: super::micros_u32(render_loop.target_interval()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::sync::{Mutex, RwLock, watch};
    use uuid::Uuid;

    use hypercolor_core::asset::AssetLibrary;
    use hypercolor_core::bus::{DisplayZoneOutputRoute, HypercolorBus};
    use hypercolor_core::device::{BackendManager, DeviceRegistry};
    use hypercolor_core::effect::{EffectEntry, EffectRegistry};
    use hypercolor_core::engine::{FpsTier, RenderLoop};
    use hypercolor_core::input::InputManager;
    use hypercolor_core::scene::SceneManager;
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
        DeviceFeatures, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
        DisplayFrameFormat, SegmentInfo,
    };
    use hypercolor_types::effect::{
        ControlBinding, EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
    };
    use hypercolor_types::layer::{
        BindingMap, BindingSource, BlendMode, LayerAdjust, LayerBinding, LayerParameter,
        LayerSource, LayerTransform, SceneLayer, SceneLayerId,
    };
    use hypercolor_types::scene::{DisplayFaceTarget, UnassignedBehavior, Zone, ZoneId, ZoneRole};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
    use hypercolor_types::viewport::ViewportRect;

    use super::{default_display_zone_viewport, display_descriptors_for_zones};
    use crate::display_output::DISPLAY_FACE_DEFAULT_FPS;
    use crate::output_power::OutputPowerState;
    use crate::performance::PerformanceTracker;
    use crate::preview_runtime::PreviewRuntime;
    use crate::render_thread::{CanvasDims, RenderThreadState};
    use crate::scene_transactions::SceneTransactionQueue;
    use crate::zone_layout_preview::ZoneLayoutPreviewOwner;
    use hypercolor_types::display::DisplayPixelFormat;

    use super::{
        EffectDemand, FrameSceneSnapshot, SceneDependencyKey, SceneRuntimeSnapshot,
        SceneSnapshotCache, build_frame_scene_snapshot, current_effect_scene_snapshot,
        refresh_effect_scene_snapshot, render_loop_snapshot, snapshot_display_zone_target_metadata,
        snapshot_scene_runtime,
    };
    use crate::render_thread::scene_state::RenderSceneState;

    #[test]
    fn scene_snapshot_cache_caches_display_zone_target_metadata_by_dependency_key() {
        let mut scheduler = SceneSnapshotCache::new();
        let zone_id = ZoneId::new();
        let target_fps = std::collections::HashMap::from([(zone_id, 30)]);
        let output_routes = std::collections::HashMap::new();
        let dependency_key = SceneDependencyKey::new(1, 7);

        assert!(
            scheduler
                .cached_display_zone_target_metadata(dependency_key)
                .is_none()
        );

        scheduler.cache_display_zone_target_metadata(dependency_key, &target_fps, &output_routes);

        assert_eq!(
            scheduler.cached_display_zone_target_metadata(dependency_key),
            Some((target_fps.clone(), output_routes.clone()))
        );
        assert!(
            scheduler
                .cached_display_zone_target_metadata(SceneDependencyKey::new(2, 7))
                .is_none()
        );
        assert!(
            scheduler
                .cached_display_zone_target_metadata(SceneDependencyKey::new(1, 8))
                .is_none()
        );
    }

    #[test]
    fn scene_snapshot_cache_caches_effect_demand_by_dependency_key_and_capture_mode() {
        let mut scheduler = SceneSnapshotCache::new();
        let dependency_key = SceneDependencyKey::new(1, 7);
        let demand = EffectDemand {
            effect_running: true,
            audio_capture_active: true,
            screen_capture_active: false,
            interaction_capture_active: false,
            media_input_active: false,
            network_input_active: false,
            sensor_input_active: false,
        };

        assert!(
            scheduler
                .cached_effect_demand(dependency_key, false)
                .is_none()
        );

        scheduler.cache_effect_demand(dependency_key, false, demand);

        assert_eq!(
            scheduler.cached_effect_demand(dependency_key, false),
            Some(demand)
        );
        assert!(
            scheduler
                .cached_effect_demand(SceneDependencyKey::new(2, 7), false)
                .is_none()
        );
        assert!(
            scheduler
                .cached_effect_demand(SceneDependencyKey::new(1, 8), false)
                .is_none()
        );
        assert!(
            scheduler
                .cached_effect_demand(dependency_key, true)
                .is_none()
        );
    }

    fn sample_layout() -> SpatialLayout {
        SpatialLayout {
            id: "frame-state-test".into(),
            name: "Frame State Test".into(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    fn sample_entry(id: EffectId, audio_reactive: bool, screen_reactive: bool) -> EffectEntry {
        EffectEntry {
            metadata: EffectMetadata {
                id,
                name: "test-effect".into(),
                author: "test".into(),
                version: "0.1.0".into(),
                description: "test effect".into(),
                category: EffectCategory::Ambient,
                tags: Vec::new(),
                controls: Vec::new(),
                presets: Vec::new(),
                audio_reactive,
                screen_reactive,
                input_reactive: false,
                source: EffectSource::Native {
                    path: PathBuf::from("native/test-effect.wgsl"),
                },
                license: None,
            },
            source_path: PathBuf::from("/effects/native/test-effect.wgsl"),
            modified: std::time::SystemTime::now(),
            state: EffectState::Loading,
        }
    }

    fn sample_zone(effect_id: EffectId) -> Zone {
        Zone {
            id: ZoneId::new(),
            name: "Test Zone".into(),
            description: None,
            layers: vec![SceneLayer::from_effect(
                SceneLayerId::new(),
                effect_id,
                HashMap::new(),
                HashMap::new(),
                None,
            )],
            layout: sample_layout(),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        }
    }

    fn sample_control_binding() -> ControlBinding {
        ControlBinding {
            sensor: "cpu.temperature".into(),
            sensor_min: 20.0,
            sensor_max: 100.0,
            target_min: 0.0,
            target_max: 1.0,
            deadband: 0.0,
            smoothing: 0.0,
        }
    }

    async fn sensor_demand_for(entry: EffectEntry, zone: Zone) -> bool {
        let mut registry = EffectRegistry::default();
        registry.register(entry);
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_scene_name: None,
            active_transition: None,
            resolved_zones: vec![zone].into(),
            resolved_zones_revision: 7,
            zone_layout_preview_generation: 0,
            active_render_zone_count: 1,
            active_display_zone_target_fps: HashMap::new(),
            active_display_zone_output_routes: HashMap::new(),
            active_display_zone_descriptors: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let mut scene_snapshot_cache = SceneSnapshotCache::new();

        current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, false)
            .await
            .demand
            .sensor_input_active
    }

    fn sample_display_device_info(device_id: DeviceId) -> DeviceInfo {
        DeviceInfo {
            id: device_id,
            name: "Pump LCD".into(),
            vendor: "test-vendor".into(),
            family: DeviceFamily::new_static("corsair", "Corsair"),
            model: Some("LCD".into()),
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native("corsair", "usb", ConnectionType::Usb),
            segments: vec![SegmentInfo {
                name: "LCD".into(),
                led_count: 320 * 320,
                topology: DeviceTopologyHint::Display {
                    width: 320,
                    height: 320,
                    circular: true,
                    format: DisplayFrameFormat::Rgb,
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: Some("0.1.0".into()),
            capabilities: DeviceCapabilities {
                led_count: 320 * 320,
                supports_direct: true,
                supports_brightness: true,
                has_display: true,
                display_resolution: Some((320, 320)),
                max_fps: 30,
                color_space: DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        }
    }

    fn minimal_render_thread_state(registry: EffectRegistry) -> RenderThreadState {
        let (_, power_state) = watch::channel(OutputPowerState::default());
        let event_bus = Arc::new(HypercolorBus::new());
        let scene_manager = crate::domain::scene::SceneService::with_temporary_store(
            SceneManager::with_default(),
            Arc::clone(&event_bus),
        )
        .expect("temporary scene store should open");
        let scene_plan = scene_manager.plan_reader();
        let asset_tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
        let asset_dir = asset_tempdir.path().join("assets");
        RenderThreadState {
            effect_registry: Arc::new(RwLock::new(registry)),
            asset_library: Arc::new(RwLock::new(
                AssetLibrary::open(asset_dir).expect("test asset library should open"),
            )),
            spatial_engine: crate::domain::spatial::SpatialService::new(SpatialEngine::new(
                sample_layout(),
            )),
            backend_manager: Arc::new(Mutex::new(BackendManager::new())),
            device_registry: DeviceRegistry::new(),
            performance: Arc::new(RwLock::new(PerformanceTracker::default())),
            discovery_runtime: None,
            event_bus: Arc::clone(&event_bus),
            preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
            zone_layout_previews: Arc::new(
                crate::zone_layout_preview::ZoneLayoutPreviewStore::default(),
            ),
            render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
            scene_manager,
            scene_plan,
            input_manager: InputManager::new(),
            interaction_routing: crate::interaction_routing::InteractionRoutingControl::default(),
            power_state,
            scene_transactions: SceneTransactionQueue::default(),
            screen_capture_configured: false,
            canvas_dims: CanvasDims::new(320, 200),
            render_acceleration_mode: RenderAccelerationMode::Cpu,
            #[cfg(feature = "wgpu")]
            render_gpu_device: None,
            configured_max_fps_tier: FpsTier::Full.into(),
            face_fps_cap: 30,
        }
    }

    #[tokio::test]
    async fn build_frame_scene_snapshot_carries_render_loop_and_scene_state_values() {
        let state = minimal_render_thread_state(EffectRegistry::default());
        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let mut render_scene_state = RenderSceneState::new(
            SpatialEngine::new(SpatialLayout {
                canvas_width: 512,
                canvas_height: 288,
                ..sample_layout()
            }),
            false,
        );
        let expected_loop_snapshot = render_loop_snapshot(&state).await;

        let snapshot = build_frame_scene_snapshot(
            &state,
            &mut scene_snapshot_cache,
            &mut render_scene_state,
            0.0,
        )
        .await;

        assert_eq!(snapshot.frame_token, expected_loop_snapshot.frame_token);
        assert_eq!(snapshot.elapsed_ms, expected_loop_snapshot.elapsed_ms);
        assert_eq!(snapshot.budget_us, expected_loop_snapshot.budget_us);
        assert_eq!(snapshot.output_power, OutputPowerState::default());
        assert!(!snapshot.effect_demand.effect_running);
        assert_eq!(snapshot.spatial_engine.layout().canvas_width, 512);
        assert_eq!(snapshot.spatial_engine.layout().canvas_height, 288);
    }

    #[tokio::test]
    async fn configured_capture_without_a_consumer_stays_idle() {
        let state = minimal_render_thread_state(EffectRegistry::default());
        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let mut render_scene_state =
            RenderSceneState::new(SpatialEngine::new(sample_layout()), true);

        let snapshot = build_frame_scene_snapshot(
            &state,
            &mut scene_snapshot_cache,
            &mut render_scene_state,
            0.0,
        )
        .await;

        assert!(!snapshot.effect_demand.effect_running);
        assert!(!snapshot.effect_demand.screen_capture_active);
    }

    #[tokio::test]
    async fn scene_runtime_snapshot_builds_display_output_routes_from_renderable_devices() {
        let state = minimal_render_thread_state(EffectRegistry::default());
        let device_id = DeviceId::new();
        let registered_id = state
            .device_registry
            .add(sample_display_device_info(device_id))
            .await;
        assert_eq!(registered_id, device_id);
        assert!(
            state
                .device_registry
                .set_state(&device_id, DeviceState::Connected)
                .await
        );

        let mut zone = sample_zone(EffectId::from(Uuid::now_v7()));
        zone.role = ZoneRole::Display;
        zone.display_target = Some(DisplayFaceTarget::new(device_id));
        let zone_id = zone.id;

        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let (target_fps, output_routes) = snapshot_display_zone_target_metadata(
            &state.device_registry,
            &mut scene_snapshot_cache,
            11,
            &[zone],
            30,
        )
        .await;

        assert_eq!(target_fps.get(&zone_id), Some(&30));
        let route = output_routes
            .get(&zone_id)
            .expect("display zone should get a fallback output route");
        assert_eq!(route.device_id, device_id);
        assert_eq!(route.width, 320);
        assert_eq!(route.height, 320);
        assert!(route.circular);
        assert_eq!(route.frame_format, DisplayFrameFormat::Rgb);
        assert!((route.brightness - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn display_descriptors_map_routes_to_shared_derivation() {
        let zone_id = ZoneId::new();
        let route = DisplayZoneOutputRoute {
            device_id: DeviceId::new(),
            width: 960,
            height: 160,
            circular: false,
            brightness: 1.0,
            frame_format: DisplayFrameFormat::Rgb,
            viewport: default_display_zone_viewport(),
        };
        let mut routes = HashMap::new();
        routes.insert(zone_id, route);
        let mut fps = HashMap::new();
        fps.insert(zone_id, 60);

        let descriptors = display_descriptors_for_zones(&fps, &routes);
        let descriptor = descriptors
            .get(&zone_id)
            .expect("display route should yield a descriptor");

        assert_eq!(
            descriptor.shape,
            hypercolor_types::display::DisplayShape::Wide
        );
        assert_eq!(descriptor.target_fps, 60);
        assert_eq!(descriptor.pixel_format, DisplayPixelFormat::Rgb);
        assert_eq!(descriptor.safe_area.width, 960);
    }

    #[test]
    fn display_descriptors_mark_jpeg_routes_as_chroma_subsampled() {
        let zone_id = ZoneId::new();
        let route = DisplayZoneOutputRoute {
            device_id: DeviceId::new(),
            width: 480,
            height: 480,
            circular: true,
            brightness: 1.0,
            frame_format: DisplayFrameFormat::Jpeg,
            viewport: default_display_zone_viewport(),
        };
        let mut routes = HashMap::new();
        routes.insert(zone_id, route);

        let descriptors = display_descriptors_for_zones(&HashMap::new(), &routes);
        let descriptor = descriptors
            .get(&zone_id)
            .expect("display route should yield a descriptor");

        assert_eq!(descriptor.pixel_format, DisplayPixelFormat::Yuv420);
        assert_eq!(descriptor.target_fps, DISPLAY_FACE_DEFAULT_FPS);
        assert_eq!(
            descriptor.shape,
            hypercolor_types::display::DisplayShape::Round
        );
        assert_eq!(descriptor.safe_area.width, 339);
    }

    #[tokio::test]
    async fn scene_runtime_snapshot_applies_zone_layout_preview_overrides() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let entry = sample_entry(effect_id, false, false);
        let metadata = entry.metadata.clone();
        let mut registry = EffectRegistry::default();
        registry.register(entry);
        let state = minimal_render_thread_state(registry);
        let mut preview_layout = sample_layout();
        preview_layout.canvas_width = 640;
        preview_layout.canvas_height = 360;

        let (scene_id, zone_id) = {
            let mut mutation = state.scene_manager.begin_mutation().await;
            let zone = mutation
                .upsert_primary_zone(
                    &metadata,
                    HashMap::new(),
                    None,
                    sample_layout(),
                    hypercolor_types::event::ChangeTrigger::System,
                    None,
                )
                .expect("test scene should accept a primary zone");
            let scene = mutation
                .scenes()
                .active_scene()
                .expect("default scene should be active");
            let scene_id = scene.id;
            let zone_id = zone.id;
            state
                .scene_manager
                .commit_mutation(mutation)
                .await
                .expect("test scene should commit");
            (scene_id, zone_id)
        };
        state
            .zone_layout_previews
            .set(
                ZoneLayoutPreviewOwner::new(),
                scene_id,
                zone_id,
                preview_layout.clone(),
            )
            .await;

        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let plan = state.scene_plan.load();
        let snapshot = snapshot_scene_runtime(&state, &mut scene_snapshot_cache, &plan, None).await;

        assert_eq!(snapshot.resolved_zones[0].layout, preview_layout);
        assert_eq!(
            snapshot.zone_layout_preview_generation,
            state.zone_layout_previews.generation()
        );
    }

    #[tokio::test]
    async fn effect_scene_snapshot_invalidates_cached_capture_demand_on_registry_generation() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let mut registry = EffectRegistry::default();
        registry.register(sample_entry(effect_id, false, false));
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_scene_name: None,
            active_transition: None,
            resolved_zones: vec![sample_zone(effect_id)].into(),
            resolved_zones_revision: 7,
            zone_layout_preview_generation: 0,
            active_render_zone_count: 1,
            active_display_zone_target_fps: HashMap::new(),
            active_display_zone_output_routes: HashMap::new(),
            active_display_zone_descriptors: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let mut scene_snapshot_cache = SceneSnapshotCache::new();

        let first =
            current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, false)
                .await;
        assert!(!first.demand.audio_capture_active);
        assert!(!first.demand.screen_capture_active);

        {
            let mut registry = state.effect_registry.write().await;
            registry.register(sample_entry(effect_id, true, true));
        }

        let second =
            current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, false)
                .await;
        assert!(second.demand.audio_capture_active);
        assert!(second.demand.screen_capture_active);
        assert!(
            second.dependency_key.dependency_generation
                > first.dependency_key.dependency_generation
        );
    }

    #[tokio::test]
    async fn screen_region_layers_request_screen_capture_when_configured() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let state = minimal_render_thread_state(EffectRegistry::default());
        let mut zone = sample_zone(effect_id);
        zone.layers = vec![SceneLayer {
            id: SceneLayerId::new(),
            name: Some("Screen".into()),
            source: LayerSource::ScreenRegion {
                viewport: ViewportRect::full(),
            },
            blend: BlendMode::Replace,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        }];
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_scene_name: None,
            active_transition: None,
            resolved_zones: vec![zone].into(),
            resolved_zones_revision: 7,
            zone_layout_preview_generation: 0,
            active_render_zone_count: 1,
            active_display_zone_target_fps: HashMap::new(),
            active_display_zone_output_routes: HashMap::new(),
            active_display_zone_descriptors: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let mut scene_snapshot_cache = SceneSnapshotCache::new();

        let snapshot =
            current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, true)
                .await;

        assert!(snapshot.demand.effect_running);
        assert!(!snapshot.demand.audio_capture_active);
        assert!(snapshot.demand.screen_capture_active);
    }

    #[tokio::test]
    async fn effect_metadata_creates_typed_input_demand() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let mut entry = sample_entry(effect_id, false, false);
        entry.metadata.input_reactive = true;
        entry.metadata.tags = vec!["MEDIA".to_owned(), "net".to_owned()];
        let mut registry = EffectRegistry::default();
        registry.register(entry);
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_scene_name: None,
            active_transition: None,
            resolved_zones: vec![sample_zone(effect_id)].into(),
            resolved_zones_revision: 7,
            zone_layout_preview_generation: 0,
            active_render_zone_count: 1,
            active_display_zone_target_fps: HashMap::new(),
            active_display_zone_output_routes: HashMap::new(),
            active_display_zone_descriptors: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let mut scene_snapshot_cache = SceneSnapshotCache::new();

        let snapshot =
            current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, false)
                .await;

        assert!(snapshot.demand.effect_running);
        assert!(snapshot.demand.interaction_capture_active);
        assert!(snapshot.demand.media_input_active);
        assert!(snapshot.demand.network_input_active);
        assert!(!snapshot.demand.audio_capture_active);
    }

    #[tokio::test]
    async fn sensor_demand_covers_metadata_control_and_layer_bindings() {
        let metadata_effect_id = EffectId::from(Uuid::now_v7());
        let mut metadata_entry = sample_entry(metadata_effect_id, false, false);
        metadata_entry.metadata.tags = vec!["SENSORS".into()];
        assert!(
            sensor_demand_for(metadata_entry, sample_zone(metadata_effect_id)).await,
            "sensor-aware metadata should request sensor publication"
        );

        let control_effect_id = EffectId::from(Uuid::now_v7());
        let mut control_zone = sample_zone(control_effect_id);
        let LayerSource::Effect {
            control_bindings, ..
        } = &mut control_zone.layers[0].source
        else {
            panic!("sample zone should lead with an effect layer");
        };
        control_bindings.insert("intensity".into(), sample_control_binding());
        assert!(
            sensor_demand_for(sample_entry(control_effect_id, false, false), control_zone,).await,
            "effect control bindings should request sensor publication"
        );

        let layer_effect_id = EffectId::from(Uuid::now_v7());
        let mut layer_zone = sample_zone(layer_effect_id);
        let mut layer = SceneLayer::from_effect(
            SceneLayerId::new(),
            layer_effect_id,
            HashMap::new(),
            HashMap::new(),
            None,
        );
        layer.bindings.push(LayerBinding {
            target: LayerParameter::Opacity,
            source: BindingSource::Sensor {
                name: "gpu.temperature".into(),
            },
            map: BindingMap::linear(20.0..=100.0, 0.0..=1.0),
        });
        layer_zone.layers = vec![layer];
        assert!(
            sensor_demand_for(sample_entry(layer_effect_id, false, false), layer_zone).await,
            "layer sensor bindings should request sensor publication"
        );
    }

    #[tokio::test]
    async fn refresh_effect_scene_snapshot_picks_up_mid_frame_registry_changes() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let mut registry = EffectRegistry::default();
        registry.register(sample_entry(effect_id, false, false));
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_scene_name: None,
            active_transition: None,
            resolved_zones: vec![sample_zone(effect_id)].into(),
            resolved_zones_revision: 7,
            zone_layout_preview_generation: 0,
            active_render_zone_count: 1,
            active_display_zone_target_fps: HashMap::new(),
            active_display_zone_output_routes: HashMap::new(),
            active_display_zone_descriptors: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::default(),
            device_registry_generation: 0,
        };
        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let render_scene_state = RenderSceneState::new(SpatialEngine::new(sample_layout()), false);
        let effect_scene =
            current_effect_scene_snapshot(&state, &mut scene_snapshot_cache, &scene_runtime, false)
                .await;
        let mut scene_snapshot = FrameSceneSnapshot {
            frame_token: 42,
            elapsed_ms: 123,
            budget_us: 16_666,
            output_power: OutputPowerState::default(),
            effect_demand: effect_scene.demand,
            effect_dependency_key: effect_scene.dependency_key,
            scene_runtime,
            spatial_engine: SpatialEngine::new(sample_layout()),
        };

        {
            let mut registry = state.effect_registry.write().await;
            registry.register(sample_entry(effect_id, true, true));
        }

        let changed = refresh_effect_scene_snapshot(
            &state,
            &mut scene_snapshot_cache,
            &render_scene_state,
            &mut scene_snapshot,
        )
        .await;

        assert!(changed);
        assert!(scene_snapshot.effect_demand.audio_capture_active);
        assert!(scene_snapshot.effect_demand.screen_capture_active);
    }
}
