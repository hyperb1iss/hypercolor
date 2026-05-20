use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::{DisplayGroupFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use hypercolor_daemon::scene_transactions::SceneTransactionQueue;
use hypercolor_daemon::session::OutputPowerState;
use hypercolor_types::canvas::Rgba;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::layer::{LayerBlendMode, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::sync::{Mutex, RwLock, watch};

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
        .expect("builtin effect should exist")
}

fn test_layout(zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "multi-layer-test".into(),
        name: "Multi Layer Test".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn full_zone(id: &str) -> Output {
    Output {
        id: id.into(),
        name: id.into(),
        device_id: id.into(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 1,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn solid_layer(
    effect_id: EffectId,
    color: [f32; 4],
    blend: LayerBlendMode,
    opacity: f32,
) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::from([("color".into(), ControlValue::Color(color))]),
        HashMap::new(),
        None,
    );
    layer.blend = blend;
    layer.opacity = opacity;
    layer
}

fn render_group(name: &str, effect_id: EffectId, layers: Vec<SceneLayer>) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.into(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers,
        layout: test_layout(vec![full_zone("zone:all")]),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }
}

fn display_group(
    group_id: ZoneId,
    device_id: DeviceId,
    effect_id: EffectId,
    layers: Vec<SceneLayer>,
) -> Zone {
    let mut group = render_group("Display", effect_id, layers);
    group.id = group_id;
    group.layout = test_layout(Vec::new());
    group.display_target = Some(DisplayFaceTarget::new(device_id));
    group.role = ZoneRole::Display;
    group
}

fn render_state() -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let asset_tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let asset_dir = asset_tempdir.path().join("assets");
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        asset_library: Arc::new(RwLock::new(
            AssetLibrary::open(asset_dir).expect("test asset library should open"),
        )),
        spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(test_layout(Vec::new())))),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        zone_layout_previews: Arc::new(
            hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewStore::default(),
        ),
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

async fn install_scene(state: &RenderThreadState, groups: Vec<Zone>) {
    let mut scene = make_scene("Multi Layer Scene");
    scene.groups = groups;
    scene.unassigned_behavior = UnassignedBehavior::Off;
    let mut scene_manager = state.scene_manager.write().await;
    scene_manager.create(scene.clone()).expect("create scene");
    scene_manager
        .activate(&scene.id, None)
        .expect("activate scene");
}

async fn run_until_canvas_frame(state: &RenderThreadState) -> hypercolor_core::bus::CanvasFrame {
    let mut canvas_rx = state.event_bus.canvas_receiver();
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.start();
    }
    let mut render_thread = RenderThread::spawn(state.clone());
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("expected canvas frame within 2 seconds")
        .expect("canvas sender should remain connected");
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.stop();
    }
    render_thread.shutdown().await.expect("shutdown");
    canvas_rx.borrow().clone()
}

#[tokio::test]
async fn duplicate_effect_layers_compose_bottom_to_top() {
    let state = render_state();
    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let group = render_group(
        "Layered",
        solid_id,
        vec![
            solid_layer(solid_id, [1.0, 0.0, 0.0, 1.0], LayerBlendMode::Replace, 1.0),
            solid_layer(solid_id, [0.0, 0.0, 1.0, 1.0], LayerBlendMode::Alpha, 0.5),
        ],
    );
    install_scene(&state, vec![group]).await;

    let frame = run_until_canvas_frame(&state).await;
    let pixel = frame.surface().get_pixel(160, 100);

    assert!(pixel.r > 0, "base layer should contribute red");
    assert!(pixel.b > 0, "top layer should contribute blue");
    assert_eq!(pixel.g, 0);
    assert_eq!(pixel.a, 255);
}

#[tokio::test]
async fn disabled_effect_layers_do_not_contribute_to_output() {
    let state = render_state();
    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let mut disabled = solid_layer(solid_id, [0.0, 0.0, 1.0, 1.0], LayerBlendMode::Replace, 1.0);
    disabled.enabled = false;
    let group = render_group(
        "Disabled Overlay",
        solid_id,
        vec![
            solid_layer(solid_id, [1.0, 0.0, 0.0, 1.0], LayerBlendMode::Replace, 1.0),
            disabled,
        ],
    );
    install_scene(&state, vec![group]).await;

    let frame = run_until_canvas_frame(&state).await;

    assert_eq!(
        frame.surface().get_pixel(160, 100),
        Rgba::new(255, 0, 0, 255)
    );
}

#[tokio::test]
async fn display_layer_stack_publishes_separately_from_scene_canvas() {
    let state = render_state();
    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let display_group_id = ZoneId::new();
    let display_device_id = DeviceId::new();
    let group_canvas_sender = state.event_bus.group_canvas_sender(display_group_id);
    let mut group_canvas_rx = group_canvas_sender.subscribe();
    let scene_group = render_group(
        "Scene",
        solid_id,
        vec![solid_layer(
            solid_id,
            [1.0, 0.0, 0.0, 1.0],
            LayerBlendMode::Replace,
            1.0,
        )],
    );
    let face_group = display_group(
        display_group_id,
        display_device_id,
        solid_id,
        vec![solid_layer(
            solid_id,
            [0.0, 0.0, 1.0, 1.0],
            LayerBlendMode::Replace,
            1.0,
        )],
    );
    install_scene(&state, vec![scene_group, face_group]).await;

    let scene_frame = run_until_canvas_frame(&state).await;
    tokio::time::timeout(Duration::from_secs(2), group_canvas_rx.changed())
        .await
        .expect("expected display group frame within 2 seconds")
        .expect("group canvas sender should remain connected");

    assert_eq!(
        scene_frame.surface().get_pixel(160, 100),
        Rgba::new(255, 0, 0, 255)
    );
    let DisplayGroupFrame::Canvas(face_frame) = group_canvas_rx.borrow().clone() else {
        panic!("display group should publish a canvas frame");
    };
    assert_eq!(&face_frame.rgba_bytes()[0..4], [0, 0, 255, 255].as_slice());
}
