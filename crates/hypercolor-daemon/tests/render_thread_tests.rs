//! Integration tests for the render thread and frame pipeline.
//!
//! These tests prove that the render thread correctly orchestrates:
//! Effect render → Spatial sample → Device push → Bus publish.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig, MockEffectRenderer};
use hypercolor_core::device::{
    BackendManager, DeviceBackend, DeviceLifecycleManager, DeviceRegistry, ReconnectPolicy,
    UsbProtocolConfigStore,
};
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::{InputData, InputManager, InputSource, ScreenData};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::attachment_profiles::AttachmentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::event::{HypercolorEvent, ZoneColors};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

use hypercolor_daemon::discovery::DiscoveryRuntime;
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::render_thread::{RenderThread, RenderThreadState};
use hypercolor_daemon::session::OutputPowerState;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn test_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test".into(),
        name: "Test Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        groups: vec![],
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
        group_id: None,
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
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
    }
}

fn point_zone(id: &str, device_id: &str, x: f32, y: f32) -> DeviceZone {
    DeviceZone {
        id: id.into(),
        name: id.into(),
        device_id: device_id.into(),
        zone_name: None,
        group_id: None,
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
        attachment: None,
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
    let (_, power_state) = watch::channel(OutputPowerState::default());
    RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::new(HypercolorBus::new()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        input_manager: Arc::new(Mutex::new(InputManager::new())),
        power_state,
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
            "device-settings.json",
        )))),
        canvas_width: 320,
        canvas_height: 200,
        screen_capture_enabled: false,
    }
}

async fn wait_for_device_state(
    lifecycle_manager: &Arc<Mutex<DeviceLifecycleManager>>,
    device_id: DeviceId,
    expected: DeviceState,
    timeout: Duration,
) {
    let result = tokio::time::timeout(timeout, async {
        loop {
            let state = {
                let lifecycle = lifecycle_manager.lock().await;
                lifecycle.state(device_id)
            };
            if state == Some(expected.clone()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "expected device {device_id} to reach {expected:?} within {timeout:?}"
    );
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
async fn pipeline_publishes_canvas_data_via_watch() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut canvas_rx = state.event_bus.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    let result = tokio::time::timeout(Duration::from_secs(1), canvas_rx.changed()).await;
    assert!(result.is_ok(), "expected canvas data within 1 second");
    let canvas = canvas_rx.borrow().clone();
    assert_eq!(canvas.width, 320);
    assert_eq!(canvas.height, 200);

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

    let (_, power_state) = watch::channel(OutputPowerState::default());
    let state = RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::new(HypercolorBus::new()),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        input_manager: Arc::new(Mutex::new(InputManager::new())),
        power_state,
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
            "device-settings.json",
        )))),
        canvas_width: 320,
        canvas_height: 200,
        screen_capture_enabled: false,
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
#[expect(
    clippy::too_many_lines,
    reason = "this integration test exercises the full reconnect flow through render, write failure detection, and lifecycle recovery"
)]
async fn pipeline_async_write_failures_enter_reconnect_flow() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Failing Strip".into(),
        led_count: 8,
        topology: LedTopology::Strip {
            count: 8,
            direction: StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    let info = backend
        .device_infos()
        .first()
        .cloned()
        .expect("mock backend should expose one device");
    let layout_device_id = DeviceLifecycleManager::layout_device_id("mock", &info);

    backend.connect(&device_id).await.expect("connect");
    backend.fail_write = true;

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(backend));
    backend_manager.map_device(&layout_device_id, "mock", device_id);
    let backend_manager = Arc::new(Mutex::new(backend_manager));

    let device_registry = DeviceRegistry::new();
    let registered_id = device_registry.add(info.clone()).await;
    assert_eq!(registered_id, device_id);

    let lifecycle_manager = Arc::new(Mutex::new(DeviceLifecycleManager::with_reconnect_policy(
        ReconnectPolicy {
            initial_delay: Duration::from_secs(5),
            ..ReconnectPolicy::default()
        },
    )));
    {
        let mut lifecycle = lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, "mock", None);
        lifecycle
            .on_connected(device_id)
            .expect("connected state should be valid");
        lifecycle
            .on_frame_success(device_id)
            .expect("frame success should move device to active");
    }

    let layout = test_layout(vec![strip_zone("zone_0", &layout_device_id, 8)]);
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout)));
    let event_bus = Arc::new(HypercolorBus::new());
    let discovery_runtime = DiscoveryRuntime {
        device_registry: device_registry.clone(),
        backend_manager: Arc::clone(&backend_manager),
        lifecycle_manager: Arc::clone(&lifecycle_manager),
        reconnect_tasks: Arc::new(StdMutex::new(HashMap::new())),
        event_bus: Arc::clone(&event_bus),
        spatial_engine: Arc::clone(&spatial_engine),
        layouts: Arc::new(RwLock::new(HashMap::new())),
        layouts_path: PathBuf::from("layouts.json"),
        layout_auto_exclusions: Arc::new(RwLock::new(HashMap::new())),
        logical_devices: Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new())),
        attachment_registry: Arc::new(RwLock::new(AttachmentRegistry::new())),
        attachment_profiles: Arc::new(RwLock::new(AttachmentProfileStore::new(PathBuf::from(
            "attachment-profiles.json",
        )))),
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
            "device-settings.json",
        )))),
        runtime_state_path: PathBuf::from("runtime-state.json"),
        usb_protocol_configs: UsbProtocolConfigStore::new(),
        in_progress: Arc::new(AtomicBool::new(false)),
        task_spawner: tokio::runtime::Handle::current(),
    };

    let mut effect_engine = EffectEngine::new();
    let renderer = MockEffectRenderer::solid(255, 0, 0);
    let metadata = MockEffectRenderer::sample_metadata("write-failure");
    effect_engine
        .activate(Box::new(renderer), metadata)
        .expect("activate");

    let (_, power_state) = watch::channel(OutputPowerState::default());
    let state = RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        spatial_engine,
        backend_manager,
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: Some(discovery_runtime.clone()),
        event_bus,
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        input_manager: Arc::new(Mutex::new(InputManager::new())),
        power_state,
        device_settings: Arc::new(RwLock::new(DeviceSettingsStore::new(PathBuf::from(
            "device-settings.json",
        )))),
        canvas_width: 320,
        canvas_height: 200,
        screen_capture_enabled: false,
    };

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    wait_for_device_state(
        &lifecycle_manager,
        device_id,
        DeviceState::Reconnecting,
        Duration::from_millis(750),
    )
    .await;

    let registry_state = tokio::time::timeout(Duration::from_millis(250), async {
        loop {
            let tracked = device_registry
                .get(&device_id)
                .await
                .expect("device should remain in registry");
            if tracked.state == DeviceState::Reconnecting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        registry_state.is_ok(),
        "expected registry state to sync to reconnecting"
    );

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let reconnect_tasks = {
        let mut tasks = discovery_runtime
            .reconnect_tasks
            .lock()
            .expect("reconnect task map lock poisoned");
        tasks.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
    };
    for handle in reconnect_tasks {
        handle.abort();
    }
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

#[tokio::test]
async fn idle_pipeline_throttles_even_with_watch_receivers() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();
    let _canvas_rx = state.event_bus.canvas_receiver();
    let _spectrum_rx = state.event_bus.spectrum_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let first_frame = tokio::time::timeout(Duration::from_secs(1), frame_rx.changed()).await;
    assert!(
        first_frame.is_ok(),
        "expected initial black frame before idle throttling"
    );
    let _ = frame_rx.borrow_and_update();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let got_extra_frame = frame_rx
        .has_changed()
        .expect("frame watch should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    assert!(
        !got_extra_frame,
        "expected idle pipeline to stop publishing repeated frames"
    );
}
