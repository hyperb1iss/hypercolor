use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{ColorInterpolation, RenderGroup, RenderGroupId, SceneId};

use crate::session::OutputPowerState;

use super::RenderThreadState;
use super::scene_dependency::SceneDependencyKey;
use super::scene_state::RenderSceneState;
use crate::display_output::capped_group_direct_display_target_fps;

#[derive(Debug, Clone)]
pub(crate) struct SceneTransitionSnapshot {
    pub from_scene: Option<SceneId>,
    pub to_scene: Option<SceneId>,
    pub progress: f32,
    pub eased_progress: f32,
    pub color_interpolation: ColorInterpolation,
}

impl Default for SceneTransitionSnapshot {
    fn default() -> Self {
        Self {
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
    pub active_transition: Option<SceneTransitionSnapshot>,
    pub active_render_groups: Arc<[RenderGroup]>,
    pub active_render_groups_revision: u64,
    pub active_render_group_count: u32,
    pub active_display_group_target_fps: HashMap<RenderGroupId, u32>,
}

impl SceneRuntimeSnapshot {
    pub(crate) fn active_render_group_count(&self) -> u32 {
        self.active_render_group_count
    }

    pub(crate) const fn dependency_key(&self, dependency_generation: u64) -> SceneDependencyKey {
        SceneDependencyKey::new(self.active_render_groups_revision, dependency_generation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectDemand {
    pub(crate) effect_running: bool,
    pub(crate) audio_capture_active: bool,
    pub(crate) screen_capture_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectSceneSnapshot {
    pub(crate) demand: EffectDemand,
    pub(crate) dependency_key: SceneDependencyKey,
}

#[derive(Debug, Clone)]
pub(crate) struct FrameSceneSnapshot {
    pub frame_token: u64,
    pub elapsed_ms: u32,
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
    pub(crate) elapsed_ms: u32,
    pub(crate) budget_us: u32,
}

#[derive(Debug, Clone, Default)]
struct CachedDisplayGroupTargetFps {
    dependency_key: SceneDependencyKey,
    values: HashMap<RenderGroupId, u32>,
}

#[derive(Debug, Clone, Copy)]
struct CachedEffectDemand {
    dependency_key: SceneDependencyKey,
    screen_capture_configured: bool,
    demand: EffectDemand,
}

#[derive(Debug, Default)]
pub(crate) struct SceneSnapshotCache {
    cached_display_group_target_fps: Option<CachedDisplayGroupTargetFps>,
    cached_effect_demand: Option<CachedEffectDemand>,
}

impl SceneSnapshotCache {
    pub const fn new() -> Self {
        Self {
            cached_display_group_target_fps: None,
            cached_effect_demand: None,
        }
    }

    pub(crate) fn cached_display_group_target_fps(
        &self,
        dependency_key: SceneDependencyKey,
    ) -> Option<HashMap<RenderGroupId, u32>> {
        self.cached_display_group_target_fps
            .as_ref()
            .filter(|cache| cache.dependency_key == dependency_key)
            .map(|cache| cache.values.clone())
    }

    pub(crate) fn cache_display_group_target_fps(
        &mut self,
        dependency_key: SceneDependencyKey,
        values: &HashMap<RenderGroupId, u32>,
    ) {
        self.cached_display_group_target_fps = Some(CachedDisplayGroupTargetFps {
            dependency_key,
            values: values.clone(),
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
    render_scene_state: &RenderSceneState,
    delta_secs: f32,
) -> FrameSceneSnapshot {
    let scene_runtime =
        current_scene_runtime_snapshot(state, scene_snapshot_cache, delta_secs).await;
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
    delta_secs: f32,
) -> SceneRuntimeSnapshot {
    let transitioning = {
        let manager = state.scene_manager.read().await;
        manager.is_transitioning()
    };

    if transitioning {
        let mut manager = state.scene_manager.write().await;
        manager.tick_transition(delta_secs);
        return snapshot_scene_runtime(state, scene_snapshot_cache, &manager).await;
    }

    let manager = state.scene_manager.read().await;
    snapshot_scene_runtime(state, scene_snapshot_cache, &manager).await
}

async fn snapshot_scene_runtime(
    state: &RenderThreadState,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    manager: &SceneManager,
) -> SceneRuntimeSnapshot {
    let active_render_groups = manager.active_render_groups();
    let active_render_groups_revision = manager.active_render_groups_revision();
    let active_display_group_target_fps = snapshot_display_group_target_fps(
        &state.device_registry,
        scene_snapshot_cache,
        active_render_groups_revision,
        active_render_groups.as_ref(),
    )
    .await;
    let active_render_group_count = u32::try_from(
        active_render_groups
            .iter()
            .filter(|group| group.enabled && group.effect_id.is_some())
            .count(),
    )
    .unwrap_or(u32::MAX);
    SceneRuntimeSnapshot {
        active_scene_id: manager.active_scene_id().copied(),
        active_transition: manager
            .active_transition()
            .map(|transition| SceneTransitionSnapshot {
                from_scene: Some(transition.from_scene),
                to_scene: Some(transition.to_scene),
                progress: transition.progress,
                eased_progress: transition.eased_progress(),
                color_interpolation: transition.spec.color_interpolation.clone(),
            }),
        active_render_groups,
        active_render_groups_revision,
        active_render_group_count,
        active_display_group_target_fps,
    }
}

async fn snapshot_display_group_target_fps(
    device_registry: &hypercolor_core::device::DeviceRegistry,
    scene_snapshot_cache: &mut SceneSnapshotCache,
    groups_revision: u64,
    groups: &[RenderGroup],
) -> HashMap<RenderGroupId, u32> {
    let dependency_key = SceneDependencyKey::new(groups_revision, device_registry.generation());
    if let Some(cached) = scene_snapshot_cache.cached_display_group_target_fps(dependency_key) {
        return cached;
    }

    let max_fps_by_device = device_registry
        .list()
        .await
        .into_iter()
        .map(|tracked| (tracked.info.id, tracked.info.capabilities.max_fps))
        .collect::<HashMap<DeviceId, u32>>();

    let target_fps = groups
        .iter()
        .filter_map(|group| {
            let target = group.display_target.as_ref()?;
            let device_max_fps = max_fps_by_device
                .get(&target.device_id)
                .copied()
                .unwrap_or(0);
            Some((
                group.id,
                capped_group_direct_display_target_fps(device_max_fps),
            ))
        })
        .collect();
    scene_snapshot_cache.cache_display_group_target_fps(dependency_key, &target_fps);
    target_fps
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

    for group in scene_runtime.active_render_groups.iter() {
        if !group.enabled {
            continue;
        }

        let Some(effect_id) = group.effect_id else {
            continue;
        };

        effect_running = true;
        if let Some(entry) = registry.get(&effect_id) {
            audio_capture_active |= entry.metadata.audio_reactive;
            screen_capture_active |= entry.metadata.screen_reactive;
        }
    }

    if !effect_running && screen_capture_configured {
        screen_capture_active = true;
    }

    let demand = EffectDemand {
        effect_running,
        audio_capture_active,
        screen_capture_active,
    };
    scene_snapshot_cache.cache_effect_demand(dependency_key, screen_capture_configured, demand);

    EffectSceneSnapshot {
        demand,
        dependency_key,
    }
}

async fn render_loop_snapshot(state: &RenderThreadState) -> RenderLoopSnapshot {
    let render_loop = state.render_loop.read().await;
    RenderLoopSnapshot {
        frame_token: render_loop.frame_number(),
        elapsed_ms: super::millis_u32(render_loop.elapsed()),
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

    use hypercolor_core::bus::HypercolorBus;
    use hypercolor_core::device::{BackendManager, DeviceRegistry};
    use hypercolor_core::effect::{EffectEntry, EffectRegistry};
    use hypercolor_core::engine::{FpsTier, RenderLoop};
    use hypercolor_core::input::InputManager;
    use hypercolor_core::scene::SceneManager;
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::effect::{
        EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
    };
    use hypercolor_types::scene::RenderGroupId;
    use hypercolor_types::scene::{RenderGroup, RenderGroupRole};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use crate::device_settings::DeviceSettingsStore;
    use crate::performance::PerformanceTracker;
    use crate::preview_runtime::PreviewRuntime;
    use crate::render_thread::{CanvasDims, RenderThreadState};
    use crate::scene_transactions::SceneTransactionQueue;
    use crate::session::OutputPowerState;

    use super::{
        EffectDemand, FrameSceneSnapshot, SceneDependencyKey, SceneRuntimeSnapshot,
        SceneSnapshotCache, build_frame_scene_snapshot, current_effect_scene_snapshot,
        refresh_effect_scene_snapshot, render_loop_snapshot,
    };
    use crate::render_thread::scene_state::RenderSceneState;

    #[test]
    fn scene_snapshot_cache_caches_display_group_target_fps_by_revision_and_registry_generation() {
        let mut scheduler = SceneSnapshotCache::new();
        let group_id = RenderGroupId::new();
        let values = std::collections::HashMap::from([(group_id, 30)]);
        let dependency_key = SceneDependencyKey::new(1, 7);

        assert!(
            scheduler
                .cached_display_group_target_fps(dependency_key)
                .is_none()
        );

        scheduler.cache_display_group_target_fps(dependency_key, &values);

        assert_eq!(
            scheduler.cached_display_group_target_fps(dependency_key),
            Some(values.clone())
        );
        assert!(
            scheduler
                .cached_display_group_target_fps(SceneDependencyKey::new(2, 7))
                .is_none()
        );
        assert!(
            scheduler
                .cached_display_group_target_fps(SceneDependencyKey::new(1, 8))
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
            spaces: None,
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

    fn sample_group(effect_id: EffectId) -> RenderGroup {
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Test Group".into(),
            description: None,
            effect_id: Some(effect_id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout(),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
            controls_version: 0,
        }
    }

    fn minimal_render_thread_state(registry: EffectRegistry) -> RenderThreadState {
        let (_, power_state) = watch::channel(OutputPowerState::default());
        let event_bus = Arc::new(HypercolorBus::new());
        RenderThreadState {
            effect_registry: Arc::new(RwLock::new(registry)),
            spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(sample_layout()))),
            backend_manager: Arc::new(Mutex::new(BackendManager::new())),
            device_registry: DeviceRegistry::new(),
            performance: Arc::new(RwLock::new(PerformanceTracker::default())),
            discovery_runtime: None,
            event_bus: Arc::clone(&event_bus),
            preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
            render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
            scene_manager: Arc::new(RwLock::new(SceneManager::with_default())),
            input_manager: Arc::new(Mutex::new(InputManager::new())),
            power_state,
            device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
                "device-settings.json",
            )))),
            scene_transactions: SceneTransactionQueue::default(),
            screen_capture_configured: false,
            canvas_dims: CanvasDims::new(320, 200),
            render_acceleration_mode: RenderAccelerationMode::Cpu,
            #[cfg(feature = "wgpu")]
            render_gpu_device: None,
            configured_max_fps_tier: FpsTier::Full.into(),
        }
    }

    #[tokio::test]
    async fn build_frame_scene_snapshot_carries_render_loop_and_scene_state_values() {
        let state = minimal_render_thread_state(EffectRegistry::default());
        let mut scene_snapshot_cache = SceneSnapshotCache::new();
        let render_scene_state = RenderSceneState::new(
            SpatialEngine::new(SpatialLayout {
                canvas_width: 512,
                canvas_height: 288,
                ..sample_layout()
            }),
            false,
        );
        let expected_loop_snapshot = render_loop_snapshot(&state).await;

        let snapshot =
            build_frame_scene_snapshot(&state, &mut scene_snapshot_cache, &render_scene_state, 0.0)
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
    async fn effect_scene_snapshot_invalidates_cached_capture_demand_on_registry_generation() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let mut registry = EffectRegistry::default();
        registry.register(sample_entry(effect_id, false, false));
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_transition: None,
            active_render_groups: vec![sample_group(effect_id)].into(),
            active_render_groups_revision: 7,
            active_render_group_count: 1,
            active_display_group_target_fps: HashMap::new(),
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
    async fn refresh_effect_scene_snapshot_picks_up_mid_frame_registry_changes() {
        let effect_id = EffectId::from(Uuid::now_v7());
        let mut registry = EffectRegistry::default();
        registry.register(sample_entry(effect_id, false, false));
        let state = minimal_render_thread_state(registry);
        let scene_runtime = SceneRuntimeSnapshot {
            active_scene_id: None,
            active_transition: None,
            active_render_groups: vec![sample_group(effect_id)].into(),
            active_render_groups_revision: 7,
            active_render_group_count: 1,
            active_display_group_target_fps: HashMap::new(),
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
