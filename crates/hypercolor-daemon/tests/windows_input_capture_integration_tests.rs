//! Deterministic Windows input and capture integration fixtures.
//!
//! These tests inject immediately after the Windows OS adapters, then run the
//! real daemon publication pump, render-thread input routing, and bus outputs.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureConfig, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CpuCaptureStorage, PhysicalOrigin, PixelExtent, RawCaptureSurface, SourceScale,
    WindowsScreenCaptureInput,
};
use hypercolor_core::input::{
    InputManager, SourceFreshness, SourceKind, SourceState, WindowsHostInput,
};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::interaction_routing::InteractionRoutingControl;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{
    CanvasDims, InputPublicationConsumer, InputPublicationDemand, RenderThread, RenderThreadState,
};
use hypercolor_daemon::scene_transactions::SceneTransactionQueue;
use hypercolor_daemon::session::OutputPowerState;
use hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewStore;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::effect::EffectId;
use hypercolor_types::event::{HypercolorEvent, InputButtonState, InputEvent};
use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use hypercolor_windows_input::{
    RawButton, RawCursor, RawDeviceDescriptor, RawDeviceKind, RawInputEvent, RawKeyPrefix,
};
use tokio::sync::{Mutex, RwLock, watch};

fn raw_device(source_id: &'static str, kind: RawDeviceKind) -> Arc<RawDeviceDescriptor> {
    Arc::new(RawDeviceDescriptor {
        source_id: Arc::from(source_id),
        interface_path: Some(Arc::from(format!("fixture:{source_id}"))),
        label: Arc::from(format!("fixture {source_id}")),
        kind,
        session_generation: 1,
        device_generation: 1,
    })
}

fn empty_layout() -> SpatialLayout {
    SpatialLayout {
        id: "windows-input-capture-fixture".to_owned(),
        name: "Windows input capture fixture".to_owned(),
        description: None,
        canvas_width: 4,
        canvas_height: 3,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn builtin_effect_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
    registry
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.metadata.id)
        })
        .expect("fixture builtin effect exists")
}

async fn install_effect_with_test_demand_activation(
    state: &RenderThreadState,
    stem: &str,
    test_input_reactive: bool,
) {
    let effect_id = {
        let mut registry = state.effect_registry.write().await;
        let effect_id = builtin_effect_id(&registry, stem);
        if test_input_reactive {
            assert_eq!(
                registry.update(&effect_id, |entry| entry.metadata.input_reactive = true),
                Some(true)
            );
        }
        effect_id
    };
    let mut scene = make_scene("Windows input capture fixture");
    scene.transition.duration_ms = 0;
    scene.unassigned_behavior = UnassignedBehavior::Off;
    scene.zones = vec![Zone {
        id: ZoneId::new(),
        name: "Fixture".to_owned(),
        description: None,
        layers: Vec::new(),
        layout: empty_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }];
    let mut scenes = state.scene_manager.write().await;
    scenes.create(scene.clone()).expect("fixture scene creates");
    scenes
        .activate(&scene.id, None)
        .expect("fixture scene activates");
}

fn test_asset_library() -> Arc<RwLock<AssetLibrary>> {
    let directory = tempfile::tempdir().expect("fixture asset directory is created");
    Arc::new(RwLock::new(
        AssetLibrary::open(directory.path().join("assets")).expect("fixture asset library opens"),
    ))
}

fn render_state(input_manager: InputManager, screen_capture_configured: bool) -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        asset_library: test_asset_library(),
        spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(empty_layout()))),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        zone_layout_previews: Arc::new(ZoneLayoutPreviewStore::default()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(SceneManager::with_default())),
        input_manager: Arc::new(Mutex::new(input_manager)),
        interaction_routing: InteractionRoutingControl::default(),
        power_state,
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
            "windows-input-capture-fixture-settings.json",
        )))),
        scene_transactions: SceneTransactionQueue::default(),
        screen_capture_configured,
        canvas_dims: CanvasDims::new(4, 3),
        render_acceleration_mode: RenderAccelerationMode::Cpu,
        #[cfg(feature = "wgpu")]
        render_gpu_device: None,
        configured_max_fps_tier: FpsTier::Full.into(),
        face_fps_cap: 30,
    }
}

async fn start_render_thread(state: &RenderThreadState) -> RenderThread {
    state.render_loop.write().await.start();
    RenderThread::spawn(state.clone())
}

async fn stop_render_thread(state: &RenderThreadState, render_thread: &mut RenderThread) {
    state.render_loop.write().await.stop();
    render_thread
        .shutdown()
        .await
        .expect("fixture render thread shuts down");
    state.input_manager.lock().await.stop_all();
}

async fn wait_until(description: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if condition() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"));
}

async fn wait_for_canvas(
    receiver: &mut watch::Receiver<CanvasFrame>,
    description: &str,
    accepts: impl Fn(&CanvasFrame) -> bool,
) -> CanvasFrame {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            receiver
                .changed()
                .await
                .expect("canvas sender remains connected");
            let frame = receiver.borrow().clone();
            if accepts(&frame) {
                break frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

fn capture_epoch() -> CaptureEpoch {
    CaptureEpoch {
        source_id: CaptureSourceId::new("windows:fixture-display")
            .expect("fixture source id is valid"),
        topology_generation: 1,
        session_generation: 1,
    }
}

fn capture_frame(epoch: &CaptureEpoch) -> CaptureFrame<RawCaptureSurface> {
    let extent = PixelExtent::new(4, 3).expect("fixture extent is nonempty");
    let captured_at = Instant::now();
    let pixels: Arc<[u8]> = Arc::from([
        255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255, 255, 0, 0,
        255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255, 255,
        0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
    ]);
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: epoch.source_id.clone(),
            topology_generation: epoch.topology_generation,
            session_generation: epoch.session_generation,
            sequence: 1,
            captured_at,
            fresh_until: captured_at + Duration::from_secs(1),
            geometry: CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("fixture geometry is valid"),
            colorimetry: CaptureColorimetry::SRGB,
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            pixels,
            CapturePixelFormat::Rgba8,
            16,
            0,
        )),
        CaptureDamage::default(),
    )
    .expect("fixture capture frame is valid")
}

fn contains_fixture_colors(frame: &CanvasFrame) -> bool {
    let mut red = false;
    let mut blue = false;
    for pixel in frame.rgba_bytes().chunks_exact(4) {
        red |= pixel[0] > 240 && pixel[1] < 15 && pixel[2] < 15 && pixel[3] == 255;
        blue |= pixel[0] < 15 && pixel[1] < 15 && pixel[2] > 240 && pixel[3] == 255;
        if red && blue {
            return true;
        }
    }
    false
}

#[tokio::test]
async fn raw_input_reaches_daemon_frame_routing_and_event_bus() {
    let keyboard = raw_device("keyboard-1", RawDeviceKind::Keyboard);
    let mouse = raw_device("mouse-1", RawDeviceKind::Mouse);
    let events = vec![
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&keyboard),
        },
        RawInputEvent::Key {
            device: Arc::clone(&keyboard),
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::Key {
            device: keyboard,
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&mouse),
        },
        RawInputEvent::Button {
            device: Arc::clone(&mouse),
            button: RawButton::Left,
            pressed: true,
        },
        RawInputEvent::MotionRelative {
            device: mouse,
            dx: 120,
            dy: -60,
        },
    ];
    let (source, fixture) = WindowsHostInput::new_deterministic_fixture(true, true);
    let mut manager = InputManager::new();
    let statuses = manager.source_status_registry();
    manager.add_source(Box::new(source));
    manager.start_all().expect("fixture source starts");
    let state = render_state(manager, false);
    install_effect_with_test_demand_activation(&state, "solid_color", true).await;
    let mut event_receiver = state.event_bus.subscribe_all();
    let mut render_thread = start_render_thread(&state).await;
    let demand = render_thread.input_publication_demands();
    let _registration = demand.register(
        InputPublicationConsumer::Diagnostic,
        InputPublicationDemand::default().with_source(SourceKind::Interaction, 60),
    );

    wait_until("daemon Raw Input demand", || {
        let status = statuses.snapshot().handles()[0].snapshot();
        fixture.is_active() && status.demanded && status.state == SourceState::Live
    })
    .await;
    assert_eq!(
        statuses.snapshot().handles()[0].snapshot().resource_count,
        0
    );
    assert!(
        fixture
            .publish(
                &events,
                Some(RawCursor {
                    x: -100,
                    y: 200,
                    norm_x: 0.25,
                    norm_y: 0.75,
                }),
                1_000,
            )
            .expect("post-adapter Raw Input batch is accepted")
    );

    let routed = tokio::time::timeout(Duration::from_secs(2), async {
        let mut routed = Vec::new();
        while routed.len() < 3 {
            match event_receiver.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::InputEventReceived { event } = timestamped.event {
                        routed.push(event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("input event bus closed")
                }
            }
        }
        routed
    })
    .await
    .expect("daemon routes all folded Raw Input events");

    assert!(
        routed
            .windows(2)
            .all(|events| events[0].seq < events[1].seq)
    );
    assert_eq!(routed[0].at_ms, 1_000);
    assert_eq!(
        routed[0].physical_code.as_deref(),
        Some("windows:set1:none:1e")
    );
    assert_eq!(routed[0].repeat_count, 1);
    assert!(matches!(
        routed[0].event,
        InputEvent::Key {
            ref source_id,
            ref key,
            state: InputButtonState::Pressed,
        } if source_id == "keyboard-1" && key == "a"
    ));
    assert!(matches!(
        routed[1].event,
        InputEvent::Key {
            state: InputButtonState::Repeated,
            ..
        }
    ));
    assert!(matches!(
        routed[2].event,
        InputEvent::MouseButton {
            ref button,
            state: InputButtonState::Pressed,
            ..
        } if button == "left"
    ));

    let registry = statuses.snapshot();
    let status = registry.handles()[0].snapshot();
    assert_eq!(status.backend.as_ref(), "raw_input");
    assert_eq!(status.state, SourceState::Live);
    assert_eq!(status.freshness, SourceFreshness::NotApplicable);
    assert_eq!(status.resource_count, 2);

    stop_render_thread(&state, &mut render_thread).await;
    assert!(!fixture.is_active());
    assert_eq!(registry.handles()[0].snapshot().state, SourceState::Stopped);
}

#[tokio::test]
async fn capture_frame_reaches_daemon_screen_and_canvas_watches() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let epoch = capture_epoch();
    let (source, fixture) =
        WindowsScreenCaptureInput::new_deterministic_fixture(config, epoch.clone())
            .expect("deterministic Windows capture source is valid");
    let mut manager = InputManager::new();
    let statuses = manager.source_status_registry();
    manager.add_source(Box::new(source));
    manager.start_all().expect("fixture source starts idle");
    let state = render_state(manager, true);
    install_effect_with_test_demand_activation(&state, "screen_cast", false).await;
    let mut canvas_receiver = state.event_bus.canvas_receiver();
    let mut screen_receiver = state.event_bus.screen_canvas_receiver();
    let mut render_thread = start_render_thread(&state).await;
    let demand = render_thread.input_publication_demands();
    let _registration = demand.register(
        InputPublicationConsumer::Diagnostic,
        InputPublicationDemand::default().with_screen(
            60,
            PixelExtent::new(640, 480).expect("test screen extent should be non-empty"),
        ),
    );

    wait_until("daemon screen-capture demand", || fixture.is_active()).await;
    assert!(
        fixture
            .publish(capture_frame(&epoch))
            .expect("adapter-boundary frame is accepted")
    );
    let screen = wait_for_canvas(
        &mut screen_receiver,
        "screen canvas watch",
        contains_fixture_colors,
    )
    .await;
    let canvas = wait_for_canvas(
        &mut canvas_receiver,
        "composed canvas watch",
        contains_fixture_colors,
    )
    .await;

    assert!(screen.width >= 4);
    assert!(screen.height >= 3);
    assert!(screen.frame_number > 0);
    assert_eq!((canvas.width, canvas.height), (4, 3));
    assert!(canvas.frame_number > 0);
    let registry = statuses.snapshot();
    let status = registry.handles()[0].snapshot();
    assert_eq!(status.backend.as_ref(), "dxgi_desktop_duplication");
    assert_eq!(status.state, SourceState::Live);
    assert_eq!(status.freshness, SourceFreshness::Fresh);
    assert_eq!(status.resource_count, 1);

    stop_render_thread(&state, &mut render_thread).await;
    assert!(!fixture.is_active());
    assert_eq!(registry.handles()[0].snapshot().state, SourceState::Stopped);
}
