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

#[derive(Default)]
struct ControlWakes(std::sync::atomic::AtomicUsize);

impl std::task::Wake for ControlWakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl ControlWakes {
    fn take(&self) -> usize {
        self.0.swap(0, std::sync::atomic::Ordering::SeqCst)
    }
}

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

#[tokio::test]
async fn held_pipeline_yields_without_counting_synthetic_frames() {
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    state.render_loop.write().await.start();
    let worker_state = state.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("render executor");
        let _entered = executor.enter();
        let supervision = crate::render_thread::supervision::RenderSupervision::new(
            crate::render_thread::supervision::RenderRestartPolicy::default(),
            None,
        );
        let mut pipeline = Box::pin(crate::render_thread::pipeline_driver::run_pipeline(
            worker_state,
            *runtime,
            supervision,
        ));
        let first = pipeline
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        sent.send(first.is_pending())
            .expect("test awaits first poll");
    });
    let yielded = received.recv_timeout(Duration::from_millis(250));
    let frames = state.render_loop.read().await.frame_number();
    // Also release the old busy-loop implementation before asserting, so a
    // failing regression cannot leave an OS render thread behind.
    state.scene_transactions.close();
    state.render_loop.write().await.stop();
    drop(barrier);
    worker.join().expect("render worker exits");
    assert!(yielded.expect("held renderer must yield its first poll"));
    assert_eq!(frames, 0, "waiting for persistence is not a rendered frame");
}

#[tokio::test]
async fn held_renderer_wakes_for_queued_work_commit_and_release() {
    let wakes = Arc::new(ControlWakes::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    let supervision = crate::render_thread::supervision::RenderSupervision::new(
        crate::render_thread::supervision::RenderRestartPolicy::default(),
        None,
    );
    let mut waiting = Box::pin(
        crate::render_thread::pipeline_driver::wait_for_scene_control(
            &state,
            &runtime,
            &supervision,
        ),
    );
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
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
    assert!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert!(wakes.take() > 0, "queued work must wake the renderer");
    waiting.await;
    service(&state, &mut runtime).await;
    let mut waiting = Box::pin(
        crate::render_thread::pipeline_driver::wait_for_scene_control(
            &state,
            &runtime,
            &supervision,
        ),
    );
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    assert!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert!(wakes.take() > 0, "commit must wake the renderer");
    waiting.await;
    service(&state, &mut runtime).await;
    activation.await.expect("committed activation");
    let mut waiting = Box::pin(
        crate::render_thread::pipeline_driver::wait_for_scene_control(
            &state,
            &runtime,
            &supervision,
        ),
    );
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(barrier);
    assert!(wakes.take() > 0, "hold release must wake the renderer");
    waiting.await;
}

#[tokio::test]
async fn held_renderer_wakes_for_abort_and_release_before_wait_registration() {
    let wakes = Arc::new(ControlWakes::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    let supervision = crate::render_thread::supervision::RenderSupervision::new(
        crate::render_thread::supervision::RenderRestartPolicy::default(),
        None,
    );
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
    assert!(
        activation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    service(&state, &mut runtime).await;
    let mut waiting = Box::pin(
        crate::render_thread::pipeline_driver::wait_for_scene_control(
            &state,
            &runtime,
            &supervision,
        ),
    );
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(activation);
    assert!(wakes.take() > 0, "abort must wake the renderer");
    waiting.await;
    service(&state, &mut runtime).await;
    assert!(runtime.render.pending_layout_activation.is_none());
    drop(barrier);
    // A release before Notified::enable has no waiter; the state predicate
    // must still let the renderer continue without another notification.
    let mut waiting = Box::pin(
        crate::render_thread::pipeline_driver::wait_for_scene_control(
            &state,
            &runtime,
            &supervision,
        ),
    );
    assert!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    );
}

#[tokio::test]
async fn held_pipeline_remains_cancellable_without_another_control_message() {
    let (_directory, state, mut runtime) = fixture();
    let authority = authority(&state);
    let barrier = begin(&authority, &state, &mut runtime).await;
    state.render_loop.write().await.start();
    let supervision = crate::render_thread::supervision::RenderSupervision::new(
        crate::render_thread::supervision::RenderRestartPolicy::default(),
        None,
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut supervised = Box::pin(async {
        tokio::select! {
            () = crate::render_thread::pipeline_driver::run_pipeline(state.clone(), *runtime, supervision) => panic!("held pipeline unexpectedly exited"),
            () = cancel.cancelled() => {},
        }
    });
    assert!(
        supervised
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    cancel.cancel();
    supervised.await;
    assert!(
        barrier.is_active(),
        "cancellation did not need owner publication"
    );
    assert_eq!(state.render_loop.read().await.frame_number(), 0);
}
