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

#[cfg(feature = "wgpu")]
use tokio::sync::oneshot;
use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{
    BackendManager, DeviceBackend, DeviceLifecycleManager, DeviceRegistry, ReconnectPolicy,
    UsbProtocolConfigStore,
};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::{InputData, InputManager, InputSource, ScreenData};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::attachment_profiles::AttachmentProfileStore;
use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_driver_api::CredentialStore;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::effect::{ControlValue, EffectId, EffectMetadata};
use hypercolor_types::event::{
    FrameData, HypercolorEvent, InputButtonState, InputEvent, ZoneColors,
};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    DisplayFaceTarget, RenderGroup, RenderGroupId, RenderGroupRole, UnassignedBehavior,
};
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

use hypercolor_daemon::discovery::DiscoveryRuntime;
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::{PreviewPixelFormat, PreviewRuntime, PreviewStreamDemand};
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
        brightness: None,
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
        brightness: None,
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

fn builtin_effect_metadata(registry: &EffectRegistry, stem: &str) -> EffectMetadata {
    registry
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.metadata.clone())
        })
        .expect("builtin effect should exist")
}

#[derive(Clone)]
struct ActiveEffectSeed {
    metadata: Option<EffectMetadata>,
    controls: HashMap<String, ControlValue>,
    preset_id: Option<PresetId>,
}

fn idle_effect() -> ActiveEffectSeed {
    ActiveEffectSeed {
        metadata: None,
        controls: HashMap::new(),
        preset_id: None,
    }
}

fn active_builtin_effect(stem: &str, controls: HashMap<String, ControlValue>) -> ActiveEffectSeed {
    let registry = builtin_effect_registry();
    let metadata = builtin_effect_metadata(&registry, stem);
    ActiveEffectSeed {
        metadata: Some(metadata),
        controls,
        preset_id: None,
    }
}

fn solid_color_controls(r: u8, g: u8, b: u8) -> HashMap<String, ControlValue> {
    HashMap::from([(
        "color".into(),
        ControlValue::Color([
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ]),
    )])
}

fn primary_group(
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    layout: SpatialLayout,
) -> RenderGroup {
    RenderGroup {
        id: RenderGroupId::new(),
        name: "Primary".into(),
        description: None,
        effect_id: Some(effect_id),
        controls,
        control_bindings: HashMap::new(),
        preset_id: None,
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Primary,
        controls_version: 0,
    }
}

fn custom_group(
    name: &str,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    layout: SpatialLayout,
) -> RenderGroup {
    RenderGroup {
        id: RenderGroupId::new(),
        name: name.into(),
        description: None,
        effect_id: Some(effect_id),
        controls,
        control_bindings: HashMap::new(),
        preset_id: None,
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: RenderGroupRole::Custom,
        controls_version: 0,
    }
}

fn display_group(
    group_id: RenderGroupId,
    device_id: DeviceId,
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    layout: SpatialLayout,
) -> RenderGroup {
    RenderGroup {
        id: group_id,
        name: "Display".into(),
        description: None,
        effect_id: Some(effect_id),
        controls,
        control_bindings: HashMap::new(),
        preset_id: None,
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(DisplayFaceTarget::new(device_id)),
        role: RenderGroupRole::Display,
        controls_version: 0,
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

#[cfg(feature = "wgpu")]
struct SequencedScreenPreviewSource {
    running: bool,
    pending_frames: VecDeque<ScreenData>,
    last_frame: ScreenData,
}

#[cfg(feature = "wgpu")]
impl SequencedScreenPreviewSource {
    fn new(frames: Vec<ScreenData>) -> Self {
        let pending_frames: VecDeque<_> = frames.into();
        let last_frame = pending_frames
            .back()
            .cloned()
            .expect("sequenced screen preview source requires at least one frame");
        Self {
            running: false,
            pending_frames,
            last_frame,
        }
    }
}

#[cfg(feature = "wgpu")]
impl InputSource for SequencedScreenPreviewSource {
    fn name(&self) -> &'static str {
        "sequenced_screen_preview"
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

        let frame = self
            .pending_frames
            .pop_front()
            .unwrap_or_else(|| self.last_frame.clone());
        Ok(InputData::Screen(frame))
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

#[cfg(feature = "wgpu")]
async fn wait_for_next_frame_with_watchdog<F>(
    rx: &mut watch::Receiver<FrameData>,
    previous_frame_number: u32,
    on_timeout: F,
) -> FrameData
where
    F: Fn(&FrameData) -> String,
{
    let (deadline_tx, mut deadline_rx) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = deadline_tx.send(());
    });

    let mut latest_frame = rx.borrow().clone();
    loop {
        tokio::select! {
            changed = rx.changed() => {
                changed.expect("frame sender should remain connected");
                let frame = rx.borrow().clone();
                latest_frame = frame.clone();
                if frame.frame_number > previous_frame_number {
                    return frame;
                }
            }
            _ = &mut deadline_rx => {
                panic!("{}", on_timeout(&latest_frame));
            }
        }
    }
}

async fn wait_for_frame_where<F>(rx: &mut watch::Receiver<FrameData>, predicate: F) -> FrameData
where
    F: Fn(&FrameData) -> bool,
{
    let mut last_frame = None;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            rx.changed()
                .await
                .expect("frame sender should remain connected");
            let frame = rx.borrow().clone();
            last_frame = Some(frame.clone());
            if predicate(&frame) {
                break frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        let details = last_frame.as_ref().map_or_else(
            || "no frame observed".to_owned(),
            |frame| {
                let zone_ids = frame
                    .zones
                    .iter()
                    .map(|zone| zone.zone_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "last frame_number={} zone_ids=[{}]",
                    frame.frame_number, zone_ids
                )
            },
        );
        panic!("expected a matching frame within 2 seconds: {details}");
    })
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

#[cfg(feature = "wgpu")]
async fn wait_for_render_loop_frame_number(
    state: &RenderThreadState,
    minimum_frame_number: u64,
) -> u64 {
    let start = std::time::Instant::now();
    loop {
        let frame_number = {
            let render_loop = state.render_loop.read().await;
            render_loop.frame_number()
        };
        if frame_number >= minimum_frame_number {
            return frame_number;
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "expected render loop frame_number to reach {minimum_frame_number} within 2 seconds, got {frame_number}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(feature = "wgpu")]
fn preview_screen_data(left: [u8; 3], right: [u8; 3], frame_number: u32) -> ScreenData {
    let mut preview_canvas = Canvas::new(320, 200);
    for y in 0..200 {
        for x in 0..320 {
            let rgb = if x < 160 { left } else { right };
            preview_canvas.set_pixel(x, y, Rgba::new(rgb[0], rgb[1], rgb[2], 255));
        }
    }

    ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(PublishedSurface::from_owned_canvas(
            preview_canvas,
            frame_number,
            frame_number.saturating_mul(10),
        )),
        source_width: 320,
        source_height: 200,
    }
}

fn make_render_state(
    active_effect: ActiveEffectSeed,
    spatial_engine: SpatialEngine,
    backend_manager: BackendManager,
) -> RenderThreadState {
    let (_, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let mut scene_manager = SceneManager::with_default();
    if let Some(metadata) = active_effect.metadata.as_ref() {
        scene_manager
            .upsert_primary_group(
                metadata,
                active_effect.controls.clone(),
                active_effect.preset_id,
                spatial_engine.layout().as_ref().clone(),
            )
            .expect("test render state should seed a default primary group");
    }
    RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine: Arc::new(RwLock::new(spatial_engine)),
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(scene_manager)),
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
        idle_effect(),
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
        idle_effect(),
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
        idle_effect(),
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
        idle_effect(),
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

    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(32, 64, 255)),
        SpatialEngine::new(layout),
        backend_manager,
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
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(24, 32, 48)),
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
        let metadata = {
            let registry = state.effect_registry.read().await;
            builtin_effect_metadata(&registry, "audio_pulse")
        };
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(&metadata, HashMap::new(), None, test_layout(Vec::new()))
            .expect("activate audio-reactive primary group");
    }

    wait_for_audio_capture_transition(&transitions, true).await;

    {
        let metadata = {
            let registry = state.effect_registry.read().await;
            builtin_effect_metadata(&registry, "solid_color")
        };
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .upsert_primary_group(
                &metadata,
                solid_color_controls(8, 16, 24),
                None,
                test_layout(Vec::new()),
            )
            .expect("reactivate non-audio primary group");
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
        idle_effect(),
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
    let state = make_render_state(
        idle_effect(),
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
    let state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(vec![strip_zone("zone_0", "mock:strip", 8)])),
        BackendManager::new(),
    );
    let mut canvas_rx = state.event_bus.canvas_receiver();

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };

    let mut scene_a = make_scene("Scene A");
    scene_a.groups = vec![primary_group(
        solid_id,
        solid_color_controls(255, 0, 0),
        test_layout(vec![strip_zone("zone_0", "mock:strip", 8)]),
    )];
    let mut scene_b = make_scene("Scene B");
    scene_b.transition.duration_ms = 5_000;
    scene_b.groups = vec![primary_group(
        solid_id,
        solid_color_controls(0, 0, 255),
        test_layout(vec![strip_zone("zone_0", "mock:strip", 8)]),
    )];
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
}

#[tokio::test]
async fn pipeline_renders_active_scene_groups_without_global_effect_engine() {
    let state = make_render_state(
        idle_effect(),
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
        custom_group(
            "Left",
            solid_id,
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            test_layout(vec![point_zone("zone_left", "mock:left", 0.25, 0.5)]),
        ),
        custom_group(
            "Right",
            solid_id,
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
            test_layout(vec![point_zone("zone_right", "mock:right", 0.75, 0.5)]),
        ),
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

    let frame = wait_for_frame_where(&mut frame_rx, |frame| {
        frame.zones.iter().any(|zone| zone.zone_id == "zone_left")
            && frame.zones.iter().any(|zone| zone.zone_id == "zone_right")
    })
    .await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

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
async fn multi_group_scene_publishes_authoritative_canvas_and_scene_canvas() {
    let state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut canvas_rx = state.event_bus.canvas_receiver();
    let mut scene_canvas_rx = state.event_bus.scene_canvas_receiver();

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };

    let mut scene = make_scene("Grouped Canvas Scene");
    scene.groups = vec![
        custom_group(
            "Left",
            solid_id,
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            test_layout(vec![point_zone("zone_left", "mock:left", 0.25, 0.5)]),
        ),
        custom_group(
            "Right",
            solid_id,
            HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
            test_layout(vec![point_zone("zone_right", "mock:right", 0.75, 0.5)]),
        ),
    ];
    scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene.clone())
            .expect("create grouped canvas scene");
        scene_manager
            .activate(&scene.id, None)
            .expect("activate grouped canvas scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), canvas_rx.changed())
        .await
        .expect("expected grouped scene canvas within 2 seconds")
        .expect("canvas sender should remain connected");
    tokio::time::timeout(Duration::from_secs(2), scene_canvas_rx.changed())
        .await
        .expect("expected grouped scene authoritative scene canvas within 2 seconds")
        .expect("scene canvas sender should remain connected");

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let canvas = canvas_rx.borrow().clone();
    let scene_canvas = scene_canvas_rx.borrow().clone();

    for frame in [&canvas, &scene_canvas] {
        assert_eq!(frame.width, 320);
        assert_eq!(frame.height, 200);
        assert_eq!(
            frame.surface().get_pixel(80, 100),
            Rgba::new(255, 0, 0, 255)
        );
        assert_eq!(
            frame.surface().get_pixel(240, 100),
            Rgba::new(0, 0, 255, 255)
        );
        assert_eq!(frame.surface().get_pixel(160, 100), Rgba::new(0, 0, 0, 255));
    }
}

#[tokio::test]
async fn late_group_canvas_subscribers_see_last_display_face_frame() {
    let state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut frame_rx = state.event_bus.frame_receiver();

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let group_id = RenderGroupId::new();
    let display_id = DeviceId::new();

    let mut scene = make_scene("Display Face Scene");
    scene.groups = vec![display_group(
        group_id,
        display_id,
        solid_id,
        HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
        test_layout(Vec::new()),
    )];
    scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene.clone())
            .expect("create display face scene");
        scene_manager
            .activate(&scene.id, None)
            .expect("activate display face scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let group_canvas_sender = state.event_bus.group_canvas_sender(group_id);
    let mut rt = RenderThread::spawn(state.clone());
    let mut published_group_rx = group_canvas_sender.subscribe();
    let _ = wait_for_next_frame(&mut frame_rx, 0).await;
    tokio::time::timeout(Duration::from_secs(2), published_group_rx.changed())
        .await
        .expect("display face canvas should publish within timeout")
        .expect("display face canvas stream should stay open");
    let group_rx = group_canvas_sender.subscribe();
    let frame = group_rx.borrow().clone();
    assert_eq!(frame.width, 320);
    assert_eq!(frame.height, 200);
    assert_eq!(&frame.rgba_bytes()[0..4], [0, 0, 255, 255].as_slice());
    let (_, published_targets) = state.event_bus.display_group_targets_snapshot();
    let published_target = published_targets
        .get(&group_id)
        .expect("display group target metadata should publish with the face frame");
    assert_eq!(published_target.device_id, display_id);
    assert_eq!(
        published_target.blend_mode,
        hypercolor_types::scene::DisplayFaceBlendMode::Replace
    );
    assert_eq!(published_target.opacity, 1.0);

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
}

#[cfg(feature = "wgpu")]
#[tokio::test]
async fn blended_display_faces_publish_authoritative_scene_canvas_on_gpu() {
    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let group_id = RenderGroupId::new();
    let display_id = DeviceId::new();

    let mut face_group = display_group(
        group_id,
        display_id,
        solid_id,
        HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
        test_layout(Vec::new()),
    );
    face_group
        .display_target
        .as_mut()
        .expect("display group should carry a display target")
        .blend_mode = hypercolor_types::scene::DisplayFaceBlendMode::Difference;

    let mut scene = make_scene("GPU Display Face Scene");
    scene.groups = vec![
        custom_group(
            "Primary",
            solid_id,
            HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
            test_layout(Vec::new()),
        ),
        face_group,
    ];
    scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(scene.clone())
            .expect("create gpu display face scene");
        scene_manager
            .activate(&scene.id, None)
            .expect("activate gpu display face scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut scene_canvas_rx = state.event_bus.scene_canvas_receiver();
    let group_canvas_sender = state.event_bus.group_canvas_sender(group_id);
    let mut group_canvas_rx = group_canvas_sender.subscribe();
    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), scene_canvas_rx.changed())
        .await
        .expect("authoritative scene canvas should publish within timeout")
        .expect("scene canvas stream should stay open");
    tokio::time::timeout(Duration::from_secs(2), group_canvas_rx.changed())
        .await
        .expect("display face canvas should publish within timeout")
        .expect("display face canvas stream should stay open");

    let scene_frame = scene_canvas_rx.borrow().clone();
    let face_frame = group_canvas_rx.borrow().clone();

    assert_eq!(scene_frame.width, 320);
    assert_eq!(scene_frame.height, 200);
    assert_eq!(
        scene_frame.surface().get_pixel(160, 100),
        Rgba::new(255, 0, 0, 255)
    );
    assert_eq!(face_frame.width, 320);
    assert_eq!(face_frame.height, 200);
    assert_eq!(&face_frame.rgba_bytes()[0..4], [0, 0, 255, 255].as_slice());

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn render_thread_prunes_stale_group_canvas_streams_when_face_groups_change() {
    let state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );
    let mut frame_rx = state.event_bus.frame_receiver();

    let solid_id = {
        let registry = state.effect_registry.read().await;
        builtin_effect_id(&registry, "solid_color")
    };
    let first_group_id = RenderGroupId::new();
    let second_group_id = RenderGroupId::new();
    let display_id = DeviceId::new();

    let mut first_scene = make_scene("Face Scene A");
    first_scene.groups = vec![display_group(
        first_group_id,
        display_id,
        solid_id,
        HashMap::from([("color".into(), ControlValue::Color([1.0, 0.0, 0.0, 1.0]))]),
        test_layout(Vec::new()),
    )];
    first_scene.unassigned_behavior = UnassignedBehavior::Off;

    let mut second_scene = make_scene("Face Scene B");
    second_scene.groups = vec![display_group(
        second_group_id,
        display_id,
        solid_id,
        HashMap::from([("color".into(), ControlValue::Color([0.0, 0.0, 1.0, 1.0]))]),
        test_layout(Vec::new()),
    )];
    second_scene.unassigned_behavior = UnassignedBehavior::Off;

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .create(first_scene.clone())
            .expect("create first face scene");
        scene_manager
            .create(second_scene.clone())
            .expect("create second face scene");
        scene_manager
            .activate(&first_scene.id, None)
            .expect("activate first face scene");
    }

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());
    let first_frame = wait_for_next_frame(&mut frame_rx, 0).await;
    assert!(first_frame.frame_number > 0);
    assert_eq!(state.event_bus.group_canvas_stream_count(), 1);
    let (_, first_targets) = state.event_bus.display_group_targets_snapshot();
    assert_eq!(state.event_bus.display_group_target_count(), 1);
    assert!(first_targets.contains_key(&first_group_id));

    {
        let mut scene_manager = state.scene_manager.write().await;
        scene_manager
            .activate(&second_scene.id, None)
            .expect("activate second face scene");
    }

    let second_frame = wait_for_next_frame(&mut frame_rx, first_frame.frame_number).await;
    assert!(second_frame.frame_number > first_frame.frame_number);
    assert_eq!(state.event_bus.group_canvas_stream_count(), 1);
    let (_, second_targets) = state.event_bus.display_group_targets_snapshot();
    assert_eq!(state.event_bus.display_group_target_count(), 1);
    assert!(!second_targets.contains_key(&first_group_id));
    assert!(second_targets.contains_key(&second_group_id));

    let stale_rx = state.event_bus.group_canvas_receiver(first_group_id);
    let stale_frame = stale_rx.borrow().clone();
    assert_eq!(stale_frame.width, 0);
    assert_eq!(stale_frame.height, 0);

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn audio_capture_enabled_when_any_active_group_is_reactive() {
    let state = make_render_state(
        idle_effect(),
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
    audio_scene.groups = vec![primary_group(
        audio_pulse_id,
        HashMap::new(),
        test_layout(vec![point_zone("zone_audio", "mock:audio", 0.5, 0.5)]),
    )];
    audio_scene.unassigned_behavior = UnassignedBehavior::Off;

    let mut solid_scene = make_scene("Solid Scene");
    solid_scene.groups = vec![primary_group(
        solid_id,
        HashMap::new(),
        test_layout(vec![point_zone("zone_audio", "mock:audio", 0.5, 0.5)]),
    )];
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
        idle_effect(),
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
    screen_scene.groups = vec![primary_group(
        screen_cast_id,
        HashMap::new(),
        test_layout(vec![point_zone("zone_screen", "mock:screen", 0.5, 0.5)]),
    )];
    screen_scene.unassigned_behavior = UnassignedBehavior::Off;

    let mut solid_scene = make_scene("Solid Scene");
    solid_scene.groups = vec![primary_group(
        solid_id,
        HashMap::new(),
        test_layout(vec![point_zone("zone_screen", "mock:screen", 0.5, 0.5)]),
    )];
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
        idle_effect(),
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
    let layout = test_layout(vec![point_zone("zone_main", "mock:main", 0.5, 0.5)]);
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(255, 0, 0)),
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
    assert_eq!(zone.colors.first().copied(), Some([255, 0, 0]));
}

#[tokio::test]
async fn pipeline_publishes_canvas_data_via_watch() {
    let state = make_render_state(
        idle_effect(),
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
        idle_effect(),
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
async fn effect_engine_removal_does_not_break_single_group_fast_path() {
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

    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(255, 0, 0)),
        spatial_engine,
        backend_manager,
    );

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
async fn primary_group_canvas_published_to_canvas_channel() {
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(255, 0, 0)),
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
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(255, 0, 0)),
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
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(255, 0, 0)),
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
    let layout_device_id = DeviceLifecycleManager::layout_device_id(&info);

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
        let _ = lifecycle.on_discovered(device_id, &info, None);
        lifecycle
            .on_connected(device_id)
            .expect("connected state should be valid");
        lifecycle
            .on_frame_success(device_id)
            .expect("frame success should move device to active");
    }

    let layout = test_layout(vec![strip_zone("zone_0", &layout_device_id, 8)]);
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout.clone())));
    let event_bus = Arc::new(HypercolorBus::new());
    let discovery_runtime = DiscoveryRuntime {
        device_registry: device_registry.clone(),
        backend_manager: Arc::clone(&backend_manager),
        lifecycle_manager: Arc::clone(&lifecycle_manager),
        reconnect_tasks: Arc::new(StdMutex::new(HashMap::new())),
        event_bus: Arc::clone(&event_bus),
        spatial_engine: Arc::clone(&spatial_engine),
        scene_manager: Arc::new(RwLock::new(SceneManager::with_default())),
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

    let effect_seed = active_builtin_effect("solid_color", solid_color_controls(255, 0, 0));
    let mut scene_manager = SceneManager::with_default();
    let metadata = effect_seed
        .metadata
        .clone()
        .expect("builtin effect should expose metadata");
    scene_manager
        .upsert_primary_group(
            &metadata,
            effect_seed.controls.clone(),
            effect_seed.preset_id,
            layout.clone(),
        )
        .expect("failing-device test should seed a primary group");

    let (_, power_state) = watch::channel(OutputPowerState::default());
    let state = RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine,
        backend_manager,
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: Some(discovery_runtime.clone()),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(scene_manager)),
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
        idle_effect(),
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

    // With no active groups and no zones, the idle pipeline stays black.
    let frame_data = frame_rx.borrow().clone();
    assert!(frame_data.zones.is_empty());
}

#[tokio::test]
async fn pipeline_uses_screen_input_canvas_when_available() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(layout),
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
        idle_effect(),
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
        idle_effect(),
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

#[cfg(feature = "wgpu")]
#[expect(
    clippy::too_many_lines,
    reason = "GPU retained-screen coverage needs full render-thread setup"
)]
#[tokio::test]
async fn pipeline_gpu_retained_screen_preview_advances_frame_watch_when_input_stalls() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

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
    let source_surface = PublishedSurface::from_owned_canvas(preview_canvas, 19, 31);
    let screen_data = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(source_surface),
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

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected initial sampled GPU frame within 2 seconds")
        .expect("frame sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    let retained_frame = wait_for_next_frame_with_watchdog(
        &mut frame_rx,
        initial_frame.frame_number,
        |latest_frame| {
        let loop_frame_number = state
            .render_loop
            .try_read()
            .map_or(u64::MAX, |render_loop| render_loop.frame_number());
            format!(
            "expected the next GPU retained frame within 2 seconds: render_loop.frame_number={} latest_watch_frame_number={} latest_watch_zone_count={}",
            loop_frame_number,
            latest_frame.frame_number,
            latest_frame.zones.len()
        )
        },
    )
    .await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    let (deadline_tx, mut deadline_rx) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = deadline_tx.send(());
    });
    tokio::select! {
        shutdown = rt.shutdown() => shutdown.expect("shutdown"),
        _ = &mut deadline_rx => panic!("render thread should stop within 2 seconds"),
    }

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

    assert!(retained_frame.frame_number > initial_frame.frame_number);
    assert_eq!(initial_left, [255, 0, 0]);
    assert_eq!(initial_right, [0, 255, 0]);
    assert_eq!(retained_left, [255, 0, 0]);
    assert_eq!(retained_right, [0, 255, 0]);
}

#[cfg(feature = "wgpu")]
#[allow(
    clippy::too_many_lines,
    reason = "fresh GPU deferred-sampling coverage needs full render-thread setup"
)]
#[tokio::test]
async fn pipeline_gpu_fresh_screen_preview_does_not_publish_stale_colors_while_sampling_defers() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

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
    let source_surface = PublishedSurface::from_owned_canvas(preview_canvas, 23, 41);
    let screen_data = ScreenData {
        zone_colors: Vec::new(),
        grid_width: 0,
        grid_height: 0,
        canvas_downscale: Some(source_surface),
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

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(2), frame_rx.changed())
        .await
        .expect("expected initial sampled GPU frame within 2 seconds")
        .expect("frame sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    let loop_frame_number = wait_for_render_loop_frame_number(&state, 2).await;
    let current_frame = frame_rx.borrow().clone();

    if current_frame.frame_number == initial_frame.frame_number {
        assert!(
            !frame_rx
                .has_changed()
                .expect("frame sender should remain connected"),
            "expected fresh deferred GPU sampling to keep frame watch quiet while render_loop.frame_number advanced to {loop_frame_number}"
        );
    }

    let current_left = current_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .and_then(|zone| zone.colors.first().copied())
        .expect("current left sample should exist");
    let current_right = current_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .and_then(|zone| zone.colors.first().copied())
        .expect("current right sample should exist");
    assert_eq!(
        current_left,
        [255, 0, 0],
        "expected deferred GPU sampling to avoid publishing a stale left-zone color while render_loop.frame_number advanced to {loop_frame_number}"
    );
    assert_eq!(
        current_right,
        [0, 255, 0],
        "expected deferred GPU sampling to avoid publishing a stale right-zone color while render_loop.frame_number advanced to {loop_frame_number}"
    );

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    let (deadline_tx, mut deadline_rx) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = deadline_tx.send(());
    });
    tokio::select! {
        shutdown = rt.shutdown() => shutdown.expect("shutdown"),
        _ = &mut deadline_rx => panic!("render thread should stop within 2 seconds"),
    }
}

#[cfg(feature = "wgpu")]
#[expect(
    clippy::too_many_lines,
    reason = "fresh GPU latest-wins coverage needs full render-thread setup"
)]
#[tokio::test]
async fn pipeline_gpu_fresh_screen_preview_publishes_latest_colors_after_deferred_sampling() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

    let initial_screen = preview_screen_data([255, 0, 0], [0, 255, 0], 1);
    let intermediate_screen = preview_screen_data([0, 0, 255], [255, 255, 0], 2);
    let latest_screen = preview_screen_data([0, 255, 255], [255, 0, 255], 3);

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(SequencedScreenPreviewSource::new(vec![
            initial_screen,
            intermediate_screen,
            latest_screen,
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
        .expect("expected initial sampled GPU frame within 2 seconds")
        .expect("frame sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    wait_for_render_loop_frame_number(&state, 3).await;
    let expected_left = [0, 255, 255];
    let expected_right = [255, 0, 255];

    let latest_frame = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            frame_rx
                .changed()
                .await
                .expect("frame sender should remain connected");
            let frame = frame_rx.borrow().clone();
            let left = frame
                .zones
                .iter()
                .find(|zone| zone.zone_id == "zone_left")
                .and_then(|zone| zone.colors.first().copied());
            let right = frame
                .zones
                .iter()
                .find(|zone| zone.zone_id == "zone_right")
                .and_then(|zone| zone.colors.first().copied());
            if frame.frame_number > initial_frame.frame_number
                && left == Some(expected_left)
                && right == Some(expected_right)
            {
                break frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        let loop_frame_number = state
            .render_loop
            .try_read()
            .map_or(0, |render_loop| render_loop.frame_number());
        let performance_debug = state
            .performance
            .try_read()
            .map(|metrics| format!("{metrics:?}"))
            .unwrap_or_else(|_| "unavailable".to_owned());
        let last_frame = frame_rx.borrow().clone();
        panic!(
            "expected deferred GPU sampling to publish the newest screen colors within 2 seconds: render_loop.frame_number={} last_watch_frame_number={} last_watch_zone_count={} performance={}",
            loop_frame_number,
            last_frame.frame_number,
            last_frame.zones.len(),
            performance_debug,
        );
    });

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    let (deadline_tx, mut deadline_rx) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = deadline_tx.send(());
    });
    tokio::select! {
        shutdown = rt.shutdown() => shutdown.expect("shutdown"),
        _ = &mut deadline_rx => panic!("render thread should stop within 2 seconds"),
    }

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
    let latest_left = latest_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .and_then(|zone| zone.colors.first().copied())
        .expect("latest left sample should exist");
    let latest_right = latest_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .and_then(|zone| zone.colors.first().copied())
        .expect("latest right sample should exist");

    assert_eq!(initial_left, [255, 0, 0]);
    assert_eq!(initial_right, [0, 255, 0]);
    assert_eq!(latest_left, [0, 255, 255]);
    assert_eq!(latest_right, [255, 0, 255]);
}

#[cfg(feature = "wgpu")]
#[expect(
    clippy::too_many_lines,
    reason = "sustained fresh-frame GPU latest-wins coverage needs full render-thread setup"
)]
#[tokio::test]
async fn pipeline_gpu_fresh_screen_preview_keeps_latest_wins_under_sustained_updates() {
    let layout = test_layout(vec![
        point_zone("zone_left", "mock:left", 0.25, 0.5),
        point_zone("zone_right", "mock:right", 0.75, 0.5),
    ]);

    let mut state = make_render_state(
        idle_effect(),
        SpatialEngine::new(layout),
        BackendManager::new(),
    );
    state.screen_capture_configured = true;
    state.render_acceleration_mode = RenderAccelerationMode::Gpu;

    let screens = vec![
        preview_screen_data([255, 0, 0], [0, 255, 0], 1),
        preview_screen_data([0, 0, 255], [255, 255, 0], 2),
        preview_screen_data([0, 255, 255], [255, 0, 255], 3),
        preview_screen_data([255, 128, 0], [0, 128, 255], 4),
        preview_screen_data([32, 224, 96], [224, 32, 160], 5),
        preview_screen_data([255, 255, 255], [16, 32, 48], 6),
    ];

    {
        let mut input_manager = state.input_manager.lock().await;
        input_manager.add_source(Box::new(SequencedScreenPreviewSource::new(screens)));
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
        .expect("expected initial sampled GPU frame within 2 seconds")
        .expect("frame sender should remain connected");

    let initial_frame = frame_rx.borrow().clone();
    wait_for_render_loop_frame_number(&state, 6).await;
    let expected_left = [255, 255, 255];
    let expected_right = [16, 32, 48];

    let latest_frame = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            frame_rx
                .changed()
                .await
                .expect("frame sender should remain connected");
            let frame = frame_rx.borrow().clone();
            let left = frame
                .zones
                .iter()
                .find(|zone| zone.zone_id == "zone_left")
                .and_then(|zone| zone.colors.first().copied());
            let right = frame
                .zones
                .iter()
                .find(|zone| zone.zone_id == "zone_right")
                .and_then(|zone| zone.colors.first().copied());
            if frame.frame_number > initial_frame.frame_number
                && left == Some(expected_left)
                && right == Some(expected_right)
            {
                break frame;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        let loop_frame_number = state
            .render_loop
            .try_read()
            .map_or(0, |render_loop| render_loop.frame_number());
        let last_frame = frame_rx.borrow().clone();
        panic!(
            "expected sustained deferred GPU sampling to publish the newest screen colors within 2 seconds: render_loop.frame_number={} last_watch_frame_number={} last_watch_zone_count={}",
            loop_frame_number,
            last_frame.frame_number,
            last_frame.zones.len(),
        );
    });

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    let (deadline_tx, mut deadline_rx) = oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = deadline_tx.send(());
    });
    tokio::select! {
        shutdown = rt.shutdown() => shutdown.expect("shutdown"),
        _ = &mut deadline_rx => panic!("render thread should stop within 2 seconds"),
    }

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
    let latest_left = latest_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_left")
        .and_then(|zone| zone.colors.first().copied())
        .expect("latest left sample should exist");
    let latest_right = latest_frame
        .zones
        .iter()
        .find(|zone| zone.zone_id == "zone_right")
        .and_then(|zone| zone.colors.first().copied())
        .expect("latest right sample should exist");

    assert_eq!(initial_left, [255, 0, 0]);
    assert_eq!(initial_right, [0, 255, 0]);
    assert_eq!(latest_left, expected_left);
    assert_eq!(latest_right, expected_right);
}

#[tokio::test]
async fn pipeline_applies_queued_layout_changes_on_the_next_frame() {
    let mut state = make_render_state(
        idle_effect(),
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
        idle_effect(),
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
        idle_effect(),
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
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(24, 32, 48)),
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
        idle_effect(),
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
    let state = make_render_state(
        active_builtin_effect("solid_color", solid_color_controls(64, 32, 255)),
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

#[tokio::test]
async fn pipeline_throttles_canvas_preview_publication_to_tracked_receiver_fps() {
    let state = make_render_state(
        active_builtin_effect("rainbow", HashMap::new()),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    let mut preview_rx = state.preview_runtime.canvas_receiver();
    preview_rx.update_demand(PreviewStreamDemand {
        fps: 5,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 360,
    });

    {
        let mut rl = state.render_loop.write().await;
        rl.start();
    }

    let mut rt = RenderThread::spawn(state.clone());

    tokio::time::timeout(Duration::from_secs(1), preview_rx.changed())
        .await
        .expect("expected initial preview publication within 1 second")
        .expect("preview sender should remain connected");
    let _ = preview_rx.borrow_and_update();

    tokio::time::sleep(Duration::from_millis(450)).await;

    {
        let mut rl = state.render_loop.write().await;
        rl.stop();
    }
    rt.shutdown().await.expect("shutdown");

    let preview_snapshot = state.preview_runtime.snapshot();
    assert!(
        preview_snapshot.canvas_frames_published <= 3,
        "expected low-fps preview demand to gate source publication, got {} canvas publications",
        preview_snapshot.canvas_frames_published
    );
    assert!(
        preview_snapshot.latest_canvas_frame_number
            > preview_snapshot.canvas_frames_published as u32,
        "expected preview telemetry to keep advancing even when source publication is throttled"
    );
}

#[test]
fn preview_runtime_receivers_share_event_bus_canvas_channel() {
    let state = make_render_state(
        idle_effect(),
        SpatialEngine::new(test_layout(Vec::new())),
        BackendManager::new(),
    );

    assert_eq!(state.event_bus.canvas_receiver_count(), 0);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 0);
    assert_eq!(state.preview_runtime.tracked_canvas_receiver_count(), 0);

    let _direct_rx = state.event_bus.canvas_receiver();
    assert_eq!(state.event_bus.canvas_receiver_count(), 1);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 0);
    assert_eq!(state.preview_runtime.tracked_canvas_receiver_count(), 0);

    let _preview_rx = state.preview_runtime.canvas_receiver();
    assert_eq!(state.event_bus.canvas_receiver_count(), 2);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 1);

    let _internal_preview_rx =
        state
            .preview_runtime
            .internal_canvas_receiver(PreviewStreamDemand {
                fps: 30,
                format: PreviewPixelFormat::Rgba,
                width: 0,
                height: 0,
            });
    assert_eq!(state.event_bus.canvas_receiver_count(), 3);
    assert_eq!(state.preview_runtime.canvas_receiver_count(), 1);
    assert_eq!(state.preview_runtime.tracked_canvas_receiver_count(), 2);
}

#[tokio::test]
async fn release_sleep_clears_published_frame_and_canvas_once() {
    let layout = test_layout(vec![strip_zone("zone_0", "mock:strip", 8)]);
    let effect_seed = active_builtin_effect("solid_color", solid_color_controls(255, 0, 0));
    let mut scene_manager = SceneManager::with_default();
    let metadata = effect_seed
        .metadata
        .clone()
        .expect("builtin effect should expose metadata");
    scene_manager
        .upsert_primary_group(
            &metadata,
            effect_seed.controls.clone(),
            effect_seed.preset_id,
            layout.clone(),
        )
        .expect("release-sleep test should seed a primary group");

    let (power_tx, power_state) = watch::channel(OutputPowerState::default());
    let event_bus = Arc::new(HypercolorBus::new());
    let state = RenderThreadState {
        effect_registry: Arc::new(RwLock::new(builtin_effect_registry())),
        spatial_engine: Arc::new(RwLock::new(SpatialEngine::new(layout))),
        backend_manager: Arc::new(Mutex::new(BackendManager::new())),
        device_registry: DeviceRegistry::new(),
        performance: Arc::new(RwLock::new(PerformanceTracker::default())),
        discovery_runtime: None,
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
        render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
        scene_manager: Arc::new(RwLock::new(scene_manager)),
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
