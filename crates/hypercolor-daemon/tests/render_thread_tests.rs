//! Integration tests for the render thread and frame pipeline.
//!
//! These tests prove that the render thread correctly orchestrates:
//! Effect render → Spatial sample → Device push → Bus publish.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig, MockEffectRenderer};
use hypercolor_core::device::net::CredentialStore;
use hypercolor_core::device::{
    BackendManager, DeviceBackend, DeviceLifecycleManager, DeviceRegistry, ReconnectPolicy,
    UsbProtocolConfigStore,
};
use hypercolor_core::effect::{EffectEngine, EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::{InputData, InputManager, InputSource, ScreenData};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::attachment_profiles::AttachmentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::event::{
    FrameData, HypercolorEvent, InputButtonState, InputEvent, ZoneColors,
};
use hypercolor_types::scene::{RenderGroup, RenderGroupId, UnassignedBehavior};
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

use hypercolor_daemon::discovery::DiscoveryRuntime;
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use hypercolor_daemon::scene_transactions::{SceneTransaction, SceneTransactionQueue};
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
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
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
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
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
        .find_map(|(id, entry)| (entry.metadata.source.source_stem() == Some(stem)).then_some(*id))
        .expect("builtin effect should exist")
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
            grid_width: 0,
            grid_height: 0,
            canvas_downscale: None,
            source_width: 0,
            source_height: 0,
        }))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct MockScreenPreviewSource {
    running: bool,
    screen_data: ScreenData,
}

impl MockScreenPreviewSource {
    fn new(screen_data: ScreenData) -> Self {
        Self {
            running: false,
            screen_data,
        }
    }
}

impl InputSource for MockScreenPreviewSource {
    fn name(&self) -> &'static str {
        "mock_screen_preview"
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

        Ok(InputData::Screen(self.screen_data.clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct BurstyScreenPreviewSource {
    running: bool,
    next_screen_data: Option<ScreenData>,
}

impl BurstyScreenPreviewSource {
    fn new(screen_data: ScreenData) -> Self {
        Self {
            running: false,
            next_screen_data: Some(screen_data),
        }
    }
}

impl InputSource for BurstyScreenPreviewSource {
    fn name(&self) -> &'static str {
        "bursty_screen_preview"
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

        if let Some(screen_data) = self.next_screen_data.take() {
            return Ok(InputData::Screen(screen_data));
        }

        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct MockAudioSource {
    running: bool,
    audio: AudioData,
}

impl MockAudioSource {
    fn new(audio: AudioData) -> Self {
        Self {
            running: false,
            audio,
        }
    }
}

impl InputSource for MockAudioSource {
    fn name(&self) -> &'static str {
        "mock_audio"
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

        Ok(InputData::Audio(self.audio.clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct DemandGatedMockAudioSource {
    running: bool,
    capture_active: bool,
    audio: AudioData,
    transitions: Arc<StdMutex<Vec<bool>>>,
}

impl DemandGatedMockAudioSource {
    fn new(audio: AudioData, transitions: Arc<StdMutex<Vec<bool>>>) -> Self {
        Self {
            running: false,
            capture_active: false,
            audio,
            transitions,
        }
    }
}

impl InputSource for DemandGatedMockAudioSource {
    fn name(&self) -> &'static str {
        "demand_gated_audio"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }
        if !self.capture_active {
            return Ok(InputData::Audio(AudioData::silence()));
        }

        Ok(InputData::Audio(self.audio.clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_audio_source(&self) -> bool {
        true
    }

    fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active = active;
        self.transitions
            .lock()
            .expect("transition log should lock")
            .push(active);
        Ok(())
    }
}

struct DemandGatedMockScreenSource {
    running: bool,
    capture_active: bool,
    screen_data: ScreenData,
    transitions: Arc<StdMutex<Vec<bool>>>,
}

impl DemandGatedMockScreenSource {
    fn new(screen_data: ScreenData, transitions: Arc<StdMutex<Vec<bool>>>) -> Self {
        Self {
            running: false,
            capture_active: false,
            screen_data,
            transitions,
        }
    }
}

impl InputSource for DemandGatedMockScreenSource {
    fn name(&self) -> &'static str {
        "demand_gated_screen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running || !self.capture_active {
            return Ok(InputData::None);
        }

        Ok(InputData::Screen(self.screen_data.clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active = active;
        self.transitions
            .lock()
            .expect("transition log should lock")
            .push(active);
        Ok(())
    }
}

struct EventOnlySource {
    running: bool,
    events: Vec<InputEvent>,
}

impl EventOnlySource {
    fn new(events: Vec<InputEvent>) -> Self {
        Self {
            running: false,
            events,
        }
    }
}

impl InputSource for EventOnlySource {
    fn name(&self) -> &'static str {
        "event_only"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn drain_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.events)
    }
}

async fn wait_for_audio_capture_transition(transitions: &Arc<StdMutex<Vec<bool>>>, expected: bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let seen = transitions
                .lock()
                .expect("transition log should lock")
                .last()
                .copied();
            if seen == Some(expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected audio capture transition");
}

async fn wait_for_screen_capture_transition(
    transitions: &Arc<StdMutex<Vec<bool>>>,
    expected: bool,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let seen = transitions
                .lock()
                .expect("transition log should lock")
                .last()
                .copied();
            if seen == Some(expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected screen capture transition");
}

async fn wait_for_next_frame(
    rx: &mut watch::Receiver<FrameData>,
    previous_frame_number: u32,
) -> FrameData {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            rx.changed()
                .await
                .expect("frame sender should remain connected");
            let frame = rx.borrow().clone();
            if frame.frame_number > previous_frame_number {
                break frame;
            }
        }
    })
    .await
    .expect("expected the next frame within 2 seconds")
}

async fn wait_for_next_canvas_frame(
    rx: &mut watch::Receiver<CanvasFrame>,
    previous_frame_number: u32,
) -> CanvasFrame {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            rx.changed()
                .await
                .expect("canvas sender should remain connected");
            let frame = rx.borrow().clone();
            if frame.frame_number > previous_frame_number {
                break frame;
            }
        }
    })
    .await
    .expect("expected the next canvas frame within 2 seconds")
}

fn make_render_state(
    effect_engine: EffectEngine,
    spatial_engine: SpatialEngine,
    backend_manager: BackendManager,
) -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(SceneManager::new())),
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

#[cfg(not(feature = "wgpu"))]
#[tokio::test]
async fn render_thread_try_spawn_rejects_explicit_gpu_without_feature() {
    let mut state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

    let Err(error) = RenderThread::try_spawn(state) else {
        panic!("explicit gpu mode should fail before the render thread starts");
    };
    assert!(format!("{error:#}").contains("rebuild hypercolor-daemon with the `wgpu` feature"));
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

#[tokio::test]
async fn render_thread_publishes_discrete_input_events() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(EventOnlySource::new(vec![InputEvent::Key {
            source_id: "host:/dev/input/event4".into(),
            key: "a".into(),
            state: InputButtonState::Pressed,
        }])));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut event_rx = state.event_bus.subscribe_all();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    let input_event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match event_rx.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::InputEventReceived { event } = timestamped.event {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before input event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for input event");

    assert_eq!(
        input_event,
        InputEvent::Key {
            source_id: "host:/dev/input/event4".into(),
            key: "a".into(),
            state: InputButtonState::Pressed,
        }
    );

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }

    rt.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn render_thread_publishes_audio_level_updates_for_active_effects() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Audio Strip".into(),
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
    backend_manager.map_device("mock:audio-strip", "mock", device_id);

    let layout = test_layout(vec![strip_zone("zone_audio", "mock:audio-strip", 10)]);

    let mut effect_engine = EffectEngine::new();
    let renderer = MockEffectRenderer::solid(32, 64, 255);
    let metadata = MockEffectRenderer::sample_metadata("audio-event");
    effect_engine
        .activate(Box::new(renderer), metadata)
        .expect("activate");

    let state = make_render_state(effect_engine, SpatialEngine::new(layout), backend_manager);

    let mut audio = AudioData::silence();
    audio.rms_level = 0.42;
    audio.beat_detected = true;
    audio.beat_confidence = 0.9;
    for value in &mut audio.spectrum[..40] {
        *value = 0.8;
    }
    for value in &mut audio.spectrum[40..130] {
        *value = 0.4;
    }
    for value in &mut audio.spectrum[130..] {
        *value = 0.2;
    }

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(MockAudioSource::new(audio)));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut event_rx = state.event_bus.subscribe_all();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    let audio_event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match event_rx.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::AudioLevelUpdate {
                        level,
                        bass,
                        mid,
                        treble,
                        beat,
                    } = timestamped.event
                    {
                        break (level, bass, mid, treble, beat);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before audio level update");
                }
            }
        }
    })
    .await
    .expect("expected audio level update within 2 seconds");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let (level, bass, mid, treble, beat) = audio_event;
    assert!((level - 0.42).abs() < f32::EPSILON);
    assert!(bass > mid);
    assert!(mid > treble);
    assert!(beat);
}

#[tokio::test]
async fn render_thread_gates_audio_capture_to_audio_reactive_effects() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(24, 32, 48)),
            MockEffectRenderer::sample_metadata("solid-idle"),
        )
        .expect("activate non-audio effect");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut audio = AudioData::silence();
    audio.rms_level = 0.7;
    let transitions = Arc::new(StdMutex::new(Vec::new()));

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(DemandGatedMockAudioSource::new(
            audio,
            Arc::clone(&transitions),
        )));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    wait_for_audio_capture_transition(&transitions, false).await;

    {
        let mut engine = state.effect_engine.lock().await;
        let mut metadata = MockEffectRenderer::sample_metadata("audio-burst");
        metadata.audio_reactive = true;
        engine
            .activate(
                Box::new(MockEffectRenderer::audio_reactive(255, 64, 32)),
                metadata,
            )
            .expect("activate audio-reactive effect");
    }

    wait_for_audio_capture_transition(&transitions, true).await;

    {
        let mut engine = state.effect_engine.lock().await;
        engine
            .activate(
                Box::new(MockEffectRenderer::solid(8, 16, 24)),
                MockEffectRenderer::sample_metadata("solid-return"),
            )
            .expect("reactivate non-audio effect");
    }

    wait_for_audio_capture_transition(&transitions, false).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let transitions = transitions
        .lock()
        .expect("transition log should lock")
        .clone();
    assert_eq!(transitions, vec![false, true, false]);
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
async fn render_thread_advances_active_scene_transitions() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("scene-transition"),
        )
        .expect("activate");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(vec![strip_zone("zone_0", "mock:strip", 8)])),
        BackendManager::new(),
    );

    let scene_a = make_scene("Scene A");
    let scene_b = make_scene("Scene B");
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene_a.clone())
            .expect("create scene a");
        scene_manager
            .create(scene_b.clone())
            .expect("create scene b");
        scene_manager
            .activate(&scene_a.id, None)
            .expect("activate scene a");
        scene_manager
            .activate(&scene_b.id, None)
            .expect("activate scene b");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    tokio::time::sleep(Duration::from_millis(120)).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let scene_manager = state.scene_manager.read().await;
    let transition = scene_manager
        .active_transition()
        .expect("scene transition should still be active");
    assert!(
        transition.progress > 0.0,
        "render thread should advance scene transitions on the frame clock"
    );
    assert_eq!(scene_manager.active_scene_id(), Some(&scene_b.id));
}

#[tokio::test]
async fn render_thread_crossfades_scene_transition_between_effect_frames() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("scene-transition-red"),
        )
        .expect("activate red");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(vec![strip_zone("zone_0", "mock:strip", 8)])),
        BackendManager::new(),
    );
    let mut canvas_rx = state.event_bus.canvas_receiver();

    let scene_a = make_scene("Scene A");
    let scene_b = make_scene("Scene B");
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene_a.clone())
            .expect("create scene a");
        scene_manager
            .create(scene_b.clone())
            .expect("create scene b");
        scene_manager
            .activate(&scene_a.id, None)
            .expect("activate scene a");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("timed out waiting for initial canvas")
        .expect("canvas sender should remain connected");
    let initial_canvas = canvas_rx.borrow().clone();
    let initial_pixel = &initial_canvas.rgba_bytes()[0..4];
    assert_eq!(initial_pixel, [255, 0, 0, 255].as_slice());

    {
        let mut engine = state.effect_engine.lock().await;
        engine
            .activate(
                Box::new(MockEffectRenderer::solid(0, 0, 255)),
                MockEffectRenderer::sample_metadata("scene-transition-blue"),
            )
            .expect("activate blue");
    }
    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .activate(&scene_b.id, None)
            .expect("activate scene b");
    }

    let blended_canvas =
        wait_for_next_canvas_frame(&mut canvas_rx, initial_canvas.frame_number).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let blended_pixel = &blended_canvas.rgba_bytes()[0..4];
    assert_ne!(blended_pixel, [255, 0, 0, 255].as_slice());
    assert_ne!(blended_pixel, [0, 0, 255, 255].as_slice());
}

#[tokio::test]
async fn pipeline_renders_active_scene_groups_without_global_effect_engine() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut frame_rx = state.event_bus.frame_receiver();

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };

    let mut scene = make_scene("Grouped Scene");
    scene.groups = vec![
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Left".into(),
            description: None,
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            preset_id: None,
            layout: test_layout(vec![point_zone("zone_left", "mock:left", 0.5, 0.5)]),
            brightness: 1.0,
            enabled: true,
            color: None,
        },
        RenderGroup {
            id: RenderGroupId::new(),
            name: "Right".into(),
            description: None,
            effect_id: Some(solid_id),
            controls: HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
            preset_id: None,
            layout: test_layout(vec![point_zone("zone_right", "mock:right", 0.5, 0.5)]),
            brightness: 1.0,
            enabled: true,
            color: None,
        },
    ];
    scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene.clone())
            .expect("create grouped scene");
        scene_manager
            .activate(&scene.id, None)
            .expect("activate grouped scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected grouped frame within 2 seconds")
        .expect("frame sender should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let frame = frame_rx.borrow().clone();
    let left_zone = frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .expect("left group zone should be rendered");
    let right_zone = frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .expect("right group zone should be rendered");

    assert_eq!(left_zone.colors.first().copied(), Some([255, 0, 0]));
    assert_eq!(right_zone.colors.first().copied(), Some([0, 0, 255]));
}

#[tokio::test]
async fn render_thread_gates_audio_capture_to_audio_reactive_scene_groups() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let transitions = Arc::new(StdMutex::new(Vec::new()));

    let mut audio = AudioData::silence();
    audio.rms_level = 0.7;
    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(DemandGatedMockAudioSource::new(
            audio,
            Arc::clone(&transitions),
        )));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let (audio_pulse_id, solid_id) = {
        let registry = state.effect_registry.read().await;
        (
            builtin_effect_id(&registry, "audio_pulse"),
            builtin_effect_id(&registry, "solid_color"),
        )
    };

    let mut audio_scene = make_scene("Audio Scene");
    audio_scene.groups = vec![RenderGroup {
        id: RenderGroupId::new(),
        name: "Audio".into(),
        description: None,
        effect_id: Some(audio_pulse_id),
        controls: HashMap::new(),
        preset_id: None,
        layout: test_layout(vec![point_zone("zone_audio", "mock:audio", 0.5, 0.5)]),
        brightness: 1.0,
        enabled: true,
        color: None,
    }];
    audio_scene.unassigned_behavior = UnassignedBehavior::Off;

    let mut solid_scene = make_scene("Solid Scene");
    solid_scene.groups = vec![RenderGroup {
        id: RenderGroupId::new(),
        name: "Solid".into(),
        description: None,
        effect_id: Some(solid_id),
        controls: HashMap::new(),
        preset_id: None,
        layout: test_layout(vec![point_zone("zone_audio", "mock:audio", 0.5, 0.5)]),
        brightness: 1.0,
        enabled: true,
        color: None,
    }];
    solid_scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(audio_scene.clone())
            .expect("create audio scene");
        scene_manager
            .create(solid_scene.clone())
            .expect("create solid scene");
        scene_manager
            .activate(&audio_scene.id, None)
            .expect("activate audio scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    wait_for_audio_capture_transition(&transitions, true).await;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .activate(&solid_scene.id, None)
            .expect("activate solid scene");
    }

    wait_for_audio_capture_transition(&transitions, false).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let transitions = transitions
        .lock()
        .expect("transition log should lock")
        .clone();
    assert_eq!(transitions, vec![true, false]);
}

#[tokio::test]
async fn render_thread_gates_screen_capture_to_screen_reactive_scene_groups() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let transitions = Arc::new(StdMutex::new(Vec::new()));
    let preview_surface = PublishedSurface::from_owned_canvas(Canvas::new(2, 2), 5, 9);
    let screen_data = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(preview_surface),
        source_width: 2,
        source_height: 2,
    };

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(DemandGatedMockScreenSource::new(
            screen_data,
            Arc::clone(&transitions),
        )));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let (screen_cast_id, solid_id) = {
        let registry = state.effect_registry.read().await;
        (
            builtin_effect_id(&registry, "screen_cast"),
            builtin_effect_id(&registry, "solid_color"),
        )
    };

    let mut screen_scene = make_scene("Screen Scene");
    screen_scene.groups = vec![RenderGroup {
        id: RenderGroupId::new(),
        name: "Screen".into(),
        description: None,
        effect_id: Some(screen_cast_id),
        controls: HashMap::new(),
        preset_id: None,
        layout: test_layout(vec![point_zone("zone_screen", "mock:screen", 0.5, 0.5)]),
        brightness: 1.0,
        enabled: true,
        color: None,
    }];
    screen_scene.unassigned_behavior = UnassignedBehavior::Off;

    let mut solid_scene = make_scene("Solid Scene");
    solid_scene.groups = vec![RenderGroup {
        id: RenderGroupId::new(),
        name: "Solid".into(),
        description: None,
        effect_id: Some(solid_id),
        controls: HashMap::new(),
        preset_id: None,
        layout: test_layout(vec![point_zone("zone_screen", "mock:screen", 0.5, 0.5)]),
        brightness: 1.0,
        enabled: true,
        color: None,
    }];
    solid_scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(screen_scene.clone())
            .expect("create screen scene");
        scene_manager
            .create(solid_scene.clone())
            .expect("create solid scene");
        scene_manager
            .activate(&screen_scene.id, None)
            .expect("activate screen scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    wait_for_screen_capture_transition(&transitions, true).await;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .activate(&solid_scene.id, None)
            .expect("activate solid scene");
    }

    wait_for_screen_capture_transition(&transitions, false).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let transitions = transitions
        .lock()
        .expect("transition log should lock")
        .clone();
    assert_eq!(transitions, vec![true, false]);
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
async fn pipeline_keeps_latest_frame_hot_for_late_subscribers() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 32, 128)),
            MockEffectRenderer::sample_metadata("late-frame-subscriber"),
        )
        .expect("activate");

    let layout = test_layout(vec![point_zone("zone_main", "mock:main", 0.5, 0.5)]);
    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(layout),
        BackendManager::new(),
    );

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let frame_rx = state.event_bus.frame_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let frame_data = frame_rx.borrow().clone();
    assert!(
        frame_data.timestamp_ms > 0 || frame_data.frame_number > 0,
        "late subscribers should see the current frame immediately"
    );
    let zone = frame_data
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_main")
        .expect("late subscriber should see sampled zones");
    assert_eq!(zone.colors.first().copied(), Some([255, 32, 128]));
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
async fn pipeline_publishes_canvas_data_via_preview_runtime() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut canvas_rx = state.preview_runtime.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    let result = tokio::time::timeout(Duration::from_secs(1), canvas_rx.changed()).await;
    assert!(
        result.is_ok(),
        "expected preview runtime canvas data within 1 second"
    );
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
    let event_bus = Arc::new(HypercolorBus::new());
    let state = RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(SceneManager::new())),
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
async fn pipeline_publishes_slot_backed_canvas_for_active_effects() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("slot-backed-canvas"),
        )
        .expect("activate");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut canvas_rx = state.event_bus.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("expected active-effect canvas within 2 seconds")
        .expect("canvas sender should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let canvas = canvas_rx.borrow().clone();
    assert_eq!(canvas.width, 320);
    assert_eq!(canvas.height, 200);
    assert!(
        canvas.surface().generation() > 0,
        "active effect canvas should come from the render surface pool"
    );
}

#[tokio::test]
async fn pipeline_keeps_slot_backed_canvas_when_recent_frames_are_retained() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("retained-slot-backed-canvas"),
        )
        .expect("activate");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut canvas_rx = state.event_bus.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let mut retained_frames = VecDeque::new();

    for _ in 0..6 {
        tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
            .await
            .expect("expected retained-frame canvas within 2 seconds")
            .expect("canvas sender should remain connected");

        let canvas = canvas_rx.borrow().clone();
        assert!(
            canvas.surface().generation() > 0,
            "active effect canvas should stay slot-backed even when recent frames are retained"
        );
        retained_frames.push_back(canvas);
        if retained_frames.len() > 4 {
            let _ = retained_frames.pop_front();
        }
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pipeline_keeps_slot_backed_canvas_with_multiple_receivers() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("multi-receiver-slot-backed-canvas"),
        )
        .expect("activate");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut primary_canvas_rx = state.event_bus.canvas_receiver();
    let mut secondary_canvas_rx = state.event_bus.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let mut primary_retained_frames = VecDeque::new();
    let mut secondary_retained_frames = VecDeque::new();

    for _ in 0..8 {
        tokio::time::timeout(Duration::from_secs(2), primary_canvas_rx.changed())
            .await
            .expect("expected primary receiver canvas within 2 seconds")
            .expect("primary canvas sender should remain connected");
        tokio::time::timeout(Duration::from_secs(2), secondary_canvas_rx.changed())
            .await
            .expect("expected secondary receiver canvas within 2 seconds")
            .expect("secondary canvas sender should remain connected");

        let primary_canvas = primary_canvas_rx.borrow().clone();
        let secondary_canvas = secondary_canvas_rx.borrow().clone();
        assert!(
            primary_canvas.surface().generation() > 0,
            "primary receiver should keep receiving slot-backed canvases"
        );
        assert!(
            secondary_canvas.surface().generation() > 0,
            "secondary receiver should keep receiving slot-backed canvases"
        );

        primary_retained_frames.push_back(primary_canvas);
        secondary_retained_frames.push_back(secondary_canvas);
        if primary_retained_frames.len() > 3 {
            let _ = primary_retained_frames.pop_front();
        }
        if secondary_retained_frames.len() > 3 {
            let _ = secondary_retained_frames.pop_front();
        }
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
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
        scene_transactions: SceneTransactionQueue::default(),
        runtime_state_path: PathBuf::from("runtime-state.json"),
        usb_protocol_configs: UsbProtocolConfigStore::new(),
        credential_store: Arc::new(
            CredentialStore::open_blocking(&std::env::temp_dir().join(format!(
                "hypercolor-test-credentials-{}",
                uuid::Uuid::now_v7()
            )))
            .expect("test credential store"),
        ),
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
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine,
        backend_manager,
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: Some(discovery_runtime.clone()),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(SceneManager::new())),
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
async fn pipeline_reuses_screen_preview_surface_for_canvas_and_screen_watch() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;

    let mut preview_canvas = Canvas::new(320, 200);
    for y in 0..200 {
        for x in 0..320 {
            let color = if x < 160 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 255, 0, 255)
            };
            preview_canvas.set_pixel(x, y, color);
        }
    }
    let source_surface = PublishedSurface::from_owned_canvas(preview_canvas, 41, 77);
    let screen_data = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(source_surface.clone()),
        source_width: 320,
        source_height: 200,
    };

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(MockScreenPreviewSource::new(screen_data)));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut frame_rx = state.event_bus.frame_receiver();
    let mut canvas_rx = state.event_bus.canvas_receiver();
    let mut screen_canvas_rx = state.event_bus.screen_canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected sampled frame within 2 seconds")
        .expect("frame sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("expected published canvas within 2 seconds")
        .expect("canvas sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), screen_canvas_rx.changed())
        .await
        .expect("expected screen preview canvas within 2 seconds")
        .expect("screen canvas sender should remain connected");

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

    let published_canvas = canvas_rx.borrow().clone();
    let published_screen = screen_canvas_rx.borrow().clone();
    let source_ptr = source_surface.rgba_bytes().as_ptr();
    assert_eq!(published_canvas.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(published_screen.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(
        published_canvas.rgba_bytes().as_ptr(),
        published_screen.rgba_bytes().as_ptr()
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "End-to-end screen preview retention coverage needs full pipeline setup"
)]
#[tokio::test]
async fn pipeline_retains_screen_preview_surface_when_input_stalls() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;

    let mut preview_canvas = Canvas::new(320, 200);
    for y in 0..200 {
        for x in 0..320 {
            let color = if x < 160 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 255, 0, 255)
            };
            preview_canvas.set_pixel(x, y, color);
        }
    }
    let source_surface = PublishedSurface::from_owned_canvas(preview_canvas, 11, 22);
    let screen_data = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(source_surface.clone()),
        source_width: 320,
        source_height: 200,
    };

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(BurstyScreenPreviewSource::new(screen_data)));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut frame_rx = state.event_bus.frame_receiver();
    let mut canvas_rx = state.event_bus.canvas_receiver();
    let mut screen_canvas_rx = state.event_bus.screen_canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected initial sampled frame within 2 seconds")
        .expect("frame sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("expected initial canvas within 2 seconds")
        .expect("canvas sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), screen_canvas_rx.changed())
        .await
        .expect("expected initial screen canvas within 2 seconds")
        .expect("screen canvas sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    let initial_canvas = canvas_rx.borrow().clone();
    let initial_screen = screen_canvas_rx.borrow().clone();

    let retained_frame = wait_for_next_frame(&mut frame_rx, initial_frame.frame_number).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let canvas_changed = canvas_rx
        .has_changed()
        .expect("canvas watch should remain connected");
    let screen_canvas_changed = screen_canvas_rx
        .has_changed()
        .expect("screen canvas watch should remain connected");
    let retained_canvas = canvas_rx.borrow().clone();
    let retained_screen = screen_canvas_rx.borrow().clone();
    let preview_snapshot = state.preview_runtime.snapshot();

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let source_ptr = source_surface.rgba_bytes().as_ptr();
    let initial_left = initial_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .and_then(|zone| zone.colors.first().copied())
        .expect("initial left sample should exist");
    let initial_right = initial_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .and_then(|zone| zone.colors.first().copied())
        .expect("initial right sample should exist");
    let retained_left = retained_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .and_then(|zone| zone.colors.first().copied())
        .expect("retained left sample should exist");
    let retained_right = retained_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .and_then(|zone| zone.colors.first().copied())
        .expect("retained right sample should exist");

    assert_eq!(initial_left, [255, 0, 0]);
    assert_eq!(initial_right, [0, 255, 0]);
    assert_eq!(retained_left, [255, 0, 0]);
    assert_eq!(retained_right, [0, 255, 0]);
    assert!(
        !canvas_changed,
        "expected retained preview surfaces to stop republishing metadata-only canvas updates"
    );
    assert!(
        !screen_canvas_changed,
        "expected retained preview surfaces to stop republishing metadata-only screen preview updates"
    );
    assert_eq!(initial_canvas.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(retained_canvas.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(initial_screen.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(retained_screen.rgba_bytes().as_ptr(), source_ptr);
    assert_eq!(
        retained_canvas.rgba_bytes().as_ptr(),
        retained_screen.rgba_bytes().as_ptr()
    );
    assert!(preview_snapshot.latest_canvas_frame_number > initial_canvas.frame_number);
    assert!(preview_snapshot.latest_screen_canvas_frame_number > initial_screen.frame_number);
}

#[tokio::test]
async fn pipeline_applies_queued_layout_changes_on_the_next_frame() {
    let mut state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(vec![point_zone(
            "zone_sample",
            "mock:sample",
            0.25,
            0.5,
        )])),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;

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

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected initial frame within 2 seconds")
        .expect("frame sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    let initial_color = initial_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_sample")
        .and_then(|zone| zone.colors.first().copied())
        .expect("initial sampled color should exist");
    assert_eq!(initial_color, [255, 0, 0]);

    state
        .scene_transactions
        .push(SceneTransaction::ReplaceLayout(test_layout(vec![
            point_zone("zone_sample", "mock:sample", 0.75, 0.5),
        ])));

    let updated_color = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            frame_rx
                .changed()
                .await
                .expect("frame sender should remain connected");
            let frame = frame_rx.borrow().clone();
            let color = frame
                .zones
                .iter()
                .find(|zone| zone.zone_id == "zone_sample")
                .and_then(|zone| zone.colors.first().copied())
                .expect("updated sampled color should exist");
            if color != initial_color {
                break color;
            }
        }
    })
    .await
    .expect("expected queued layout update within 2 seconds");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    assert_eq!(updated_color, [0, 255, 0]);
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

#[tokio::test]
async fn idle_pipeline_skips_spectrum_publication_without_receivers() {
    let mut audio = AudioData::silence();
    audio.rms_level = 0.42;
    audio.beat_detected = true;
    audio.beat_confidence = 0.9;
    for value in &mut audio.spectrum[..40] {
        *value = 0.8;
    }
    for value in &mut audio.spectrum[40..130] {
        *value = 0.4;
    }
    for value in &mut audio.spectrum[130..] {
        *value = 0.2;
    }

    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();
    assert_eq!(state.event_bus.spectrum_receiver_count(), 0);

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(MockAudioSource::new(audio)));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let first_frame = tokio::time::timeout(Duration::from_secs(1), frame_rx.changed()).await;
    assert!(
        first_frame.is_ok(),
        "expected initial frame before idle throttling"
    );
    let _ = frame_rx.borrow_and_update();

    tokio::time::sleep(Duration::from_millis(200)).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let published_spectrum = state.event_bus.spectrum_sender().borrow().clone();
    assert_eq!(published_spectrum.timestamp_ms, 0);
    assert!(published_spectrum.level.abs() <= f32::EPSILON);
    assert_eq!(published_spectrum.bins.len(), 0);
}

#[tokio::test]
async fn render_thread_reuses_published_spectrum_bins_between_frames() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(24, 32, 48)),
            MockEffectRenderer::sample_metadata("spectrum-reuse"),
        )
        .expect("activate");

    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut audio = AudioData::silence();
    audio.rms_level = 0.42;
    audio.beat_detected = true;
    audio.beat_confidence = 0.9;
    for value in &mut audio.spectrum[..40] {
        *value = 0.8;
    }
    for value in &mut audio.spectrum[40..130] {
        *value = 0.4;
    }
    for value in &mut audio.spectrum[130..] {
        *value = 0.2;
    }

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(MockAudioSource::new(audio)));
        input_manager
            .start_all()
            .expect("input manager should start");
    }

    let mut spectrum_rx = state.event_bus.spectrum_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(1), spectrum_rx.changed())
        .await
        .expect("expected first spectrum frame within 1 second")
        .expect("spectrum watch should remain connected");
    let first_ptr = {
        let first = spectrum_rx.borrow_and_update();
        assert_eq!(first.bins.len(), 200);
        first.bins.as_ptr()
    };

    tokio::time::timeout(Duration::from_secs(1), spectrum_rx.changed())
        .await
        .expect("expected second spectrum frame within 1 second")
        .expect("spectrum watch should remain connected");
    let second_ptr = {
        let second = spectrum_rx.borrow_and_update();
        assert_eq!(second.bins.len(), 200);
        second.bins.as_ptr()
    };

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    assert_eq!(first_ptr, second_ptr);
}

#[tokio::test]
async fn idle_pipeline_does_not_republish_empty_screen_canvas_frames() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();
    let mut screen_canvas_rx = state.event_bus.screen_canvas_receiver();

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
    if screen_canvas_rx
        .has_changed()
        .expect("screen canvas watch should remain connected")
    {
        let _ = screen_canvas_rx.borrow_and_update();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    let screen_canvas_changed = screen_canvas_rx
        .has_changed()
        .expect("screen canvas watch should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    assert!(
        !screen_canvas_changed,
        "expected idle pipeline to stop republishing identical empty screen preview frames"
    );
}

#[tokio::test]
async fn idle_pipeline_skips_canvas_publication_without_receivers() {
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(64, 32, 255)),
            MockEffectRenderer::sample_metadata("canvas-idle"),
        )
        .expect("activate");
    let state = make_render_state(
        effect_engine,
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut frame_rx = state.event_bus.frame_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    let first_frame = tokio::time::timeout(Duration::from_secs(1), frame_rx.changed()).await;
    assert!(
        first_frame.is_ok(),
        "expected initial frame before idle throttling"
    );
    let _ = frame_rx.borrow_and_update();

    tokio::time::sleep(Duration::from_millis(200)).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let published_canvas = state.event_bus.canvas_sender().borrow().clone();
    let preview_snapshot = state.preview_runtime.snapshot();
    assert_eq!(published_canvas.width, 0);
    assert_eq!(published_canvas.height, 0);
    assert_eq!(preview_snapshot.canvas_frames_published, 0);
    assert!(preview_snapshot.latest_canvas_frame_number > 0);
}

#[test]
fn preview_runtime_receivers_share_event_bus_canvas_channel() {
    let state = make_render_state(
        EffectEngine::new(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    assert_eq!(state.event_bus.canvas_receiver_count(), 0);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 0);

    let _direct_rx = state.event_bus.canvas_receiver();
    assert_eq!(state.event_bus.canvas_receiver_count(), 1);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 0);

    let _preview_rx = state.preview_runtime.canvas_receiver();
    assert_eq!(state.event_bus.canvas_receiver_count(), 2);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 1);
}

#[tokio::test]
async fn release_sleep_clears_published_frame_and_canvas_once() {
    let layout = test_layout(vec![strip_zone("zone_0", "mock:strip", 8)]);
    let mut effect_engine = EffectEngine::new();
    effect_engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("release-sleep"),
        )
        .expect("activate");

    let (power_tx, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let state = RenderThreadState {
        effect_engine: Arc::new(Mutex::new(effect_engine)),
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(layout))),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(SceneManager::new())),
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
    };

    let mut frame_rx = state.event_bus.frame_receiver();
    let mut canvas_rx = state.event_bus.canvas_receiver();

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("timed out waiting for initial frame")
        .expect("frame sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("timed out waiting for initial canvas")
        .expect("canvas sender should remain connected");

    assert!(
        !frame_rx.borrow().zones.is_empty(),
        "initial render should publish sampled zone colors"
    );

    power_tx.send_replace(OutputPowerState {
        sleeping: true,
        session_brightness: 0.0,
        off_output_behavior: OffOutputBehavior::Release,
        ..OutputPowerState::default()
    });

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("timed out waiting for cleared frame")
        .expect("frame sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("timed out waiting for cleared canvas")
        .expect("canvas sender should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let cleared_frame = frame_rx.borrow().clone();
    assert!(
        cleared_frame.zones.is_empty(),
        "release sleep should clear the published zone frame"
    );

    let cleared_canvas = canvas_rx.borrow().clone();
    assert_eq!(cleared_canvas.width, 320);
    assert_eq!(cleared_canvas.height, 200);
    assert!(
        cleared_canvas
            .rgba_bytes()
            .chunks_exact(4)
            .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0),
        "release sleep should publish a blank canvas instead of the stale preview"
    );
}
