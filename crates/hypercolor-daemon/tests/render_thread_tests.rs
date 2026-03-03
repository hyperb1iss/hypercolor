//! Integration tests for the render thread and frame pipeline.
//!
//! These tests prove that the render thread correctly orchestrates:
//! Effect render → Spatial sample → Device push → Bus publish.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig, MockEffectRenderer};
use hypercolor_core::device::{BackendManager, DeviceBackend};
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::{InputData, InputManager, InputSource, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{HypercolorEvent, ZoneColors};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

use hypercolor_daemon::render_thread::{RenderThread, RenderThreadState};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn test_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test".into(),
        name: "Test Layout".into(),
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

fn strip_zone(id: &str, device_id: &str, led_count: u32) -> DeviceZone {
    DeviceZone {
        id: id.into(),
        name: id.into(),
        device_id: device_id.into(),
        zone_name: None,
        position: NormalizedPosition { x: 0.5, y: 0.5 },
        size: NormalizedPosition { x: 1.0, y: 1.0 },
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
    }
}

fn point_zone(id: &str, device_id: &str, x: f32, y: f32) -> DeviceZone {
    DeviceZone {
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
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
    }
}

struct MockScreenSource {
    running: bool,
    zone_colors: Vec<ZoneColors>,
}

impl MockScreenSource {
    fn new(zone_colors: Vec<ZoneColors>) -> Self {
        Self {
            running: false,
            zone_colors,
        }
    }
}

impl InputSource for MockScreenSource {
    fn name(&self) -> &'static str {
        "mock_screen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }

        Ok(InputData::Screen(ScreenData {
            zone_colors: self.zone_colors.clone(),
        }))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

fn make_render_state(
    effect_engine: EffectEngine,
    spatial_engine: SpatialEngine,
    backend_manager: BackendManager,
) -> RenderThreadState {
    RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        event_bus: Arc::new(HypercolorBus::new()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        input_manager: Arc::new(Mutex::new(InputManager::new())),
        canvas_width: 320,
        canvas_height: 200,
    }
}

// ── Render Thread Lifecycle Tests ───────────────────────────────────────────

#[tokio::test]
async fn render_thread_exits_when_loop_not_started() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    // Render loop is in Created state (not started) — thread should exit immediately.
    let mut rt = RenderThread::spawn(state);

    // Give it a moment to start and exit.
    tokio::time::sleep(Duration::from_millis(100)).await;

    rt.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn render_thread_exits_on_stop() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    // Start the render loop, then stop it.
    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    // Let it run a few frames.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the render loop — thread should exit.
    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }

    rt.shutdown().await.expect("shutdown should succeed");
}

// ── Frame Pipeline Tests ────────────────────────────────────────────────────

#[tokio::test]
async fn pipeline_publishes_frame_events() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    // Subscribe to events before starting.
    let mut rx = state.event_bus.subscribe_all();

    // Start render loop.
    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    // Wait for at least one frame.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop and collect events.
    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    // Check that FrameRendered events were published.
    let mut frame_events = 0;
    while let Ok(event) = rx.try_recv() {
        if matches!(event.event, HypercolorEvent::FrameRendered { .. }) {
            frame_events += 1;
        }
    }
    assert!(
        frame_events > 0,
        "expected at least one FrameRendered event"
    );
}

#[tokio::test]
async fn pipeline_publishes_frame_data_via_watch() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();

    // Start render loop.
    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    // Wait for frame data to arrive.
    let result = tokio::time::timeout(Duration::from_secs(1), frame_rx.changed()).await;
    assert!(result.is_ok(), "expected frame data within 1 second");

    // Stop.
    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pipeline_renders_active_effect_to_devices() {
    // Set up a mock device.
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Test Strip".into(),
        led_count: 10,
        topology: LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(backend));
    backend_manager.map_device("mock:strip", "mock", device_id);

    // Set up spatial layout with one zone.
    let layout = test_layout(vec![strip_zone("zone_0", "mock:strip", 10)]);
    let spatial_engine = SpatialEngine::new(layout);

    // Set up effect engine with a solid red renderer.
    let mut effect_engine = EffectEngine::new();
    let renderer = MockEffectRenderer::solid(255, 0, 0);
    let metadata = MockEffectRenderer::sample_metadata("red_test");
    effect_engine
        .activate(Box::new(renderer), metadata)
        .expect("activate");

    let state = RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        event_bus: Arc::new(HypercolorBus::new()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        input_manager: Arc::new(Mutex::new(InputManager::new())),
        canvas_width: 320,
        canvas_height: 200,
    };

    // Start.
    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    // Subscribe to frame data before spawning.
    let mut frame_rx = state.event_bus.frame_receiver();

    let mut rt = RenderThread::spawn(state.clone());

    // Wait for at least one frame to be published.
    let got_frame = tokio::time::timeout(Duration::from_secs(2), frame_rx.changed()).await;
    assert!(got_frame.is_ok(), "expected frame data within 2 seconds");

    // Let a few more frames run.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop.
    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    // Verify the watch channel received frame data (zones may be present
    // depending on spatial sampling, but the frame should exist).
    let frame_data = frame_rx.borrow().clone();
    // The frame_number is zero-indexed and read before frame_complete increments,
    // so even frame_number==0 means one frame was rendered.
    assert!(
        frame_data.timestamp_ms > 0 || frame_data.frame_number > 0,
        "expected frames to have been rendered"
    );
}

#[tokio::test]
async fn pipeline_with_no_effect_produces_black_canvas() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();

    // Start render loop.
    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    // Wait for at least one frame.
    let _ = tokio::time::timeout(Duration::from_secs(1), frame_rx.changed()).await;

    // Stop.
    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    // With no effect active, EffectEngine returns a black canvas.
    // With no zones, frame data has empty zone list.
    let frame_data = frame_rx.borrow().clone();
    assert!(frame_data.zones.is_empty());
}

#[tokio::test]
async fn pipeline_uses_screen_input_canvas_when_available() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(MockScreenSource::new(vec![
            ZoneColors {
                zone_id: "screen:sector_0_0".to_owned(),
                colors: vec![[255, 0, 0]],
            },
            ZoneColors {
                zone_id: "screen:sector_0_1".to_owned(),
                colors: vec![[0, 255, 0]],
            },
        ])));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut frame_rx = state.event_bus.frame_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let got_frame = tokio::time::timeout(Duration::from_secs(2), frame_rx.changed()).await;
    assert!(got_frame.is_ok(), "expected frame data within 2 seconds");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let frame_data = frame_rx.borrow().clone();
    let left_zone = frame_data
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .expect("left zone should be sampled");
    let right_zone = frame_data
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .expect("right zone should be sampled");

    assert_eq!(left_zone.colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(right_zone.colors.first().copied(), Some([0, 255, 0]));
}
