//! Deterministic Windows input and capture integration fixtures.
//!
//! These tests inject immediately after the Windows OS adapters, then run the
//! real daemon publication pump, render-thread input routing, and bus outputs.

#![cfg(target_os = "windows")]

use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::screen::consumer::{
    CaptureConfig, CaptureEpoch, CaptureSourceId, PixelExtent,
};
use hypercolor_core::input::screen::implementer::WindowsScreenCaptureInput;
use hypercolor_core::input::{
    InputManager, ManagedSourceRole, SourceFreshness, SourceKind, SourceState, WindowsHostInput,
};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::SceneTransactionQueue;
use hypercolor_daemon::domain::scene::SceneService;
use hypercolor_daemon::domain::spatial::SpatialService;
use hypercolor_daemon::interaction_routing::InteractionRoutingControl;
use hypercolor_daemon::output_power::OutputPowerState;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{
    CanvasDims, InputPublicationConsumer, InputPublicationDemand, RenderThread, RenderThreadState,
};
use hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewStore;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::effect::EffectId;
use hypercolor_types::event::{HypercolorEvent, InputButtonState, InputEvent};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostKeyIdentity,
    HostKeySignal, HostPointerButton, HostPointerMotion, HostPointerSnapshot, HostRepeatEvidence,
};
use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tokio::sync::{Mutex, RwLock, watch};

fn host_device(source_id: &'static str, keyboard: bool) -> Arc<HostInputDevice> {
    Arc::new(HostInputDevice {
        source_id: Arc::from(source_id),
        label: Arc::from(format!("fixture {source_id}")),
        capabilities: HostInputCapabilities {
            keyboard,
            pointer: !keyboard,
        },
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
    state: &mut RenderThreadState,
    stem: &str,
    test_input_reactive: bool,
) {
    {
        let mut registry = state.effect_registry.write().await;
        let effect_id = builtin_effect_id(&registry, stem);
        if test_input_reactive {
            assert_eq!(
                registry.update(&effect_id, |entry| entry.metadata.input_reactive = true),
                Some(true)
            );
        }
    }
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
    let mut scenes = state.scene_manager.snapshot().await;
    scenes.create(scene.clone()).expect("fixture scene creates");
    scenes
        .activate(&scene.id, None)
        .expect("fixture scene activates");
    let scene_manager = SceneService::in_memory(scenes, Arc::clone(&state.event_bus));
    state.scene_plan = scene_manager.plan_reader();
    state.scene_manager = scene_manager;
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
    let scene_manager =
        SceneService::in_memory(SceneManager::with_default(), Arc::clone(&event_bus));
    let scene_plan = scene_manager.plan_reader();
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        asset_library: test_asset_library(),
        spatial_engine: SpatialService::new(SpatialEngine::new(empty_layout())),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        zone_layout_previews: Arc::new(ZoneLayoutPreviewStore::default()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager,
        scene_plan,
        input_manager,
        interaction_routing: InteractionRoutingControl::default(),
        power_state,
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
    state.input_manager.stop_all();
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

fn capture_epoch() -> CaptureEpoch {
    CaptureEpoch {
        source_id: CaptureSourceId::new("windows:fixture-display")
            .expect("fixture source id is valid"),
        topology_generation: 1,
        session_generation: 1,
    }
}

#[tokio::test]
async fn raw_input_reaches_daemon_frame_routing_and_event_bus() {
    let keyboard = host_device("keyboard-1", true);
    let mouse = host_device("mouse-1", false);
    let key_identity = HostKeyIdentity {
        key: Arc::from("a"),
        physical_code: Arc::from("windows:set1:none:1e"),
    };
    let events = vec![
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&keyboard),
        },
        HostInputEvent::Key {
            device: Some(Arc::clone(&keyboard)),
            identity: key_identity.clone(),
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::Unknown,
            },
        },
        HostInputEvent::Key {
            device: Some(keyboard),
            identity: key_identity,
            signal: HostKeySignal::Edge {
                pressed: true,
                repeat: HostRepeatEvidence::Unknown,
            },
        },
        HostInputEvent::DeviceArrived {
            device: Arc::clone(&mouse),
        },
        HostInputEvent::Button {
            device: Some(Arc::clone(&mouse)),
            button: HostPointerButton::left(),
            pressed: true,
            physical_code: Arc::from("windows:button:left"),
        },
        HostInputEvent::Motion {
            device: Some(mouse),
            motion: HostPointerMotion::Relative {
                delta_x: 120.0,
                delta_y: -60.0,
                units_per_x: 1200.0,
                units_per_y: 1200.0,
            },
        },
    ];
    let (source, fixture) = WindowsHostInput::new_deterministic_fixture(true, true);
    let manager = InputManager::new();
    let statuses = manager.source_status_registry();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(source)))
        .expect("Windows host fixture should register");
    manager.start_all().expect("fixture source starts");
    let mut state = render_state(manager, false);
    install_effect_with_test_demand_activation(&mut state, "solid_color", true).await;
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
            .publish(HostInputBatch {
                events: &events,
                pointer: Some(HostPointerSnapshot {
                    x: -100.0,
                    y: 200.0,
                    norm_x: 0.25,
                    norm_y: 0.75,
                    coordinate_space_generation: 1,
                }),
                at_ms: 1_000,
                device_catalog_generation: 1,
            })
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
async fn daemon_demand_activates_and_releases_the_deterministic_windows_source() {
    let config = CaptureConfig {
        target_fps: 60,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let epoch = capture_epoch();
    let (source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(config, epoch)
        .expect("deterministic Windows capture source is valid");
    let manager = InputManager::new();
    let statuses = manager.source_status_registry();
    manager
        .add_source(ManagedSourceRole::screen(Box::new(source)))
        .expect("Windows screen fixture should register");
    manager.start_all().expect("fixture source starts idle");
    let mut state = render_state(manager, true);
    install_effect_with_test_demand_activation(&mut state, "screen_cast", false).await;
    let mut render_thread = start_render_thread(&state).await;
    let demand = render_thread.input_publication_demands();
    let registration = demand.register(
        InputPublicationConsumer::Diagnostic,
        InputPublicationDemand::default().with_screen(
            60,
            PixelExtent::new(640, 480).expect("test screen extent should be non-empty"),
        ),
    );

    wait_until("daemon screen-capture demand", || fixture.is_active()).await;
    assert!(fixture.epoch_is_current());
    let registry = statuses.snapshot();
    let status = registry.handles()[0].snapshot();
    assert_eq!(status.backend.as_ref(), "dxgi_desktop_duplication");
    assert!(status.demanded);

    drop(registration);
    stop_render_thread(&state, &mut render_thread).await;
    assert!(!fixture.is_active());
    assert!(!fixture.epoch_is_current());
    assert_eq!(registry.handles()[0].snapshot().state, SourceState::Stopped);
}
