use std::collections::HashMap;

use tracing::warn;

use hypercolor_core::scene::SceneManager;
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{RenderGroup, RenderGroupId};

use super::RenderThreadState;
use super::frame_scheduler::{
    FrameSceneSnapshot, FrameSceneSnapshotInputs, FrameScheduler, SceneRuntimeSnapshot,
    SceneDependencyKey, SceneTransitionSnapshot,
};
use super::scene_state::RenderSceneState;
use crate::display_output::capped_group_direct_display_target_fps;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderLoopSnapshot {
    pub(crate) frame_token: u64,
    pub(crate) elapsed_ms: u32,
    pub(crate) budget_us: u32,
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

impl EffectSceneSnapshot {
    pub(crate) const fn registry_generation(self) -> u64 {
        self.dependency_key.dependency_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CachedRenderGroupDemand {
    pub(crate) dependency_key: SceneDependencyKey,
    pub(crate) screen_capture_configured: bool,
    pub(crate) demand: EffectDemand,
}

pub(crate) async fn build_frame_scene_snapshot(
    state: &RenderThreadState,
    frame_scheduler: &mut FrameScheduler,
    render_scene_state: &RenderSceneState,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
    delta_secs: f32,
) -> FrameSceneSnapshot {
    let scene_runtime = current_scene_runtime_snapshot(state, frame_scheduler, delta_secs).await;
    let effect_scene = current_effect_scene_snapshot(
        state,
        &scene_runtime,
        last_render_group_demand,
        render_scene_state.screen_capture_configured(),
    )
    .await;
    let render_loop_snapshot = render_loop_snapshot(state).await;
    frame_scheduler.build_snapshot(FrameSceneSnapshotInputs {
        frame_token: render_loop_snapshot.frame_token,
        elapsed_ms: render_loop_snapshot.elapsed_ms,
        budget_us: render_loop_snapshot.budget_us,
        output_power: *state.power_state.borrow(),
        effect_demand: effect_scene.demand,
        effect_registry_generation: effect_scene.registry_generation(),
        scene_runtime,
        spatial_engine: render_scene_state.spatial_engine().clone(),
    })
}

pub(crate) async fn refresh_effect_scene_snapshot(
    state: &RenderThreadState,
    render_scene_state: &RenderSceneState,
    scene_snapshot: &mut FrameSceneSnapshot,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
) -> bool {
    let refreshed = current_effect_scene_snapshot(
        state,
        &scene_snapshot.scene_runtime,
        last_render_group_demand,
        render_scene_state.screen_capture_configured(),
    )
    .await;
    let changed = refreshed.demand != scene_snapshot.effect_demand
        || refreshed.registry_generation() != scene_snapshot.effect_registry_generation;
    scene_snapshot.effect_demand = refreshed.demand;
    scene_snapshot.effect_registry_generation = refreshed.registry_generation();
    changed
}

async fn current_scene_runtime_snapshot(
    state: &RenderThreadState,
    frame_scheduler: &mut FrameScheduler,
    delta_secs: f32,
) -> SceneRuntimeSnapshot {
    let transitioning = {
        let manager = state.scene_manager.read().await;
        manager.is_transitioning()
    };

    if transitioning {
        let mut manager = state.scene_manager.write().await;
        manager.tick_transition(delta_secs);
        return snapshot_scene_runtime(state, frame_scheduler, &manager).await;
    }

    let manager = state.scene_manager.read().await;
    snapshot_scene_runtime(state, frame_scheduler, &manager).await
}

async fn snapshot_scene_runtime(
    state: &RenderThreadState,
    frame_scheduler: &mut FrameScheduler,
    manager: &SceneManager,
) -> SceneRuntimeSnapshot {
    let active_render_groups = manager.active_render_groups();
    let active_render_groups_revision = manager.active_render_groups_revision();
    let active_display_group_target_fps = snapshot_display_group_target_fps(
        &state.device_registry,
        frame_scheduler,
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
    frame_scheduler: &mut FrameScheduler,
    groups_revision: u64,
    groups: &[RenderGroup],
) -> HashMap<RenderGroupId, u32> {
    let dependency_key = SceneDependencyKey::new(groups_revision, device_registry.generation());
    if let Some(cached) = frame_scheduler.cached_display_group_target_fps(dependency_key) {
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
    frame_scheduler.cache_display_group_target_fps(dependency_key, &target_fps);
    target_fps
}

async fn current_effect_scene_snapshot(
    state: &RenderThreadState,
    scene_runtime: &SceneRuntimeSnapshot,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
    screen_capture_configured: bool,
) -> EffectSceneSnapshot {
    let registry = state.effect_registry.read().await;
    let dependency_key = scene_runtime.dependency_key(registry.generation());
    if let Some(cached) = last_render_group_demand.as_ref()
        && cached.dependency_key == dependency_key
        && cached.screen_capture_configured == screen_capture_configured
    {
        return EffectSceneSnapshot {
            demand: cached.demand,
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
    *last_render_group_demand = Some(CachedRenderGroupDemand {
        dependency_key,
        screen_capture_configured,
        demand,
    });

    EffectSceneSnapshot {
        demand,
        dependency_key,
    }
}

pub(crate) async fn reconcile_audio_capture(
    state: &RenderThreadState,
    desired_active: bool,
    last_audio_capture_active: &mut Option<bool>,
) {
    if last_audio_capture_active.is_some_and(|previous| previous == desired_active) {
        return;
    }

    let result = {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.set_audio_capture_active(desired_active)
    };

    match result {
        Ok(()) => {
            *last_audio_capture_active = Some(desired_active);
        }
        Err(error) => {
            warn!(
                desired_active,
                %error,
                "Failed to update audio capture demand"
            );
        }
    }
}

pub(crate) async fn reconcile_screen_capture(
    state: &RenderThreadState,
    desired_active: bool,
    last_screen_capture_active: &mut Option<bool>,
) {
    if last_screen_capture_active.is_some_and(|previous| previous == desired_active) {
        return;
    }

    let result = {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.set_screen_capture_active(desired_active)
    };

    match result {
        Ok(()) => {
            *last_screen_capture_active = Some(desired_active);
        }
        Err(error) => {
            warn!(
                desired_active,
                %error,
                "Failed to update screen capture demand"
            );
        }
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
    use hypercolor_types::scene::{RenderGroup, RenderGroupId, RenderGroupRole};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use crate::device_settings::DeviceSettingsStore;
    use crate::performance::PerformanceTracker;
    use crate::preview_runtime::PreviewRuntime;
    use crate::render_thread::{CanvasDims, RenderThreadState};
    use crate::scene_transactions::SceneTransactionQueue;
    use crate::session::OutputPowerState;

    use super::{
        CachedRenderGroupDemand, current_effect_scene_snapshot, refresh_effect_scene_snapshot,
    };
    use crate::render_thread::frame_scheduler::{
        FrameSceneSnapshotInputs, SceneRuntimeSnapshot,
    };
    use crate::render_thread::scene_state::RenderSceneState;

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
            configured_max_fps_tier: FpsTier::Full,
        }
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
        let mut cached = None::<CachedRenderGroupDemand>;

        let first = current_effect_scene_snapshot(&state, &scene_runtime, &mut cached, false).await;
        assert!(!first.demand.audio_capture_active);
        assert!(!first.demand.screen_capture_active);

        {
            let mut registry = state.effect_registry.write().await;
            registry.register(sample_entry(effect_id, true, true));
        }

        let second =
            current_effect_scene_snapshot(&state, &scene_runtime, &mut cached, false).await;
        assert!(second.demand.audio_capture_active);
        assert!(second.demand.screen_capture_active);
        assert!(second.registry_generation() > first.registry_generation());
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
        let mut cached = None::<CachedRenderGroupDemand>;
        let mut frame_scheduler = crate::render_thread::frame_scheduler::FrameScheduler::new();
        let render_scene_state = RenderSceneState::new(SpatialEngine::new(sample_layout()), false);
        let effect_scene =
            current_effect_scene_snapshot(&state, &scene_runtime, &mut cached, false).await;
        let mut scene_snapshot = frame_scheduler.build_snapshot(FrameSceneSnapshotInputs {
            frame_token: 42,
            elapsed_ms: 123,
            budget_us: 16_666,
            output_power: OutputPowerState::default(),
            effect_demand: effect_scene.demand,
            effect_registry_generation: effect_scene.registry_generation(),
            scene_runtime,
            spatial_engine: SpatialEngine::new(sample_layout()),
        });

        {
            let mut registry = state.effect_registry.write().await;
            registry.register(sample_entry(effect_id, true, true));
        }

        let changed = refresh_effect_scene_snapshot(
            &state,
            &render_scene_state,
            &mut scene_snapshot,
            &mut cached,
        )
        .await;

        assert!(changed);
        assert!(scene_snapshot.effect_demand.audio_capture_active);
        assert!(scene_snapshot.effect_demand.screen_capture_active);
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
