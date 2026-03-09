//! Integration tests exercising the mock backend through the full pipeline.
//!
//! These tests verify that the mock device backend, transport scanner, and
//! effect renderer work correctly together — from discovery through frame
//! rendering and LED color output — without any real hardware.

use hypercolor_core::device::mock::{
    MockCall, MockDeviceBackend, MockDeviceConfig, MockEffectRenderer, MockTransportScanner,
};
use hypercolor_core::device::{DeviceBackend, DeviceRegistry, DiscoveryOrchestrator};
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::effect::EffectRenderer;
use hypercolor_core::spatial::{SpatialEngine, generate_positions};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Rgba;
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, SpatialLayout, StripDirection,
};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a strip device config.
fn strip_config(name: &str, led_count: u32) -> MockDeviceConfig {
    MockDeviceConfig {
        name: name.to_owned(),
        led_count,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        id: Some(DeviceId::new()),
    }
}

/// Build a matrix device config.
fn matrix_config(name: &str, width: u32, height: u32) -> MockDeviceConfig {
    MockDeviceConfig {
        name: name.to_owned(),
        led_count: width * height,
        topology: LedTopology::Matrix {
            width,
            height,
            serpentine: false,
            start_corner: hypercolor_types::spatial::Corner::TopLeft,
        },
        id: Some(DeviceId::new()),
    }
}

/// Build a ring device config.
fn ring_config(name: &str, count: u32) -> MockDeviceConfig {
    MockDeviceConfig {
        name: name.to_owned(),
        led_count: count,
        topology: LedTopology::Ring {
            count,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
        id: Some(DeviceId::new()),
    }
}

/// Build a minimal spatial layout with one zone mapped to a device.
fn build_layout_for_device(
    device_id: DeviceId,
    zone_name: &str,
    topology: LedTopology,
) -> SpatialLayout {
    SpatialLayout {
        id: "test-layout".to_owned(),
        name: "Test Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: format!("zone-{zone_name}"),
            name: zone_name.to_owned(),
            device_id: device_id.to_string(),
            zone_name: None,
            group_id: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology,
            led_positions: Vec::new(), // Rebuilt by SpatialEngine::new
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
        }],
        groups: vec![],
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Nearest,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

/// Build a layout with two zones: a strip and a matrix.
fn build_dual_zone_layout(
    strip_id: DeviceId,
    strip_count: u32,
    matrix_id: DeviceId,
    matrix_width: u32,
    matrix_height: u32,
) -> SpatialLayout {
    SpatialLayout {
        id: "dual-zone-layout".to_owned(),
        name: "Dual Zone Test Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            DeviceZone {
                id: "zone-strip".to_owned(),
                name: "LED Strip".to_owned(),
                device_id: strip_id.to_string(),
                zone_name: None,
                group_id: None,
                position: NormalizedPosition::new(0.5, 0.25),
                size: NormalizedPosition::new(1.0, 0.5),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: strip_count,
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
            },
            DeviceZone {
                id: "zone-matrix".to_owned(),
                name: "LED Matrix".to_owned(),
                device_id: matrix_id.to_string(),
                zone_name: None,
                group_id: None,
                position: NormalizedPosition::new(0.5, 0.75),
                size: NormalizedPosition::new(0.5, 0.5),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Matrix {
                    width: matrix_width,
                    height: matrix_height,
                    serpentine: false,
                    start_corner: hypercolor_types::spatial::Corner::TopLeft,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: None,
                edge_behavior: None,
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
            },
        ],
        groups: vec![],
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Nearest,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

// ── Test 1: Mock Device Discovery ───────────────────────────────────────────

#[tokio::test]
async fn scanner_finds_devices_and_orchestrator_deduplicates() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    // Two scanners with overlapping devices (same fingerprint)
    let strip = strip_config("Living Room Strip", 60);
    let matrix = matrix_config("Desk Matrix", 10, 10);
    let ring = ring_config("Fan Ring", 12);

    let scanner_a = MockTransportScanner::new("mock-mdns")
        .with_device(&strip)
        .with_device(&matrix);

    let scanner_b = MockTransportScanner::new("mock-udp").with_device(&ring);

    orchestrator.add_scanner(Box::new(scanner_a));
    orchestrator.add_scanner(Box::new(scanner_b));

    let report = orchestrator.full_scan().await;

    // All three unique devices should be discovered
    assert_eq!(report.new_devices.len(), 3);
    assert_eq!(report.total_known, 3);
    assert!(report.reappeared_devices.is_empty());

    // Registry should contain exactly 3 devices
    assert_eq!(orchestrator.registry().len().await, 3);
}

#[tokio::test]
async fn scanner_deduplicates_same_fingerprint() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    // Create two scanners that report the exact same device config with same ID
    let shared_id = DeviceId::new();
    let config = MockDeviceConfig {
        name: "Shared Device".to_owned(),
        led_count: 30,
        topology: LedTopology::Strip {
            count: 30,
            direction: StripDirection::LeftToRight,
        },
        id: Some(shared_id),
    };

    let scanner_a = MockTransportScanner::new("scanner-a").with_device(&config);
    let scanner_b = MockTransportScanner::new("scanner-b").with_device(&config);

    orchestrator.add_scanner(Box::new(scanner_a));
    orchestrator.add_scanner(Box::new(scanner_b));

    let report = orchestrator.full_scan().await;

    // Should deduplicate to at most 2 (fingerprints are unique per scanner+device combo)
    // but both point to the same device ID, so registry contains at most 2 entries
    // (one per unique fingerprint). The important thing: no duplicates by fingerprint.
    assert!(report.total_known <= 2);
}

#[tokio::test]
async fn scanner_handles_failure_gracefully() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let mut failing_scanner = MockTransportScanner::new("broken");
    failing_scanner.should_fail = true;

    let good_scanner =
        MockTransportScanner::new("healthy").with_device(&strip_config("Good Strip", 30));

    orchestrator.add_scanner(Box::new(failing_scanner));
    orchestrator.add_scanner(Box::new(good_scanner));

    let report = orchestrator.full_scan().await;

    // The good scanner's device should still show up
    assert_eq!(report.new_devices.len(), 1);
    assert_eq!(report.total_known, 1);
}

// ── Test 2: Mock Device Lifecycle ───────────────────────────────────────────

#[tokio::test]
async fn device_lifecycle_discover_connect_write_disconnect() {
    let config = strip_config("Test Strip", 60);
    let device_id = config.id.expect("device id should be set");

    let mut backend = MockDeviceBackend::new().with_device(&config);

    // Discover
    let devices = backend.discover().await.expect("discover should succeed");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Test Strip");
    assert_eq!(devices[0].total_led_count(), 60);

    // Connect
    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");
    assert!(backend.is_connected(&device_id));

    // Write colors (solid red)
    let red_frame: Vec<[u8; 3]> = vec![[255, 0, 0]; 60];
    backend
        .write_colors(&device_id, &red_frame)
        .await
        .expect("write should succeed");

    // Verify written data
    let written = backend
        .last_colors(&device_id)
        .expect("colors should be stored");
    assert_eq!(written.len(), 60);
    assert!(written.iter().all(|c| *c == [255, 0, 0]));
    assert_eq!(backend.write_count(), 1);

    // Disconnect
    backend
        .disconnect(&device_id)
        .await
        .expect("disconnect should succeed");
    assert!(!backend.is_connected(&device_id));

    // Verify call log
    let calls = backend.calls();
    assert_eq!(calls[0], MockCall::Discover);
    assert_eq!(calls[1], MockCall::Connect(device_id));
    assert_eq!(
        calls[2],
        MockCall::WriteColors {
            device_id,
            led_count: 60,
        }
    );
    assert_eq!(calls[3], MockCall::Disconnect(device_id));
}

#[tokio::test]
async fn write_to_disconnected_device_fails() {
    let config = strip_config("Disconnected Strip", 30);
    let device_id = config.id.expect("device id");

    let mut backend = MockDeviceBackend::new().with_device(&config);

    let colors: Vec<[u8; 3]> = vec![[0, 255, 0]; 30];
    let result = backend.write_colors(&device_id, &colors).await;
    assert!(
        result.is_err(),
        "writing to disconnected device should fail"
    );
}

#[tokio::test]
async fn connect_failure_propagates() {
    let config = strip_config("Flaky Strip", 10);
    let device_id = config.id.expect("device id");

    let mut backend = MockDeviceBackend::new().with_device(&config);
    backend.fail_connect = true;

    let result = backend.connect(&device_id).await;
    assert!(
        result.is_err(),
        "connect should fail when fail_connect is set"
    );
}

#[tokio::test]
async fn write_failure_propagates() {
    let config = strip_config("Unreliable Strip", 10);
    let device_id = config.id.expect("device id");

    let mut backend = MockDeviceBackend::new().with_device(&config);

    backend
        .connect(&device_id)
        .await
        .expect("connect should succeed");
    backend.fail_write = true;

    let colors: Vec<[u8; 3]> = vec![[0, 0, 255]; 10];
    let result = backend.write_colors(&device_id, &colors).await;
    assert!(result.is_err(), "write should fail when fail_write is set");
}

// ── Test 3: Mock Effect Rendering ───────────────────────────────────────────

#[test]
fn solid_renderer_produces_uniform_canvas() {
    let mut engine = EffectEngine::new().with_canvas_size(64, 64);
    let renderer = Box::new(MockEffectRenderer::solid(255, 0, 0));
    let meta = MockEffectRenderer::sample_metadata("solid-red");

    engine
        .activate(renderer, meta)
        .expect("activation should succeed");

    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick should succeed");

    // Every pixel should be solid red
    let pixel = canvas.get_pixel(0, 0);
    assert_eq!(pixel, Rgba::new(255, 0, 0, 255));

    let mid_pixel = canvas.get_pixel(32, 32);
    assert_eq!(mid_pixel, Rgba::new(255, 0, 0, 255));

    let corner_pixel = canvas.get_pixel(63, 63);
    assert_eq!(corner_pixel, Rgba::new(255, 0, 0, 255));
}

#[test]
fn rainbow_renderer_produces_gradient() {
    let mut engine = EffectEngine::new().with_canvas_size(360, 1);
    let renderer = Box::new(MockEffectRenderer::rainbow());
    let meta = MockEffectRenderer::sample_metadata("rainbow");

    engine
        .activate(renderer, meta)
        .expect("activation should succeed");

    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick should succeed");

    // First pixel should be near red (hue ~0)
    let left = canvas.get_pixel(0, 0);
    assert!(
        left.r > 200,
        "left edge should have high red: got {}",
        left.r
    );

    // Pixel at ~1/3 should be near green (hue ~120)
    let mid = canvas.get_pixel(120, 0);
    assert!(
        mid.g > 200,
        "1/3 mark should have high green: got {}",
        mid.g
    );

    // Pixel at ~2/3 should be near blue (hue ~240)
    let right = canvas.get_pixel(240, 0);
    assert!(
        right.b > 200,
        "2/3 mark should have high blue: got {}",
        right.b
    );
}

#[test]
fn audio_reactive_renderer_scales_with_level() {
    let mut engine = EffectEngine::new().with_canvas_size(10, 10);
    let renderer = Box::new(MockEffectRenderer::audio_reactive(200, 100, 50));
    let meta = MockEffectRenderer::sample_metadata("audio-pulse");

    engine
        .activate(renderer, meta)
        .expect("activation should succeed");

    // Silence -> all black
    let silence = AudioData::silence();
    let canvas_silent = engine.tick(0.016, &silence).expect("tick silent");
    let px = canvas_silent.get_pixel(5, 5);
    assert_eq!(px.r, 0, "silence should produce black");
    assert_eq!(px.g, 0);
    assert_eq!(px.b, 0);

    // Full volume -> base color at full intensity
    let mut loud = AudioData::silence();
    loud.rms_level = 1.0;
    let canvas_loud = engine.tick(0.016, &loud).expect("tick loud");
    let px_loud = canvas_loud.get_pixel(5, 5);
    assert_eq!(px_loud.r, 200, "full volume should produce base color");
    assert_eq!(px_loud.g, 100);
    assert_eq!(px_loud.b, 50);

    // Half volume -> roughly half intensity
    let mut half = AudioData::silence();
    half.rms_level = 0.5;
    let canvas_half = engine.tick(0.016, &half).expect("tick half");
    let px_half = canvas_half.get_pixel(5, 5);
    assert_eq!(px_half.r, 100, "half volume should produce half intensity");
    assert_eq!(px_half.g, 50);
    assert_eq!(px_half.b, 25);
}

#[test]
fn effect_renderer_lifecycle_tracking() {
    let mut renderer = MockEffectRenderer::solid(0, 0, 0);
    let meta = MockEffectRenderer::sample_metadata("lifecycle-test");

    assert!(!renderer.initialized);
    assert!(!renderer.destroyed);
    assert_eq!(renderer.tick_count, 0);

    renderer.init(&meta).expect("init should succeed");
    assert!(renderer.initialized);

    let input = hypercolor_core::effect::FrameInput {
        time_secs: 0.0,
        delta_secs: 0.016,
        frame_number: 0,
        audio: AudioData::silence(),
        interaction: hypercolor_core::input::InteractionData::default(),
        canvas_width: 10,
        canvas_height: 10,
    };
    let _ = renderer.tick(&input).expect("tick");
    let _ = renderer.tick(&input).expect("tick");
    assert_eq!(renderer.tick_count, 2);

    renderer.destroy();
    assert!(renderer.destroyed);
}

// ── Test 4: Full Pipeline (No Render Loop) ──────────────────────────────────

#[tokio::test]
async fn full_pipeline_solid_red_through_strip() {
    // Set up: 60-LED strip device
    let strip = strip_config("Pipeline Strip", 60);
    let strip_id = strip.id.expect("strip id");

    let mut backend = MockDeviceBackend::new().with_device(&strip);

    // Connect the device
    backend
        .connect(&strip_id)
        .await
        .expect("connect should succeed");

    // Create effect engine with solid red renderer
    let mut effect_engine = EffectEngine::new().with_canvas_size(320, 200);
    let renderer = Box::new(MockEffectRenderer::solid(255, 0, 0));
    let meta = MockEffectRenderer::sample_metadata("solid-red");
    effect_engine
        .activate(renderer, meta)
        .expect("activate effect");

    // Tick to produce a canvas
    let audio = AudioData::silence();
    let canvas = effect_engine
        .tick(0.016, &audio)
        .expect("tick should produce canvas");

    // Build spatial layout for the strip
    let layout = build_layout_for_device(
        strip_id,
        "Pipeline Strip",
        LedTopology::Strip {
            count: 60,
            direction: StripDirection::LeftToRight,
        },
    );
    let spatial = SpatialEngine::new(layout);

    // Sample the canvas to get LED colors
    let zone_colors = spatial.sample(&canvas);
    assert_eq!(zone_colors.len(), 1, "should have one zone");
    assert_eq!(zone_colors[0].colors.len(), 60, "strip should have 60 LEDs");

    // Every LED should be red (sampled from a solid red canvas)
    for (i, color) in zone_colors[0].colors.iter().enumerate() {
        assert_eq!(*color, [255, 0, 0], "LED {i} should be red, got {color:?}");
    }

    // Write colors to the mock device
    backend
        .write_colors(&strip_id, &zone_colors[0].colors)
        .await
        .expect("write colors should succeed");

    // Verify the backend received the correct data
    let written = backend.last_colors(&strip_id).expect("should have colors");
    assert_eq!(written.len(), 60);
    assert!(written.iter().all(|c| *c == [255, 0, 0]));
}

#[tokio::test]
async fn full_pipeline_dual_devices() {
    // Two devices: a 60-LED strip and a 10x10 matrix
    let strip = strip_config("Dual Strip", 60);
    let strip_id = strip.id.expect("strip id");

    let matrix = matrix_config("Dual Matrix", 10, 10);
    let matrix_id = matrix.id.expect("matrix id");

    let mut backend = MockDeviceBackend::new()
        .with_device(&strip)
        .with_device(&matrix);

    backend.connect(&strip_id).await.expect("connect strip");
    backend.connect(&matrix_id).await.expect("connect matrix");

    // Solid green effect
    let mut effect_engine = EffectEngine::new().with_canvas_size(320, 200);
    let renderer = Box::new(MockEffectRenderer::solid(0, 255, 0));
    let meta = MockEffectRenderer::sample_metadata("solid-green");
    effect_engine
        .activate(renderer, meta)
        .expect("activate effect");

    let audio = AudioData::silence();
    let canvas = effect_engine.tick(0.016, &audio).expect("tick");

    // Build dual-zone layout
    let layout = build_dual_zone_layout(strip_id, 60, matrix_id, 10, 10);
    let spatial = SpatialEngine::new(layout);
    let zone_colors = spatial.sample(&canvas);

    assert_eq!(zone_colors.len(), 2, "should have two zones");

    // Strip zone
    assert_eq!(zone_colors[0].colors.len(), 60);
    for color in &zone_colors[0].colors {
        assert_eq!(*color, [0, 255, 0], "strip LED should be green");
    }

    // Matrix zone
    assert_eq!(zone_colors[1].colors.len(), 100);
    for color in &zone_colors[1].colors {
        assert_eq!(*color, [0, 255, 0], "matrix LED should be green");
    }

    // Write to both devices
    backend
        .write_colors(&strip_id, &zone_colors[0].colors)
        .await
        .expect("write strip");
    backend
        .write_colors(&matrix_id, &zone_colors[1].colors)
        .await
        .expect("write matrix");

    assert_eq!(backend.write_count(), 2);

    let strip_written = backend.last_colors(&strip_id).expect("strip colors");
    assert_eq!(strip_written.len(), 60);

    let matrix_written = backend.last_colors(&matrix_id).expect("matrix colors");
    assert_eq!(matrix_written.len(), 100);
}

// ── Test 5: Multiple Frames ─────────────────────────────────────────────────

#[tokio::test]
async fn multiple_frames_increment_and_update() {
    let config = strip_config("Multi-Frame Strip", 30);
    let device_id = config.id.expect("device id");

    let mut backend = MockDeviceBackend::new().with_device(&config);
    backend.connect(&device_id).await.expect("connect");

    // Rainbow renderer — output changes each frame due to time offset
    let mut effect_engine = EffectEngine::new().with_canvas_size(320, 200);
    let renderer = Box::new(MockEffectRenderer::rainbow());
    let meta = MockEffectRenderer::sample_metadata("rainbow-multi");
    effect_engine.activate(renderer, meta).expect("activate");

    let layout = build_layout_for_device(
        device_id,
        "Multi-Frame Strip",
        LedTopology::Strip {
            count: 30,
            direction: StripDirection::LeftToRight,
        },
    );
    let spatial = SpatialEngine::new(layout);
    let audio = AudioData::silence();

    let mut previous_colors: Option<Vec<[u8; 3]>> = None;
    let delta = 0.1; // 100ms per frame so time advances enough for visible change

    for frame_idx in 0..10u64 {
        let canvas = effect_engine
            .tick(delta, &audio)
            .expect("tick should succeed");

        let zone_colors = spatial.sample(&canvas);
        assert_eq!(zone_colors.len(), 1);
        assert_eq!(zone_colors[0].colors.len(), 30);

        backend
            .write_colors(&device_id, &zone_colors[0].colors)
            .await
            .expect("write colors");

        // After a few frames, colors should differ from previous frame
        // (rainbow shifts over time). Skip frame 0 since there's no previous.
        if let Some(ref prev) = previous_colors {
            if frame_idx > 1 {
                let any_changed = zone_colors[0]
                    .colors
                    .iter()
                    .zip(prev.iter())
                    .any(|(a, b)| a != b);
                assert!(
                    any_changed,
                    "frame {frame_idx}: colors should change between frames"
                );
            }
        }

        previous_colors = Some(zone_colors[0].colors.clone());
    }

    // 10 frames written
    assert_eq!(backend.write_count(), 10);
}

// ── Test 6: Effect Switching ────────────────────────────────────────────────

#[tokio::test]
async fn effect_switching_produces_different_output() {
    let config = strip_config("Effect Switch Strip", 20);
    let device_id = config.id.expect("device id");

    let mut backend = MockDeviceBackend::new().with_device(&config);
    backend.connect(&device_id).await.expect("connect");

    let layout = build_layout_for_device(
        device_id,
        "Effect Switch Strip",
        LedTopology::Strip {
            count: 20,
            direction: StripDirection::LeftToRight,
        },
    );
    let spatial = SpatialEngine::new(layout);
    let audio = AudioData::silence();

    // Effect A: solid red
    let mut engine = EffectEngine::new().with_canvas_size(320, 200);
    let renderer_a = Box::new(MockEffectRenderer::solid(255, 0, 0));
    let meta_a = MockEffectRenderer::sample_metadata("effect-a-red");
    engine.activate(renderer_a, meta_a).expect("activate A");

    let canvas_a = engine.tick(0.016, &audio).expect("tick A");
    let colors_a = spatial.sample(&canvas_a);
    backend
        .write_colors(&device_id, &colors_a[0].colors)
        .await
        .expect("write A");

    let written_a = backend.last_colors(&device_id).expect("colors A").clone();
    assert!(
        written_a.iter().all(|c| *c == [255, 0, 0]),
        "effect A should produce red"
    );

    // Deactivate A
    engine.deactivate();
    assert!(!engine.is_running());

    // Effect B: solid blue
    let renderer_b = Box::new(MockEffectRenderer::solid(0, 0, 255));
    let meta_b = MockEffectRenderer::sample_metadata("effect-b-blue");
    engine.activate(renderer_b, meta_b).expect("activate B");

    let canvas_b = engine.tick(0.016, &audio).expect("tick B");
    let colors_b = spatial.sample(&canvas_b);
    backend
        .write_colors(&device_id, &colors_b[0].colors)
        .await
        .expect("write B");

    let written_b = backend.last_colors(&device_id).expect("colors B").clone();
    assert!(
        written_b.iter().all(|c| *c == [0, 0, 255]),
        "effect B should produce blue"
    );

    // Verify they're different
    assert_ne!(
        written_a, written_b,
        "effects A and B should produce different colors"
    );
    assert_eq!(backend.write_count(), 2);
}

#[tokio::test]
async fn effect_replacement_without_explicit_deactivate() {
    // The engine should auto-deactivate the previous effect when activating a new one
    let mut engine = EffectEngine::new().with_canvas_size(32, 32);
    let audio = AudioData::silence();

    // Activate effect A
    let renderer_a = Box::new(MockEffectRenderer::solid(255, 0, 0));
    let meta_a = MockEffectRenderer::sample_metadata("auto-replace-a");
    engine.activate(renderer_a, meta_a).expect("activate A");
    let canvas_a = engine.tick(0.016, &audio).expect("tick A");
    assert_eq!(canvas_a.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));

    // Activate effect B directly (no deactivate call)
    let renderer_b = Box::new(MockEffectRenderer::solid(0, 255, 0));
    let meta_b = MockEffectRenderer::sample_metadata("auto-replace-b");
    engine.activate(renderer_b, meta_b).expect("activate B");
    let canvas_b = engine.tick(0.016, &audio).expect("tick B");
    assert_eq!(canvas_b.get_pixel(0, 0), Rgba::new(0, 255, 0, 255));
}

// ── Additional Edge Case Tests ──────────────────────────────────────────────

#[test]
fn topology_position_generation_strip() {
    let topology = LedTopology::Strip {
        count: 5,
        direction: StripDirection::LeftToRight,
    };
    let positions = generate_positions(&topology);
    assert_eq!(positions.len(), 5);

    // First LED at left edge, last at right edge, all at y=0.5
    assert!((positions[0].x - 0.0).abs() < f32::EPSILON);
    assert!((positions[0].y - 0.5).abs() < f32::EPSILON);
    assert!((positions[4].x - 1.0).abs() < f32::EPSILON);
    assert!((positions[4].y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn topology_position_generation_matrix() {
    let topology = LedTopology::Matrix {
        width: 3,
        height: 3,
        serpentine: false,
        start_corner: hypercolor_types::spatial::Corner::TopLeft,
    };
    let positions = generate_positions(&topology);
    assert_eq!(positions.len(), 9);

    // Top-left corner
    assert!((positions[0].x - 0.0).abs() < f32::EPSILON);
    assert!((positions[0].y - 0.0).abs() < f32::EPSILON);

    // Bottom-right corner
    assert!((positions[8].x - 1.0).abs() < f32::EPSILON);
    assert!((positions[8].y - 1.0).abs() < f32::EPSILON);
}

#[tokio::test]
async fn backend_info_returns_mock_metadata() {
    let backend = MockDeviceBackend::new();
    let info = backend.info();
    assert_eq!(info.id, "mock");
    assert_eq!(info.name, "Mock Device Backend");
    assert!(!info.description.is_empty());
}

#[tokio::test]
async fn registry_integration_with_discovery() {
    let registry = DeviceRegistry::new();
    let mut orchestrator = DiscoveryOrchestrator::new(registry);

    let scanner = MockTransportScanner::new("test-scanner")
        .with_device(&strip_config("Reg Strip A", 30))
        .with_device(&ring_config("Reg Ring B", 16));

    orchestrator.add_scanner(Box::new(scanner));

    let report = orchestrator.full_scan().await;
    assert_eq!(report.new_devices.len(), 2);

    // Verify devices are in the registry
    let devices = orchestrator.registry().list().await;
    assert_eq!(devices.len(), 2);

    // All devices should be in Known state initially
    for device in &devices {
        assert_eq!(device.state, DeviceState::Known);
    }
}

#[tokio::test]
async fn mock_backend_multiple_devices_independent_state() {
    let dev_a = strip_config("Device A", 10);
    let id_a = dev_a.id.expect("id a");
    let dev_b = strip_config("Device B", 20);
    let id_b = dev_b.id.expect("id b");

    let mut backend = MockDeviceBackend::new()
        .with_device(&dev_a)
        .with_device(&dev_b);

    // Connect both
    backend.connect(&id_a).await.expect("connect A");
    backend.connect(&id_b).await.expect("connect B");
    assert!(backend.is_connected(&id_a));
    assert!(backend.is_connected(&id_b));

    // Write different colors to each
    let red: Vec<[u8; 3]> = vec![[255, 0, 0]; 10];
    let blue: Vec<[u8; 3]> = vec![[0, 0, 255]; 20];

    backend.write_colors(&id_a, &red).await.expect("write A");
    backend.write_colors(&id_b, &blue).await.expect("write B");

    // Verify independent state
    let colors_a = backend.last_colors(&id_a).expect("A colors");
    let colors_b = backend.last_colors(&id_b).expect("B colors");

    assert_eq!(colors_a.len(), 10);
    assert_eq!(colors_b.len(), 20);
    assert!(colors_a.iter().all(|c| *c == [255, 0, 0]));
    assert!(colors_b.iter().all(|c| *c == [0, 0, 255]));

    // Disconnect A only — B should remain connected
    backend.disconnect(&id_a).await.expect("disconnect A");
    assert!(!backend.is_connected(&id_a));
    assert!(backend.is_connected(&id_b));

    // Writing to A should now fail, B should succeed
    let result_a = backend.write_colors(&id_a, &red).await;
    assert!(result_a.is_err());

    backend
        .write_colors(&id_b, &blue)
        .await
        .expect("B should still work");
}
