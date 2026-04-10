use tracing::warn;

use hypercolor_core::scene::SceneManager;

use super::RenderThreadState;
use super::frame_scheduler::{
    FrameSceneSnapshot, FrameSceneSnapshotInputs, FrameScheduler, SceneRuntimeSnapshot,
    SceneTransitionSnapshot,
};
use super::scene_state::RenderSceneState;

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
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CachedRenderGroupDemand {
    pub(crate) groups_revision: u64,
    pub(crate) demand: EffectDemand,
}

pub(crate) async fn build_frame_scene_snapshot(
    state: &RenderThreadState,
    frame_scheduler: &mut FrameScheduler,
    render_scene_state: &RenderSceneState,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
    delta_secs: f32,
) -> FrameSceneSnapshot {
    let scene_runtime = current_scene_runtime_snapshot(state, delta_secs).await;
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
        effect_generation: effect_scene.generation,
        scene_runtime,
        spatial_engine: render_scene_state.spatial_engine().clone(),
    })
}

async fn current_scene_runtime_snapshot(
    state: &RenderThreadState,
    delta_secs: f32,
) -> SceneRuntimeSnapshot {
    let transitioning = {
        let manager = state.scene_manager.read().await;
        manager.is_transitioning()
    };

    if transitioning {
        let mut manager = state.scene_manager.write().await;
        manager.tick_transition(delta_secs);
        return snapshot_scene_runtime(&manager);
    }

    let manager = state.scene_manager.read().await;
    snapshot_scene_runtime(&manager)
}

fn snapshot_scene_runtime(manager: &SceneManager) -> SceneRuntimeSnapshot {
    SceneRuntimeSnapshot {
        active_scene_id: manager.active_scene_id().copied(),
        active_transition: manager
            .active_transition()
            .map(|transition| SceneTransitionSnapshot {
                from_scene: Some(transition.from_scene),
                to_scene: Some(transition.to_scene),
                progress: transition.progress,
                eased_progress: transition.eased_progress(),
            }),
        active_render_groups: manager.active_render_groups(),
        active_render_groups_revision: manager.active_render_groups_revision(),
    }
}

async fn current_effect_scene_snapshot(
    state: &RenderThreadState,
    scene_runtime: &SceneRuntimeSnapshot,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
    screen_capture_configured: bool,
) -> EffectSceneSnapshot {
    if scene_runtime.has_active_render_groups() {
        return current_render_group_effect_scene_snapshot(
            state,
            scene_runtime,
            last_render_group_demand,
        )
        .await;
    }

    let engine = state.effect_engine.lock().await;
    let effect_running = engine.is_running();
    let audio_capture_active = effect_running
        && engine
            .active_metadata()
            .is_some_and(|meta| meta.audio_reactive);
    let screen_capture_active = (effect_running
        && engine
            .active_metadata()
            .is_some_and(|meta| meta.screen_reactive))
        || (!effect_running && screen_capture_configured);
    EffectSceneSnapshot {
        demand: EffectDemand {
            effect_running,
            audio_capture_active,
            screen_capture_active,
        },
        generation: engine.scene_generation(),
    }
}

async fn current_render_group_effect_scene_snapshot(
    state: &RenderThreadState,
    scene_runtime: &SceneRuntimeSnapshot,
    last_render_group_demand: &mut Option<CachedRenderGroupDemand>,
) -> EffectSceneSnapshot {
    if let Some(cached) = last_render_group_demand.as_ref()
        && cached.groups_revision == scene_runtime.active_render_groups_revision
    {
        return EffectSceneSnapshot {
            demand: cached.demand,
            generation: 0,
        };
    }

    let registry = state.effect_registry.read().await;
    let mut effect_running = false;
    let mut audio_capture_active = false;
    let mut screen_capture_active = false;

    for group in scene_runtime.active_render_groups.iter() {
        if !group.enabled || group.layout.zones.is_empty() {
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

    let demand = EffectDemand {
        effect_running,
        audio_capture_active,
        screen_capture_active,
    };
    *last_render_group_demand = Some(CachedRenderGroupDemand {
        groups_revision: scene_runtime.active_render_groups_revision,
        demand,
    });

    EffectSceneSnapshot {
        demand,
        generation: 0,
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

async fn render_loop_snapshot(state: &RenderThreadState) -> RenderLoopSnapshot {
    let render_loop = state.render_loop.read().await;
    RenderLoopSnapshot {
        frame_token: render_loop.frame_number(),
        elapsed_ms: super::millis_u32(render_loop.elapsed()),
        budget_us: super::micros_u32(render_loop.target_interval()),
    }
}
