//! The actual frame handler must not reacquire authorities held by a restore.

use super::*;
use crate::{
    domain::{scene::SceneService, spatial::SpatialService},
    output_power::OutputPowerState,
    performance::PerformanceTracker,
    preview_runtime::PreviewRuntime,
    render_thread::CanvasDims,
    scene_transactions::{
        LayoutTransactionAuthority, LayoutUpdateError, SceneTransactionQueue,
        portable::{PortableRenderBarrier, PortableRendererSnapshot},
    },
};
use hypercolor_core::{
    asset::AssetLibrary,
    bus::HypercolorBus,
    device::{BackendManager, DeviceRegistry},
    effect::EffectRegistry,
    engine::{FpsTier, RenderLoop},
    input::InputManager,
    scene::SceneManager,
    spatial::SpatialEngine,
};
use hypercolor_types::{
    config::RenderAccelerationMode,
    portable_setup::{PortableScene, SetupBindings},
    spatial::{EdgeBehavior, SamplingMode, SpatialLayout},
};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, watch};

fn layout(width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: "restore-test".into(),
        name: "Restore test".into(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn fixture() -> (tempfile::TempDir, RenderThreadState, Box<PipelineRuntime>) {
    let directory = tempfile::tempdir().expect("private fixture");
    let layout = layout(16, 16);
    let bus = Arc::new(HypercolorBus::new());
    let scenes = SceneService::with_temporary_store(
        SceneManager::with_default_layout(layout.clone()),
        Arc::clone(&bus),
    )
    .expect("scene store");
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let state = RenderThreadState {
        effect_registry: Arc::new(RwLock::new(EffectRegistry::default())),
        asset_library: Arc::new(RwLock::new(
            AssetLibrary::open(directory.path().join("media")).expect("media"),
        )),
        spatial_engine: SpatialService::new(SpatialEngine::new(layout.clone())),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&bus),
        preview_runtime: Arc::new(PreviewRuntime::new(bus)),
        zone_layout_previews: Arc::new(
            crate::zone_layout_preview::ZoneLayoutPreviewStore::default(),
        ),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_plan: scenes.plan_reader(),
        scene_manager: scenes,
        input_manager: InputManager::new(),
        interaction_routing: crate::interaction_routing::InteractionRoutingControl::default(),
        power_state,
        scene_transactions: SceneTransactionQueue::new(),
        screen_capture_configured: false,
        canvas_dims: CanvasDims::new(16, 16),
        render_acceleration_mode: RenderAccelerationMode::Cpu,
        #[cfg(feature = "wgpu")]
        render_gpu_device: None,
        configured_max_fps_tier: FpsTier::Full.into(),
        face_fps_cap: 30,
    };
    let runtime = Box::new(
        PipelineRuntime::new(
            16,
            16,
            SpatialEngine::new(layout),
            false,
            RenderAccelerationMode::Cpu,
            FpsTier::Full,
        )
        .expect("CPU renderer"),
    );
    (directory, state, runtime)
}

fn authority(state: &RenderThreadState) -> LayoutTransactionAuthority {
    LayoutTransactionAuthority::new(
        state.spatial_engine.clone(),
        state.scene_manager.clone(),
        state.scene_transactions.clone(),
    )
}

async fn service(state: &RenderThreadState, runtime: &mut PipelineRuntime) {
    tokio::time::timeout(
        Duration::from_secs(2),
        service_scene_transactions(
            state,
            &mut runtime.scene,
            &mut runtime.frame_loop,
            &mut runtime.render,
        ),
    )
    .await
    .expect("frame handler must not wait on restore-owned writers");
}

async fn begin(
    authority: &LayoutTransactionAuthority,
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
) -> PortableRenderBarrier {
    let mut pending = Box::pin(authority.begin_portable_restore());
    assert!(matches!(
        pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    service(state, runtime).await;
    pending.await.expect("acknowledged frame boundary")
}

#[tokio::test]
async fn portable_renderer_actual_frame_commits_with_scene_effect_and_device_writers_held() {
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    let bindings = SetupBindings::default();
    let snapshot = state.scene_manager.snapshot().await;
    let before: BTreeMap<_, _> = snapshot
        .list()
        .into_iter()
        .map(|scene| {
            (
                scene.id.0,
                PortableScene::project(scene, &bindings).expect("portable scene"),
            )
        })
        .collect();
    let scenes = state
        .scene_manager
        .prepare_portable_replace(
            &before,
            &before,
            &bindings,
            &bindings,
            (16, 16),
            &HashMap::new(),
        )
        .await
        .expect("scene writer");
    let effects = Arc::clone(&state.effect_registry).write_owned().await;
    let _devices = state
        .device_registry
        .prepare_output_settings(&HashMap::new(), HashMap::new())
        .await
        .expect("device writer");
    let candidate = SpatialEngine::new(layout(24, 18));
    let snapshot = Arc::new(PortableRendererSnapshot::new(
        &barrier,
        effects.clone(),
        HashMap::new(),
    ));
    let mut activation = Box::pin(authority.activate_portable_renderer(
        &barrier,
        candidate,
        scenes.candidate(),
        snapshot,
    ));
    assert!(matches!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    service(&state, &mut runtime).await;
    assert!(matches!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    service(&state, &mut runtime).await;
    activation
        .await
        .expect("renderer acknowledged under held writers");
    assert_eq!(
        (state.canvas_dims.width(), state.canvas_dims.height()),
        (24, 18)
    );
    assert_eq!(
        state.spatial_engine.layout().canvas_width,
        16,
        "authority waits for aggregate publication"
    );
    assert!(state.scene_transactions.portable_restore_held());
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        execute_frame(&state, &mut runtime, Instant::now(), SkipDecision::None),
    )
    .await
    .expect("held frame never snapshots guarded authorities");
}

#[tokio::test]
async fn portable_renderer_teardown_completes_an_accepted_activation() {
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    let scenes = state.scene_manager.snapshot().await;
    let snapshot = Arc::new(PortableRendererSnapshot::new(
        &barrier,
        EffectRegistry::default(),
        HashMap::new(),
    ));
    let mut activation = Box::pin(authority.activate_portable_renderer(
        &barrier,
        SpatialEngine::new(layout(24, 18)),
        &scenes,
        snapshot,
    ));
    assert!(matches!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    service(&state, &mut runtime).await;
    assert!(matches!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    state.scene_transactions.close();
    drop(runtime);
    assert!(matches!(
        activation.await,
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::RendererStopped
        ))
    ));
}
