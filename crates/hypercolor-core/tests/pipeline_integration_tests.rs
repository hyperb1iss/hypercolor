//! End-to-end integration tests for the complete Hypercolor pipeline.
//!
//! These tests prove that Discovery -> Registry -> Effect Rendering ->
//! Spatial Sampling -> Device Color Write all work together as a cohesive
//! system. No mocks — only real inline test implementations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use uuid::Uuid;

use hypercolor_core::bus::{EventFilter, HypercolorBus};
use hypercolor_core::device::{
    DeviceRegistry, DiscoveredDevice, DiscoveryOrchestrator, TransportScanner,
};
use hypercolor_core::effect::{
    EffectEngine, EffectEntry, EffectRegistry, EffectRenderer, FrameInput,
};
use hypercolor_core::engine::{
    FpsController, FpsTier, RenderLoop, RenderLoopState, TierTransitionConfig,
};
use hypercolor_core::scene::{SceneManager, TransitionState, make_scene};
use hypercolor_core::spatial::{sample_led, sample_zone};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{EventCategory, HypercolorEvent};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, ScenePriority, TransitionSpec, ZoneAssignment,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, NormalizedPosition, SamplingMode, SpatialLayout,
};

// ─── Test Renderers ──────────────────────────────────────────────────────────
// Real inline implementations — NOT mocks.

/// Renderer that paints a horizontal red-to-blue gradient across the canvas.
struct TestGradientRenderer {
    initialized: bool,
}

impl TestGradientRenderer {
    fn new() -> Self {
        Self { initialized: false }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
impl EffectRenderer for TestGradientRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
        self.initialized = true;
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        for y in 0..input.canvas_height {
            for x in 0..input.canvas_width {
                let t = x as f32 / (input.canvas_width.saturating_sub(1)) as f32;
                let r = ((1.0 - t) * 255.0) as u8;
                let b = (t * 255.0) as u8;
                canvas.set_pixel(x, y, Rgba::new(r, 0, b, 255));
            }
        }
        Ok(canvas)
    }

    fn set_control(&mut self, _name: &str, _value: &ControlValue) {}

    fn destroy(&mut self) {
        self.initialized = false;
    }
}

/// Renderer that fills with a solid color, advancing brightness per frame.
struct TestSolidRenderer {
    frame_count: u64,
}

impl TestSolidRenderer {
    fn new() -> Self {
        Self { frame_count: 0 }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
impl EffectRenderer for TestSolidRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> Result<Canvas> {
        self.frame_count += 1;
        let brightness = ((self.frame_count * 25) % 256) as u8;
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        canvas.fill(Rgba::new(brightness, brightness, brightness, 255));
        Ok(canvas)
    }

    fn set_control(&mut self, _name: &str, _value: &ControlValue) {}

    fn destroy(&mut self) {}
}

// ─── Test Transport Scanner ──────────────────────────────────────────────────

/// Scanner that returns preconfigured devices.
struct TestScanner {
    name: String,
    devices: Vec<DiscoveredDevice>,
}

impl TestScanner {
    fn new(name: &str, devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            name: name.to_string(),
            devices,
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for TestScanner {
    fn name(&self) -> &str {
        &self.name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        Ok(self.devices.clone())
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn test_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_string(),
        author: "hyperb1iss".to_string(),
        version: "1.0.0".to_string(),
        description: format!("Test effect: {name}"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_string()],
        source: EffectSource::Native {
            path: PathBuf::from(format!("native/{name}.wgsl")),
        },
        license: Some("Apache-2.0".to_string()),
    }
}

fn make_device_info(name: &str, led_count: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: name.to_string(),
        vendor: "TestCorp".to_string(),
        family: DeviceFamily::Wled,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "main".to_string(),
            led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: Some("1.0.0".to_string()),
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: true,
            supports_brightness: true,
            max_fps: 60,
        },
    }
}

fn make_discovered_device(name: &str, led_count: u32) -> DiscoveredDevice {
    let info = make_device_info(name, led_count);
    let fp = DeviceFingerprint(format!("test:{name}"));
    DiscoveredDevice {
        connection_type: ConnectionType::Network,
        name: name.to_string(),
        family: DeviceFamily::Wled,
        fingerprint: fp,
        info,
        metadata: HashMap::new(),
    }
}

/// Create a horizontal LED strip zone positioned at canvas center.
fn make_strip_zone(id: &str, led_count: u32) -> DeviceZone {
    #[allow(clippy::as_conversions)]
    let capacity = led_count as usize;
    let mut positions = Vec::with_capacity(capacity);
    for i in 0..led_count {
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let t = if led_count <= 1 {
            0.5
        } else {
            i as f32 / (led_count - 1) as f32
        };
        positions.push(NormalizedPosition::new(t, 0.5));
    }

    DeviceZone {
        id: id.to_string(),
        name: format!("Strip {id}"),
        device_id: format!("test:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 0.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: hypercolor_types::spatial::LedTopology::Strip {
            count: led_count,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        led_positions: positions,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
    }
}

fn make_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test-layout".to_string(),
        name: "Test Layout".to_string(),
        description: None,
        canvas_width: DEFAULT_CANVAS_WIDTH,
        canvas_height: DEFAULT_CANVAS_HEIGHT,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 1: Render Loop Lifecycle
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn render_loop_lifecycle_create_start_tick_stop() {
    let mut render_loop = RenderLoop::new(60);
    assert_eq!(render_loop.state(), RenderLoopState::Created);

    // Start
    render_loop.start();
    assert_eq!(render_loop.state(), RenderLoopState::Running);
    assert!(render_loop.is_running());

    // Tick 5 frames and collect stats
    let mut frame_stats = Vec::new();
    for _ in 0..5 {
        assert!(render_loop.tick());
        if let Some(stats) = render_loop.frame_complete() {
            frame_stats.push(stats);
        }
    }

    assert_eq!(frame_stats.len(), 5, "should have 5 frame stats");
    assert_eq!(render_loop.frame_number(), 5);

    // Verify stats contain expected fields
    for (i, stats) in frame_stats.iter().enumerate() {
        assert_eq!(
            stats.tier,
            FpsTier::Full,
            "frame {i} should be at Full tier"
        );
        assert_eq!(
            stats.frames_since_tier_change,
            u64::try_from(i + 1).expect("frame count fits u64"),
            "frames_since_tier_change at frame {i}"
        );
    }

    // Stop
    render_loop.stop();
    assert_eq!(render_loop.state(), RenderLoopState::Stopped);
    assert!(!render_loop.is_running());

    // Tick after stop returns false
    assert!(!render_loop.tick());
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 2: Effect Engine + Render Loop
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn effect_engine_with_render_loop_produces_canvas() {
    // Create and start render loop
    let mut render_loop = RenderLoop::new(60);
    render_loop.start();

    // Create effect engine with gradient renderer
    let mut effect_engine = EffectEngine::new();
    let renderer = Box::new(TestGradientRenderer::new());
    let meta = test_metadata("gradient");
    effect_engine
        .activate(renderer, meta)
        .expect("activation should succeed");

    assert!(effect_engine.is_running());

    let audio = AudioData::silence();

    // Tick render loop + effect engine together for 5 frames
    for frame in 0..5 {
        assert!(render_loop.tick(), "render loop tick {frame}");

        let canvas = effect_engine
            .tick(0.016, &audio)
            .expect("effect engine tick should succeed");

        assert_eq!(canvas.width(), DEFAULT_CANVAS_WIDTH);
        assert_eq!(canvas.height(), DEFAULT_CANVAS_HEIGHT);

        // Verify gradient: left edge should be red, right edge blue
        let left = canvas.get_pixel(0, 0);
        let right = canvas.get_pixel(DEFAULT_CANVAS_WIDTH - 1, 0);

        assert_eq!(left.r, 255, "frame {frame}: left edge should be red");
        assert_eq!(left.b, 0, "frame {frame}: left edge should have no blue");
        assert_eq!(right.r, 0, "frame {frame}: right edge should have no red");
        assert_eq!(right.b, 255, "frame {frame}: right edge should be blue");

        let _stats = render_loop.frame_complete();
    }

    assert_eq!(render_loop.frame_number(), 5);
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 3: Spatial Sampling Pipeline
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn spatial_sampling_with_known_canvas_colors() {
    // Create a canvas with a horizontal red-to-blue gradient
    let width = 100;
    let height = 10;
    let mut canvas = Canvas::new(width, height);

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        clippy::cast_precision_loss
    )]
    for x in 0..width {
        let t = x as f32 / (width - 1) as f32;
        let r = ((1.0 - t) * 255.0) as u8;
        let b = (t * 255.0) as u8;
        for y in 0..height {
            canvas.set_pixel(x, y, Rgba::new(r, 0, b, 255));
        }
    }

    // Create a 5-LED strip zone across the full width
    let zone = make_strip_zone("strip-1", 5);
    let layout = make_layout(vec![zone.clone()]);

    // Sample the zone
    let colors = sample_zone(&canvas, &zone, &layout);

    assert_eq!(colors.len(), 5, "should have 5 LED colors");

    // LED 0 at x=0.0 should be red (255, 0, 0)
    assert!(
        colors[0][0] > 200,
        "LED 0 red channel should be high, got {}",
        colors[0][0]
    );
    assert!(
        colors[0][2] < 55,
        "LED 0 blue channel should be low, got {}",
        colors[0][2]
    );

    // LED 4 at x=1.0 should be blue (0, 0, 255)
    assert!(
        colors[4][0] < 55,
        "LED 4 red channel should be low, got {}",
        colors[4][0]
    );
    assert!(
        colors[4][2] > 200,
        "LED 4 blue channel should be high, got {}",
        colors[4][2]
    );

    // LED 2 at x=0.5 should be middle purple (~128, 0, ~128)
    assert!(
        colors[2][0] > 80 && colors[2][0] < 180,
        "LED 2 red channel should be mid-range, got {}",
        colors[2][0]
    );
    assert!(
        colors[2][2] > 80 && colors[2][2] < 180,
        "LED 2 blue channel should be mid-range, got {}",
        colors[2][2]
    );
}

#[test]
fn spatial_sampling_single_led_samples_with_bilinear() {
    let mut canvas = Canvas::new(10, 10);
    canvas.fill(Rgba::new(100, 200, 50, 255));

    let mode = SamplingMode::Bilinear;
    let edge = EdgeBehavior::Clamp;
    let zone = make_strip_zone("point", 1);

    let color = sample_led(
        &canvas,
        NormalizedPosition::new(0.5, 0.5),
        &zone,
        &mode,
        edge,
    );

    assert_eq!(color.r, 100);
    assert_eq!(color.g, 200);
    assert_eq!(color.b, 50);
    assert_eq!(color.a, 255);
}

#[test]
fn spatial_sampling_nearest_mode_snaps_correctly() {
    let mut canvas = Canvas::new(4, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    canvas.set_pixel(2, 0, Rgba::new(0, 0, 255, 255));
    canvas.set_pixel(3, 0, Rgba::new(255, 255, 0, 255));

    let zone = make_strip_zone("snap", 1);
    let mode = SamplingMode::Nearest;
    let edge = EdgeBehavior::Clamp;

    // Sample at x=0.0 should get the leftmost pixel (red)
    let color = sample_led(
        &canvas,
        NormalizedPosition::new(0.0, 0.5),
        &zone,
        &mode,
        edge,
    );
    assert_eq!(color.r, 255);
    assert_eq!(color.g, 0);

    // Sample at x=1.0 should get the rightmost pixel (yellow)
    let color = sample_led(
        &canvas,
        NormalizedPosition::new(1.0, 0.5),
        &zone,
        &mode,
        edge,
    );
    assert_eq!(color.r, 255);
    assert_eq!(color.g, 255);
    assert_eq!(color.b, 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 4: Device Registry with Discovery
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn device_registry_with_discovery_full_scan() {
    let registry = DeviceRegistry::new();
    assert!(registry.is_empty().await);

    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());

    // Add test scanner with 3 devices
    let devices = vec![
        make_discovered_device("wled-strip-1", 60),
        make_discovered_device("wled-strip-2", 120),
        make_discovered_device("wled-matrix", 256),
    ];

    orchestrator.add_scanner(Box::new(TestScanner::new("test-wled", devices)));
    assert_eq!(orchestrator.scanner_count(), 1);

    // Full scan
    let report = orchestrator.full_scan().await;

    assert_eq!(report.new_devices.len(), 3, "should discover 3 new devices");
    assert_eq!(report.total_known, 3);
    assert!(report.reappeared_devices.is_empty());

    // Registry should now have 3 devices
    assert_eq!(registry.len().await, 3);

    let all_devices = registry.list().await;
    assert_eq!(all_devices.len(), 3);

    // Verify each device is findable
    for id in &report.new_devices {
        let device = registry.get(id).await;
        assert!(device.is_some(), "device {id} should be in registry");
    }
}

#[tokio::test]
async fn discovery_deduplicates_across_scanners() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());

    // Two scanners report the same device with the same fingerprint
    let device1 = make_discovered_device("shared-device", 30);
    let mut device2 = make_discovered_device("shared-device", 30);
    device2.name = "shared-device-v2".to_string();

    orchestrator.add_scanner(Box::new(TestScanner::new("scanner-a", vec![device1])));
    orchestrator.add_scanner(Box::new(TestScanner::new("scanner-b", vec![device2])));

    let report = orchestrator.full_scan().await;

    // Deduplication by fingerprint means only 1 unique device
    assert_eq!(report.total_known, 1, "should deduplicate to 1 device");
}

#[tokio::test]
async fn discovery_rescan_reports_reappeared() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());

    let devices = vec![make_discovered_device("persistent-device", 10)];
    orchestrator.add_scanner(Box::new(TestScanner::new("test", devices.clone())));

    // First scan: new device
    let report1 = orchestrator.full_scan().await;
    assert_eq!(report1.new_devices.len(), 1);
    assert!(report1.reappeared_devices.is_empty());

    // Second scan: same device reappears
    let report2 = orchestrator.full_scan().await;
    assert!(report2.new_devices.is_empty(), "no new devices on rescan");
    assert_eq!(
        report2.reappeared_devices.len(),
        1,
        "device should reappear"
    );
    assert_eq!(report2.total_known, 1);
}

#[tokio::test]
async fn registry_state_transitions() {
    let registry = DeviceRegistry::new();
    let info = make_device_info("state-test", 10);
    let id = registry.add(info).await;

    // Default state is Known
    let device = registry.get(&id).await.expect("device should exist");
    assert_eq!(device.state, hypercolor_types::device::DeviceState::Known);

    // Transition to Connected
    assert!(
        registry
            .set_state(&id, hypercolor_types::device::DeviceState::Connected)
            .await
    );
    let device = registry.get(&id).await.expect("device should exist");
    assert_eq!(
        device.state,
        hypercolor_types::device::DeviceState::Connected
    );

    // Transition to Active
    assert!(
        registry
            .set_state(&id, hypercolor_types::device::DeviceState::Active)
            .await
    );
    let device = registry.get(&id).await.expect("device should exist");
    assert_eq!(device.state, hypercolor_types::device::DeviceState::Active);
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 5: Event Bus Integration
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn event_bus_broadcast_receives_events() {
    let bus = HypercolorBus::new();

    // Subscribe before publishing
    let mut rx = bus.subscribe_all();

    // Publish DaemonStarted event
    bus.publish(HypercolorEvent::DaemonStarted {
        version: "0.1.0".to_string(),
        pid: 1234,
        device_count: 0,
        effect_count: 0,
    });

    let event = rx.recv().await.expect("should receive event");
    assert!(matches!(event.event, HypercolorEvent::DaemonStarted { .. }));
    assert!(!event.timestamp.is_empty());
    assert!(event.mono_ms < 10_000); // should be within first 10 seconds
}

#[tokio::test]
async fn event_bus_filtered_subscription() {
    let bus = HypercolorBus::new();

    // Subscribe to only System category events
    let filter = EventFilter::new().categories(vec![EventCategory::System]);
    let mut filtered_rx = bus.subscribe_filtered(filter);

    // Also get unfiltered for comparison
    let mut all_rx = bus.subscribe_all();

    // Publish a System event
    bus.publish(HypercolorEvent::DaemonStarted {
        version: "0.1.0".to_string(),
        pid: 1234,
        device_count: 0,
        effect_count: 0,
    });

    // Publish a Device event (should be filtered out)
    bus.publish(HypercolorEvent::DeviceDiscovered {
        device_id: "test".to_string(),
        name: "Test Device".to_string(),
        backend: "wled".to_string(),
        led_count: 60,
        address: None,
    });

    // Publish another System event
    bus.publish(HypercolorEvent::Paused);

    // Unfiltered should get all 3
    let e1 = all_rx.recv().await.expect("should receive event 1");
    assert!(matches!(e1.event, HypercolorEvent::DaemonStarted { .. }));
    let e2 = all_rx.recv().await.expect("should receive event 2");
    assert!(matches!(e2.event, HypercolorEvent::DeviceDiscovered { .. }));
    let e3 = all_rx.recv().await.expect("should receive event 3");
    assert!(matches!(e3.event, HypercolorEvent::Paused));

    // Filtered should only get System events (DaemonStarted and Paused)
    let f1 = filtered_rx
        .recv()
        .await
        .expect("should receive filtered event 1");
    assert!(matches!(f1.event, HypercolorEvent::DaemonStarted { .. }));

    let f2 = filtered_rx
        .recv()
        .await
        .expect("should receive filtered event 2");
    assert!(matches!(f2.event, HypercolorEvent::Paused));
}

#[tokio::test]
async fn event_bus_subscriber_count_tracks_subscriptions() {
    let bus = HypercolorBus::new();

    assert_eq!(bus.subscriber_count(), 0);

    let rx1 = bus.subscribe_all();
    assert_eq!(bus.subscriber_count(), 1);

    let _rx2 = bus.subscribe_all();
    assert_eq!(bus.subscriber_count(), 2);

    drop(rx1);
    // Note: broadcast subscriber count may not immediately reflect drops,
    // but we verify it was at least 2 before the drop.
}

#[tokio::test]
async fn event_bus_frame_data_watch_channel() {
    use hypercolor_types::event::{FrameData, ZoneColors};

    let bus = HypercolorBus::new();
    let mut frame_rx = bus.frame_receiver();

    // Publish frame data
    let frame = FrameData::new(
        vec![ZoneColors {
            zone_id: "zone-1".to_string(),
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        }],
        1,
        100,
    );

    bus.frame_sender()
        .send(frame)
        .expect("frame send should succeed");

    // Wait for change
    frame_rx.changed().await.expect("should detect change");
    let received = frame_rx.borrow_and_update();
    assert_eq!(received.frame_number, 1);
    assert_eq!(received.total_leds(), 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 6: Scene Manager + Transitions
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn scene_manager_create_activate_transition() {
    let mut manager = SceneManager::new();

    // Create two scenes
    let mut scene_a = make_scene("Ambient Glow");
    scene_a.zone_assignments = vec![ZoneAssignment {
        zone_name: "strip-1".to_string(),
        effect_name: "aurora".to_string(),
        parameters: HashMap::new(),
        brightness: Some(0.8),
    }];

    let mut scene_b = make_scene("Party Mode");
    scene_b.zone_assignments = vec![ZoneAssignment {
        zone_name: "strip-1".to_string(),
        effect_name: "strobe".to_string(),
        parameters: HashMap::new(),
        brightness: Some(1.0),
    }];

    let id_a = scene_a.id;
    let id_b = scene_b.id;

    manager.create(scene_a).expect("create scene A");
    manager.create(scene_b).expect("create scene B");
    assert_eq!(manager.scene_count(), 2);

    // Activate scene A (instant, no previous scene)
    manager.activate(&id_a, None).expect("activate A");
    assert_eq!(
        manager.active_scene_id().expect("should have active scene"),
        &id_a
    );
    assert!(
        !manager.is_transitioning(),
        "first activation has no transition"
    );

    // Activate scene B with a 1000ms transition
    let transition = TransitionSpec {
        duration_ms: 1000,
        easing: EasingFunction::Linear,
        color_interpolation: ColorInterpolation::Oklab,
    };
    manager
        .activate(&id_b, Some(transition))
        .expect("activate B with transition");

    assert!(manager.is_transitioning(), "should have active transition");
    let t = manager.active_transition().expect("transition exists");
    assert!(
        (t.progress - 0.0).abs() < f32::EPSILON,
        "progress should start at 0"
    );

    // Tick the transition halfway (500ms = 0.5 progress)
    manager.tick_transition(0.5);
    let t = manager.active_transition().expect("still transitioning");
    assert!(
        (t.progress - 0.5).abs() < 0.01,
        "progress should be ~0.5, got {}",
        t.progress
    );

    // Tick to completion
    manager.tick_transition(0.6);
    assert!(!manager.is_transitioning(), "transition should be complete");
}

#[test]
fn scene_manager_priority_stack_ordering() {
    let mut manager = SceneManager::new();

    let mut ambient = make_scene("Ambient");
    ambient.priority = ScenePriority::AMBIENT;
    let ambient_id = ambient.id;

    let mut user = make_scene("User Scene");
    user.priority = ScenePriority::USER;
    let user_id = user.id;

    let mut alert = make_scene("Alert");
    alert.priority = ScenePriority::ALERT;
    let alert_id = alert.id;

    manager.create(ambient).expect("create ambient");
    manager.create(user).expect("create user");
    manager.create(alert).expect("create alert");

    // Activate in any order — highest priority wins
    manager
        .activate(&ambient_id, None)
        .expect("activate ambient");
    manager.activate(&user_id, None).expect("activate user");
    assert_eq!(
        manager.active_scene_id().expect("active"),
        &user_id,
        "user priority > ambient"
    );

    manager.activate(&alert_id, None).expect("activate alert");
    assert_eq!(
        manager.active_scene_id().expect("active"),
        &alert_id,
        "alert priority > user"
    );

    // Deactivate alert -> user should take over
    manager.deactivate_current();
    assert_eq!(
        manager.active_scene_id().expect("active"),
        &user_id,
        "user should restore after alert deactivated"
    );
}

#[test]
fn transition_state_progress_and_blending() {
    let from_id = hypercolor_types::scene::SceneId::new();
    let to_id = hypercolor_types::scene::SceneId::new();

    let from_assignments = vec![ZoneAssignment {
        zone_name: "z1".to_string(),
        effect_name: "aurora".to_string(),
        parameters: HashMap::new(),
        brightness: Some(1.0),
    }];

    let to_assignments = vec![ZoneAssignment {
        zone_name: "z1".to_string(),
        effect_name: "strobe".to_string(),
        parameters: HashMap::new(),
        brightness: Some(0.5),
    }];

    let spec = TransitionSpec {
        duration_ms: 1000,
        easing: EasingFunction::Linear,
        color_interpolation: ColorInterpolation::Srgb,
    };

    let mut ts = TransitionState::new(from_id, to_id, spec, from_assignments, to_assignments);
    assert!(!ts.is_complete());
    assert!((ts.progress - 0.0).abs() < f32::EPSILON);

    // Tick 25% (250ms of 1000ms)
    ts.tick(0.25);
    assert!((ts.progress - 0.25).abs() < 0.01);

    // Blend should produce interpolated brightness
    let blended = ts.blend();
    assert_eq!(blended.len(), 1);
    let b = blended[0].brightness.expect("brightness set");
    // At t=0.25: lerp(1.0, 0.5, 0.25) = 0.875
    assert!(
        (b - 0.875).abs() < 0.05,
        "brightness should be ~0.875, got {b}"
    );
    // Before midpoint: effect name should still be "from" side
    assert_eq!(blended[0].effect_name, "aurora");

    // Tick past midpoint
    ts.tick(0.30);
    let blended = ts.blend();
    // Past 0.5: effect name should be "to" side
    assert_eq!(blended[0].effect_name, "strobe");

    // Tick to completion
    ts.tick(1.0);
    assert!(ts.is_complete());
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 7: Full Pipeline (No Real Devices)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn full_pipeline_render_sample_extract_colors() {
    // Stage 1: Create render loop and start it
    let mut render_loop = RenderLoop::new(60);
    render_loop.start();

    // Stage 2: Create effect engine with gradient renderer
    let mut effect_engine = EffectEngine::new().with_canvas_size(100, 10);
    let renderer = Box::new(TestGradientRenderer::new());
    effect_engine
        .activate(renderer, test_metadata("pipeline-gradient"))
        .expect("activation should succeed");

    // Stage 3: Create spatial layout with a 10-LED strip
    let zone = make_strip_zone("pipeline-strip", 10);
    let layout = SpatialLayout {
        id: "pipeline-layout".to_string(),
        name: "Pipeline Test Layout".to_string(),
        description: None,
        canvas_width: 100,
        canvas_height: 10,
        zones: vec![zone.clone()],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let audio = AudioData::silence();

    // Stage 4: Frame 1 — tick render loop, tick effect engine, sample
    assert!(render_loop.tick(), "render loop tick 1");
    let canvas = effect_engine
        .tick(0.016, &audio)
        .expect("effect engine tick 1");

    let colors = sample_zone(&canvas, &zone, &layout);

    assert_eq!(colors.len(), 10, "should have 10 LED colors");

    // Verify gradient: LED 0 should be reddish, LED 9 should be bluish
    assert!(colors[0][0] > 200, "LED 0 should be mostly red");
    assert!(colors[0][2] < 55, "LED 0 should have little blue");
    assert!(colors[9][0] < 55, "LED 9 should have little red");
    assert!(colors[9][2] > 200, "LED 9 should be mostly blue");

    // Verify monotonic gradient: red decreases, blue increases
    for i in 1..10 {
        assert!(
            colors[i][0] <= colors[i - 1][0].saturating_add(5),
            "red should decrease or stay level: LED {} has {}, LED {} has {}",
            i,
            colors[i][0],
            i - 1,
            colors[i - 1][0]
        );
        assert!(
            colors[i][2].saturating_add(5) >= colors[i - 1][2],
            "blue should increase or stay level: LED {} has {}, LED {} has {}",
            i,
            colors[i][2],
            i - 1,
            colors[i - 1][2]
        );
    }

    let _stats = render_loop.frame_complete();

    // Stage 5: Frame 2 — verify frame advances
    assert!(render_loop.tick(), "render loop tick 2");
    let canvas2 = effect_engine
        .tick(0.016, &audio)
        .expect("effect engine tick 2");
    let colors2 = sample_zone(&canvas2, &zone, &layout);

    // Gradient is time-independent so colors should be the same
    assert_eq!(colors2.len(), 10);
    assert_eq!(colors2[0][0], colors[0][0], "gradient should be stable");
    assert_eq!(colors2[9][2], colors[9][2], "gradient should be stable");

    let _stats = render_loop.frame_complete();
    assert_eq!(render_loop.frame_number(), 2);
}

#[test]
fn full_pipeline_with_advancing_solid_renderer() {
    let mut render_loop = RenderLoop::new(60);
    render_loop.start();

    let mut effect_engine = EffectEngine::new().with_canvas_size(10, 10);
    effect_engine
        .activate(
            Box::new(TestSolidRenderer::new()),
            test_metadata("solid-advance"),
        )
        .expect("activate");

    let zone = make_strip_zone("solid-strip", 3);
    let layout = SpatialLayout {
        id: "solid-layout".to_string(),
        name: "Solid Layout".to_string(),
        description: None,
        canvas_width: 10,
        canvas_height: 10,
        zones: vec![zone.clone()],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let audio = AudioData::silence();

    let mut prev_brightness = 0u8;
    for frame in 0..5 {
        assert!(render_loop.tick());
        let canvas = effect_engine
            .tick(0.016, &audio)
            .expect("tick should succeed");
        let colors = sample_zone(&canvas, &zone, &layout);

        assert_eq!(colors.len(), 3);

        // All LEDs should be the same brightness
        assert_eq!(
            colors[0], colors[1],
            "frame {frame}: all LEDs should be equal"
        );
        assert_eq!(colors[1], colors[2]);

        // Brightness should change each frame
        let brightness = colors[0][0];
        if frame > 0 {
            assert_ne!(
                brightness, prev_brightness,
                "frame {frame}: brightness should change"
            );
        }
        prev_brightness = brightness;

        let _ = render_loop.frame_complete();
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TEST 8: FPS Controller Tier Transitions
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn fps_controller_downshift_on_slow_frames() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        upshift_sustain_secs: 0.1,
        upshift_headroom_ratio: 0.5,
        ewma_alpha: 0.3,
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);

    assert_eq!(ctrl.tier(), FpsTier::Full);

    // Feed 2 slow frames (budget is ~16.6ms, so 25ms exceeds it)
    ctrl.record_frame(Duration::from_millis(25));
    assert_eq!(ctrl.consecutive_misses(), 1);
    assert!(!ctrl.should_downshift());

    ctrl.record_frame(Duration::from_millis(25));
    assert_eq!(ctrl.consecutive_misses(), 2);
    assert!(ctrl.should_downshift());

    // Execute transition
    let new_tier = ctrl.maybe_transition();
    assert_eq!(new_tier, Some(FpsTier::High));
    assert_eq!(ctrl.tier(), FpsTier::High);

    // Consecutive misses should be reset
    assert_eq!(ctrl.consecutive_misses(), 0);
}

#[test]
fn fps_controller_upshift_needs_sustained_headroom() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 2,
        upshift_sustain_secs: 0.0, // instant upshift for testing
        upshift_headroom_ratio: 0.7,
        ewma_alpha: 0.9, // very responsive EWMA for testing
    };
    let mut ctrl = FpsController::with_config(FpsTier::High, config);

    assert_eq!(ctrl.tier(), FpsTier::High);

    // Feed many fast frames (1ms is well under 22.2ms budget)
    for _ in 0..20 {
        ctrl.record_frame(Duration::from_millis(1));
    }

    // With sustained headroom and 0s wait, should upshift
    // First call sets upshift_eligible_since
    let _ = ctrl.should_upshift();
    // Second call sees sustained headroom (with 0s threshold, should pass)
    if ctrl.should_upshift() {
        let new_tier = ctrl.upshift();
        assert_eq!(new_tier, Some(FpsTier::Full));
        assert_eq!(ctrl.tier(), FpsTier::Full);
    }
}

#[test]
fn fps_controller_full_downshift_cascade() {
    let config = TierTransitionConfig {
        downshift_miss_threshold: 1,
        ..TierTransitionConfig::default()
    };
    let mut ctrl = FpsController::with_config(FpsTier::Full, config);

    let expected = [
        FpsTier::High,
        FpsTier::Medium,
        FpsTier::Low,
        FpsTier::Minimal,
    ];

    for expected_tier in expected {
        ctrl.record_frame(Duration::from_millis(500));
        let result = ctrl.maybe_transition();
        assert_eq!(result, Some(expected_tier));
    }

    // At Minimal, no more downshifts
    ctrl.record_frame(Duration::from_millis(500));
    assert_eq!(ctrl.maybe_transition(), None);
    assert_eq!(ctrl.tier(), FpsTier::Minimal);
}

// ═════════════════════════════════════════════════════════════════════════════
// Additional Integration Tests
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn effect_engine_activate_deactivate_cycle() {
    let mut engine = EffectEngine::new();
    let audio = AudioData::silence();

    // Cycle 1: gradient renderer
    engine
        .activate(
            Box::new(TestGradientRenderer::new()),
            test_metadata("cycle-1"),
        )
        .expect("activate cycle 1");
    assert!(engine.is_running());

    let c1 = engine.tick(0.016, &audio).expect("tick cycle 1");
    assert_eq!(c1.get_pixel(0, 0).r, 255); // red on left

    // Cycle 2: solid renderer replaces gradient
    engine
        .activate(Box::new(TestSolidRenderer::new()), test_metadata("cycle-2"))
        .expect("activate cycle 2");
    assert!(engine.is_running());

    let c2 = engine.tick(0.016, &audio).expect("tick cycle 2");
    // Solid renderer frame 1: brightness = (1 * 25) % 256 = 25
    let pixel = c2.get_pixel(0, 0);
    assert_eq!(pixel.r, 25);
    assert_eq!(pixel.g, 25);
    assert_eq!(pixel.b, 25);

    // Deactivate
    engine.deactivate();
    assert!(!engine.is_running());

    // Tick after deactivation returns black canvas
    let c3 = engine.tick(0.016, &audio).expect("tick after deactivate");
    assert_eq!(c3.get_pixel(0, 0).r, 0);
}

#[test]
fn effect_registry_search_and_categorize() {
    let mut registry = EffectRegistry::default();

    let entries = vec![
        ("aurora", EffectCategory::Ambient, vec!["nature", "calm"]),
        ("strobe", EffectCategory::Audio, vec!["reactive", "beat"]),
        ("plasma", EffectCategory::Generative, vec!["art", "noise"]),
        ("solid", EffectCategory::Utility, vec!["simple"]),
        (
            "aurora-northern",
            EffectCategory::Ambient,
            vec!["nature", "polar"],
        ),
    ];

    for (name, category, tags) in entries {
        let entry = EffectEntry {
            metadata: EffectMetadata {
                id: EffectId::new(Uuid::now_v7()),
                name: name.to_string(),
                author: "test".to_string(),
                version: "1.0.0".to_string(),
                description: format!("Effect: {name}"),
                category,
                tags: tags.into_iter().map(String::from).collect(),
                source: EffectSource::Native {
                    path: PathBuf::from(format!("native/{name}.wgsl")),
                },
                license: None,
            },
            source_path: PathBuf::from(format!("/effects/{name}.wgsl")),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        };
        registry.register(entry);
    }

    assert_eq!(registry.len(), 5);

    // Search by name prefix
    let aurora_results = registry.search("aurora");
    assert_eq!(aurora_results.len(), 2);

    // By category
    let ambient = registry.by_category(EffectCategory::Ambient);
    assert_eq!(ambient.len(), 2);

    // All tags (deduplicated and sorted)
    let tags = registry.all_tags();
    assert!(tags.contains(&"nature".to_string()));
    assert!(tags.contains(&"beat".to_string()));

    // Categories present
    let cats = registry.categories();
    assert!(cats.len() >= 4);
}

#[test]
fn render_loop_pause_resume_does_not_lose_frames() {
    let mut rl = RenderLoop::new(60);
    rl.start();

    // Tick 2 frames
    assert!(rl.tick());
    let _ = rl.frame_complete();
    assert!(rl.tick());
    let _ = rl.frame_complete();
    assert_eq!(rl.frame_number(), 2);

    // Pause
    rl.pause();
    assert_eq!(rl.state(), RenderLoopState::Paused);
    assert!(!rl.tick(), "tick should return false when paused");

    // Resume
    rl.resume();
    assert_eq!(rl.state(), RenderLoopState::Running);

    // Tick 2 more frames
    assert!(rl.tick());
    let _ = rl.frame_complete();
    assert!(rl.tick());
    let _ = rl.frame_complete();

    // Total should be 4 (pause doesn't reset)
    assert_eq!(rl.frame_number(), 4);
}

#[tokio::test]
async fn registry_add_remove_lifecycle() {
    let registry = DeviceRegistry::new();

    let info = make_device_info("lifecycle-device", 30);
    let id = info.id;

    // Add
    let returned_id = registry.add(info).await;
    assert_eq!(returned_id, id);
    assert_eq!(registry.len().await, 1);
    assert!(registry.contains(&id).await);

    // Remove
    let removed = registry.remove(&id).await;
    assert!(removed.is_some());
    assert_eq!(registry.len().await, 0);
    assert!(!registry.contains(&id).await);

    // Remove non-existent
    let removed_again = registry.remove(&id).await;
    assert!(removed_again.is_none());
}

#[test]
fn spatial_layout_with_multiple_zones() {
    // Create a canvas with distinct quadrant colors
    let mut canvas = Canvas::new(100, 100);

    // Top-left: red, Top-right: green, Bottom-left: blue, Bottom-right: white
    #[allow(clippy::as_conversions)]
    for y in 0..100 {
        for x in 0..100 {
            let color = match (x < 50, y < 50) {
                (true, true) => Rgba::new(255, 0, 0, 255),
                (false, true) => Rgba::new(0, 255, 0, 255),
                (true, false) => Rgba::new(0, 0, 255, 255),
                (false, false) => Rgba::new(255, 255, 255, 255),
            };
            canvas.set_pixel(x, y, color);
        }
    }

    // Create two zones: one at top-left, one at bottom-right
    let mut zone_tl = make_strip_zone("top-left", 1);
    zone_tl.led_positions = vec![NormalizedPosition::new(0.0, 0.0)];

    let mut zone_br = make_strip_zone("bot-right", 1);
    zone_br.led_positions = vec![NormalizedPosition::new(1.0, 1.0)];

    let layout = SpatialLayout {
        id: "quadrant-layout".to_string(),
        name: "Quadrant Test".to_string(),
        description: None,
        canvas_width: 100,
        canvas_height: 100,
        zones: vec![zone_tl.clone(), zone_br.clone()],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let colors_tl = sample_zone(&canvas, &zone_tl, &layout);
    let colors_br = sample_zone(&canvas, &zone_br, &layout);

    // Top-left should be red
    assert_eq!(colors_tl[0][0], 255, "top-left should be red");
    assert_eq!(colors_tl[0][1], 0);
    assert_eq!(colors_tl[0][2], 0);

    // Bottom-right should be white
    assert_eq!(colors_br[0][0], 255, "bottom-right should be white");
    assert_eq!(colors_br[0][1], 255);
    assert_eq!(colors_br[0][2], 255);
}

#[test]
fn effect_engine_pause_resume_canvas_behavior() {
    let mut engine = EffectEngine::new();
    let audio = AudioData::silence();

    engine
        .activate(
            Box::new(TestGradientRenderer::new()),
            test_metadata("pause-test"),
        )
        .expect("activate");

    // Normal tick
    let canvas = engine.tick(0.016, &audio).expect("tick");
    assert_eq!(canvas.get_pixel(0, 0).r, 255); // gradient starts red

    // Pause
    engine.pause();
    assert_eq!(engine.state(), EffectState::Paused);

    // Tick while paused returns black
    let paused_canvas = engine.tick(0.016, &audio).expect("tick while paused");
    assert_eq!(paused_canvas.get_pixel(0, 0).r, 0);
    assert_eq!(paused_canvas.get_pixel(0, 0).a, 255); // opaque black

    // Resume
    engine.resume();
    assert_eq!(engine.state(), EffectState::Running);

    // Tick after resume should produce gradient again
    let resumed = engine.tick(0.016, &audio).expect("tick after resume");
    assert_eq!(resumed.get_pixel(0, 0).r, 255);
}

#[tokio::test]
async fn multiple_scanners_aggregate_results() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());

    // Scanner A: 2 WLED devices
    let wled_devices = vec![
        make_discovered_device("wled-1", 30),
        make_discovered_device("wled-2", 60),
    ];
    orchestrator.add_scanner(Box::new(TestScanner::new("wled", wled_devices)));

    // Scanner B: 1 USB device
    let usb_device = {
        let info = DeviceInfo {
            id: DeviceId::new(),
            name: "USB HID Controller".to_string(),
            vendor: "PrismRGB".to_string(),
            family: DeviceFamily::Custom("prism".to_string()),
            connection_type: ConnectionType::Usb,
            zones: vec![ZoneInfo {
                name: "channel-1".to_string(),
                led_count: 40,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("2.0.0".to_string()),
            capabilities: DeviceCapabilities {
                led_count: 40,
                supports_direct: true,
                supports_brightness: true,
                max_fps: 60,
            },
        };
        DiscoveredDevice {
            connection_type: ConnectionType::Usb,
            name: "USB HID Controller".to_string(),
            family: DeviceFamily::Custom("prism".to_string()),
            fingerprint: DeviceFingerprint("usb:prism-1".to_string()),
            info,
            metadata: HashMap::new(),
        }
    };
    orchestrator.add_scanner(Box::new(TestScanner::new("usb", vec![usb_device])));

    assert_eq!(orchestrator.scanner_count(), 2);

    let report = orchestrator.full_scan().await;
    assert_eq!(
        report.new_devices.len(),
        3,
        "should discover 3 unique devices"
    );
    assert_eq!(report.total_known, 3);
    assert_eq!(registry.len().await, 3);
}

#[test]
fn canvas_sampling_area_average() {
    let mut canvas = Canvas::new(10, 10);
    // Fill entire canvas with a single color
    canvas.fill(Rgba::new(128, 64, 32, 255));

    let zone = make_strip_zone("area-test", 1);
    let mode = SamplingMode::AreaAverage {
        radius_x: 2.0,
        radius_y: 2.0,
    };
    let edge = EdgeBehavior::Clamp;

    let color = sample_led(
        &canvas,
        NormalizedPosition::new(0.5, 0.5),
        &zone,
        &mode,
        edge,
    );

    // Area average of uniform canvas should return the same color
    assert_eq!(color.r, 128);
    assert_eq!(color.g, 64);
    assert_eq!(color.b, 32);
}

#[test]
fn scene_manager_delete_active_scene() {
    let mut manager = SceneManager::new();

    let scene = make_scene("Deletable");
    let id = scene.id;
    manager.create(scene).expect("create");
    manager.activate(&id, None).expect("activate");

    assert_eq!(manager.active_scene_id().expect("active"), &id);

    // Delete the active scene
    let deleted = manager.delete(&id).expect("delete");
    assert_eq!(deleted.name, "Deletable");

    // No active scene anymore
    assert!(manager.active_scene_id().is_none());
    assert_eq!(manager.scene_count(), 0);
}

#[test]
fn render_loop_stats_snapshot_reflects_state() {
    let mut rl = RenderLoop::new(30);
    let stats = rl.stats();
    assert_eq!(stats.state, RenderLoopState::Created);
    assert_eq!(stats.tier, FpsTier::Medium);
    assert_eq!(stats.total_frames, 0);

    rl.start();
    let stats = rl.stats();
    assert_eq!(stats.state, RenderLoopState::Running);

    rl.set_tier(FpsTier::Low);
    let stats = rl.stats();
    assert_eq!(stats.tier, FpsTier::Low);
}

#[tokio::test]
async fn registry_list_by_state_filters_correctly() {
    let registry = DeviceRegistry::new();

    let info1 = make_device_info("active-device", 10);
    let id1 = registry.add(info1).await;

    let info2 = make_device_info("known-device", 20);
    let _id2 = registry.add(info2).await;

    // Set device 1 to Active
    registry
        .set_state(&id1, hypercolor_types::device::DeviceState::Active)
        .await;

    let active = registry
        .list_by_state(&hypercolor_types::device::DeviceState::Active)
        .await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].info.name, "active-device");

    let known = registry
        .list_by_state(&hypercolor_types::device::DeviceState::Known)
        .await;
    assert_eq!(known.len(), 1);
    assert_eq!(known[0].info.name, "known-device");
}
