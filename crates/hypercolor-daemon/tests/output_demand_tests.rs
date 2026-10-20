//! Integration tests for output-demand gating.
//!
//! The render loop parks while nothing consumes its frames and resumes at
//! its full tier the moment a consumer appears. These tests drive the real
//! render thread and prove each consumer kind keeps or wakes it, that a
//! capture after a park returns a freshly rendered frame, and that effect
//! time carries across the park without a jump.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::SceneTransactionQueue;
use hypercolor_daemon::canvas_capture::{CanvasCaptureRequest, CanvasImageFormat, capture_canvas};
use hypercolor_daemon::domain::scene::SceneService;
use hypercolor_daemon::domain::spatial::SpatialService;
use hypercolor_daemon::output_demand::{OutputDemandTelemetry, is_output_device};
use hypercolor_daemon::output_power::OutputPowerState;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use hypercolor_daemon::simulators::SimulatedDisplayExt;
use hypercolor_types::api::simulators::SimulatedDisplay;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState};
use hypercolor_types::effect::EffectMetadata;
use hypercolor_types::event::FrameData;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

/// Deadline for liveness waits. Generous on purpose: these bound hangs,
/// they do not assert latency.
const WAIT_DEADLINE: Duration = Duration::from_secs(10);

/// Upper bound on how long an event-driven wake may take. The parked
/// safety tick is a full second, so a wake inside this bound came from the
/// demand signal, not the tick.
const WAKE_LATENCY_BOUND: Duration = Duration::from_millis(500);

/// Window used to prove a running loop renders at its tier rather than
/// trickling.
const RATE_WINDOW: Duration = Duration::from_millis(600);

/// Frames a 60 fps loop must render inside [`RATE_WINDOW`]. Well below the
/// 36 an unloaded machine renders, so a busy test host does not flake, and
/// far above anything a parked or tick-driven loop produces.
const MIN_FRAMES_IN_RATE_WINDOW: u64 = 15;

fn test_layout(zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "output-demand".into(),
        name: "Output Demand".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn point_zone(id: &str, device_id: &str, x: f32, y: f32) -> Output {
    Output {
        id: id.into(),
        name: id.into(),
        device_id: device_id.into(),
        zone_name: None,
        position: NormalizedPosition { x, y },
        size: NormalizedPosition { x: 0.2, y: 0.2 },
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Point,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

fn builtin_effect_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

fn builtin_effect_metadata(stem: &str) -> EffectMetadata {
    builtin_effect_registry()
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.metadata.clone())
        })
        .expect("builtin effect should exist")
}

fn solid_color_controls(r: u8, g: u8, b: u8) -> HashMap<String, ControlValue> {
    HashMap::from([(
        "color".into(),
        ControlValue::linear_color([
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ]),
    )])
}

fn test_asset_library() -> Arc<RwLock<AssetLibrary>> {
    let asset_tempdir = tempfile::tempdir().expect("test asset tempdir should be created");
    let asset_dir = asset_tempdir.path().join("assets");
    Arc::new(RwLock::new(
        AssetLibrary::open(asset_dir).expect("test asset library should open"),
    ))
}

/// A render state running `effect` on one point zone at the canvas center,
/// with no devices attached and nothing subscribed.
fn make_render_state(effect: &str, controls: HashMap<String, ControlValue>) -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let layout = test_layout(vec![point_zone("zone_main", "mock:main", 0.5, 0.5)]);
    let spatial_engine = SpatialEngine::new(layout.clone());
    let mut scene_manager = SceneManager::with_default();
    scene_manager
        .upsert_primary_zone(&builtin_effect_metadata(effect), controls, None, layout)
        .expect("test render state should seed a default primary zone");
    let scene_manager = SceneService::with_temporary_store(scene_manager, Arc::clone(&event_bus))
        .expect("temporary scene store should open");
    let scene_plan = scene_manager.plan_reader();
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        asset_library: test_asset_library(),
        spatial_engine: SpatialService::new(spatial_engine),
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
        scene_manager,
        scene_plan,
        input_manager: InputManager::new(),
        interaction_routing:
            hypercolor_daemon::interaction_routing::InteractionRoutingControl::default(),
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

async fn start(state: &RenderThreadState) -> RenderThread {
    state.render_loop.write().await.start();
    RenderThread::spawn(state.clone())
}

async fn stop(state: &RenderThreadState, mut render_thread: RenderThread) {
    state.render_loop.write().await.stop();
    render_thread.shutdown().await.expect("shutdown");
}

async fn telemetry(state: &RenderThreadState) -> OutputDemandTelemetry {
    state.performance.read().await.output_demand_telemetry()
}

async fn frame_number(state: &RenderThreadState) -> u64 {
    state.render_loop.read().await.frame_number()
}

/// Wait until the loop has parked `parks` times in total and return the
/// render loop frame number it parked at.
async fn wait_until_parked(state: &RenderThreadState, parks: u64) -> u64 {
    tokio::time::timeout(WAIT_DEADLINE, async {
        loop {
            let telemetry = telemetry(state).await;
            if telemetry.parked_since.is_some() && telemetry.parks_total >= parks {
                return frame_number(state).await;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("render loop should park (park #{parks}) in time"))
}

/// Wait until the loop renders past `frame`, returning how long it took.
async fn wait_until_rendering_past(state: &RenderThreadState, frame: u64) -> Duration {
    let started = Instant::now();
    tokio::time::timeout(WAIT_DEADLINE, async {
        while frame_number(state).await <= frame {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("render loop should resume rendering in time");
    started.elapsed()
}

async fn frames_rendered_in(state: &RenderThreadState, window: Duration) -> u64 {
    let before = frame_number(state).await;
    tokio::time::sleep(window).await;
    frame_number(state).await.saturating_sub(before)
}

async fn assert_renders_at_full_tier(state: &RenderThreadState, consumer: &str) {
    let frames = frames_rendered_in(state, RATE_WINDOW).await;
    assert!(
        frames >= MIN_FRAMES_IN_RATE_WINDOW,
        "{consumer} should keep the loop rendering at its tier, rendered {frames} frames in {RATE_WINDOW:?}"
    );
    assert_eq!(
        state.render_loop.read().await.stats().tier,
        FpsTier::Full,
        "{consumer} should not change the FPS tier"
    );
}

fn mock_device_info(name: &str) -> DeviceInfo {
    let backend = MockDeviceBackend::new().with_device(&MockDeviceConfig {
        name: name.into(),
        led_count: 8,
        topology: LedTopology::Point,
        id: Some(DeviceId::new()),
    });
    backend.device_infos()[0].clone()
}

async fn connect_device(registry: &DeviceRegistry, info: DeviceInfo) -> DeviceId {
    let device_id = registry.add(info).await;
    assert!(
        registry.set_state(&device_id, DeviceState::Connected).await,
        "test device should move to connected"
    );
    device_id
}

fn zone_main_color(frame: &FrameData) -> [u8; 3] {
    frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_main")
        .and_then(|zone| zone.colors.first().copied())
        .expect("frame should carry the main zone")
}

/// HSV hue of an 8-bit color, in degrees.
fn hue_degrees([r, g, b]: [u8; 3]) -> f32 {
    let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    if delta <= f32::EPSILON {
        return 0.0;
    }
    let sector = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    sector * 60.0
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let difference = (a - b).rem_euclid(360.0);
    difference.min(360.0 - difference)
}

#[tokio::test]
async fn unconsumed_render_loop_parks_and_renders_nothing() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;

    let parked_at = wait_until_parked(&state, 1).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    assert_eq!(
        frame_number(&state).await,
        parked_at,
        "a parked loop must not render, safety ticks included"
    );
    let telemetry = telemetry(&state).await;
    assert!(telemetry.parked_since.is_some());
    assert_eq!(telemetry.parks_total, 1);
    assert_eq!(
        state.render_loop.read().await.stats().tier,
        FpsTier::Full,
        "parking leaves the tier where it was"
    );
    assert!(
        !telemetry.consumers.any(),
        "the gate should have counted no consumers: {:?}",
        telemetry.consumers
    );

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn connected_device_keeps_the_loop_rendering_without_parking() {
    let state = make_render_state("rainbow", HashMap::new());
    connect_device(&state.device_registry, mock_device_info("Desk Strip")).await;
    let render_thread = start(&state).await;

    assert_renders_at_full_tier(&state, "a connected device").await;
    assert_renders_at_full_tier(&state, "a connected device").await;
    assert_eq!(
        telemetry(&state).await.parks_total,
        0,
        "a rig with a connected device never parks"
    );

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn device_connect_wakes_a_parked_loop_and_disconnect_parks_it_again() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;
    let parked_at = wait_until_parked(&state, 1).await;

    let device_id = connect_device(&state.device_registry, mock_device_info("Fan Ring")).await;
    let latency = wait_until_rendering_past(&state, parked_at).await;
    assert!(
        latency < WAKE_LATENCY_BOUND,
        "device connect should wake the loop at once, took {latency:?}"
    );
    assert_renders_at_full_tier(&state, "a newly connected device").await;
    assert_eq!(
        telemetry(&state).await.last_wake_reason,
        Some("output_consumer")
    );

    state
        .device_registry
        .set_state(&device_id, DeviceState::Reconnecting)
        .await;
    wait_until_parked(&state, 2).await;

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn simulated_display_without_a_preview_is_not_output_demand() {
    let simulator = SimulatedDisplay {
        id: DeviceId::new(),
        name: "Virtual LCD".into(),
        width: 240,
        height: 240,
        circular: true,
        enabled: true,
    }
    .device_info();
    assert!(!is_output_device(&simulator, true));
    assert!(is_output_device(&mock_device_info("Strip"), true));
    assert!(!is_output_device(&mock_device_info("Strip"), false));

    let state = make_render_state("rainbow", HashMap::new());
    connect_device(&state.device_registry, simulator).await;
    let render_thread = start(&state).await;

    let parked_at = wait_until_parked(&state, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(frame_number(&state).await, parked_at);

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn canvas_preview_subscription_wakes_a_parked_loop_at_full_tier() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;
    let parked_at = wait_until_parked(&state, 1).await;

    let subscribed_at = Instant::now();
    let mut preview = state.preview_runtime.canvas_receiver();
    tokio::time::timeout(WAIT_DEADLINE, preview.changed())
        .await
        .expect("a preview subscriber should receive a frame")
        .expect("canvas lane should stay open");
    let latency = subscribed_at.elapsed();
    assert!(
        latency < WAKE_LATENCY_BOUND,
        "a preview subscription should wake the loop at once, took {latency:?}"
    );
    assert!(
        u64::from(preview.borrow_and_update().frame_number) >= parked_at,
        "the preview should receive a frame rendered after the wake"
    );
    assert_renders_at_full_tier(&state, "a canvas preview").await;
    assert_eq!(telemetry(&state).await.consumers.stream_receivers, 1);

    drop(preview);
    wait_until_parked(&state, 2).await;

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn scene_canvas_receiver_for_a_display_backdrop_wakes_a_parked_loop() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;
    let parked_at = wait_until_parked(&state, 1).await;

    let mut backdrop = state.preview_runtime.scene_canvas_receiver();
    let latency = wait_until_rendering_past(&state, parked_at).await;
    assert!(
        latency < WAKE_LATENCY_BOUND,
        "a display backdrop receiver should wake the loop at once, took {latency:?}"
    );
    tokio::time::timeout(WAIT_DEADLINE, backdrop.changed())
        .await
        .expect("the backdrop should receive a scene canvas")
        .expect("scene canvas lane should stay open");
    assert_renders_at_full_tier(&state, "a display backdrop").await;

    drop(backdrop);
    wait_until_parked(&state, 2).await;

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn frame_and_zone_canvas_receivers_and_demand_holds_wake_a_parked_loop() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;

    let parked_at = wait_until_parked(&state, 1).await;
    let frames = state.event_bus.frame_receiver();
    wait_until_rendering_past(&state, parked_at).await;
    assert_renders_at_full_tier(&state, "a frame stream receiver").await;
    drop(frames);

    let parked_at = wait_until_parked(&state, 2).await;
    let zone_canvas = state
        .event_bus
        .zone_canvas_receiver(hypercolor_types::scene::ZoneId::new());
    wait_until_rendering_past(&state, parked_at).await;
    drop(zone_canvas);

    let parked_at = wait_until_parked(&state, 3).await;
    let hold = state.event_bus.hold_output_demand();
    wait_until_rendering_past(&state, parked_at).await;
    assert_renders_at_full_tier(&state, "an output demand hold").await;
    drop(hold);
    wait_until_parked(&state, 4).await;

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn canvas_capture_after_a_park_returns_a_freshly_rendered_frame() {
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;
    let parked_at = wait_until_parked(&state, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let requested_at = Instant::now();
    let captured = capture_canvas(
        &state.preview_runtime,
        CanvasCaptureRequest::single(CanvasImageFormat::Png),
    )
    .await
    .expect("capture should succeed");
    let latency = requested_at.elapsed();

    let frame = captured.first().expect("capture should return one frame");
    assert!(frame.fresh, "a capture must wake the loop for a new frame");
    // The park closed its frame without rendering, so `parked_at` is the
    // number of the first frame after the wake. That frame still composes
    // producer output requested before the park; the capture skips it.
    assert!(
        u64::from(frame.frame_number) > parked_at,
        "the capture should skip the wake frame and return one rendered after the request, got frame {} after parking at {parked_at}",
        frame.frame_number
    );
    assert!(
        !state
            .preview_runtime
            .is_render_resume_frame(frame.frame_number)
    );
    assert!(
        latency < WAKE_LATENCY_BOUND,
        "a capture of a parked loop should land within a frame or two, took {latency:?}"
    );
    assert_eq!(frame.source_width, 320);
    assert_eq!(frame.source_height, 200);

    // The capture's demand ends with it, so the loop parks again.
    wait_until_parked(&state, 2).await;

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn effect_time_stays_continuous_across_park_and_resume() {
    // Rainbow turns its hue 60 degrees per second, about one degree per
    // frame at 60 fps. A clock that kept running through a 1.5 second park
    // would jump about 90 degrees on resume.
    let state = make_render_state("rainbow", HashMap::new());
    let render_thread = start(&state).await;
    wait_until_parked(&state, 1).await;
    let (parked_frame, parked_hue) = {
        let frame = state.event_bus.frame_lane().borrow();
        (frame.frame_number, hue_degrees(zone_main_color(&frame)))
    };
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let mut frames = state.event_bus.frame_receiver();
    let resumed = tokio::time::timeout(WAIT_DEADLINE, async {
        loop {
            frames.changed().await.expect("frame lane should stay open");
            let frame = frames.borrow_and_update().clone();
            if frame.frame_number > parked_frame {
                break frame;
            }
        }
    })
    .await
    .expect("a frame subscriber should wake the loop");
    let resumed_hue = hue_degrees(zone_main_color(&resumed));
    let resume_step = hue_distance(parked_hue, resumed_hue);
    assert!(
        resume_step < 12.0,
        "effect time should step one frame across the park, hue moved {resume_step} degrees ({parked_hue} -> {resumed_hue})"
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    let later_hue = hue_degrees(zone_main_color(&frames.borrow_and_update()));
    let progress = hue_distance(resumed_hue, later_hue);
    assert!(
        progress > 10.0,
        "effect time should keep flowing after the resume, hue moved {progress} degrees in 500 ms"
    );

    stop(&state, render_thread).await;
}

#[tokio::test]
async fn scene_change_while_parked_renders_the_new_effect_then_parks_again() {
    let state = make_render_state("solid_color", solid_color_controls(255, 0, 0));
    let render_thread = start(&state).await;
    wait_until_parked(&state, 1).await;
    assert_eq!(
        zone_main_color(&state.event_bus.frame_lane().borrow()),
        [255, 0, 0]
    );

    let layout = state.spatial_engine.snapshot().layout().as_ref().clone();
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            &builtin_effect_metadata("solid_color"),
            solid_color_controls(0, 0, 255),
            None,
            layout,
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("scene mutation should stage");
    state
        .scene_manager
        .commit_mutation(mutation)
        .await
        .expect("scene mutation should commit");

    // Nothing subscribes, yet the producers settle on the new scene so a
    // later consumer sees it at once.
    tokio::time::timeout(WAIT_DEADLINE, async {
        while zone_main_color(&state.event_bus.frame_lane().borrow()) != [0, 0, 255] {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the parked loop should settle the new effect");
    wait_until_parked(&state, 2).await;
    assert_eq!(
        telemetry(&state).await.last_wake_reason,
        Some("producer_state")
    );

    stop(&state, render_thread).await;
}
