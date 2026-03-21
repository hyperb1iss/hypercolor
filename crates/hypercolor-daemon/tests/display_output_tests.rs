use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};

use hypercolor_daemon::display_output::{DisplayOutputState, DisplayOutputThread};
use hypercolor_daemon::logical_devices::LogicalDevice;
use hypercolor_daemon::session::OutputPowerState;

struct RecordingDisplayBackend {
    expected_device_id: DeviceId,
    connected: bool,
    display_writes: Arc<Mutex<Vec<Vec<u8>>>>,
    write_delay: Duration,
}

impl RecordingDisplayBackend {
    fn new(expected_device_id: DeviceId, display_writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        Self {
            expected_device_id,
            connected: false,
            display_writes,
            write_delay: Duration::ZERO,
        }
    }

    fn with_write_delay(mut self, write_delay: Duration) -> Self {
        self.write_delay = write_delay;
        self
    }
}

#[async_trait]
impl DeviceBackend for RecordingDisplayBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "USB Recording".to_owned(),
            description: "Test backend for display output".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        self.connected = false;
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        Ok(())
    }

    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("display write while disconnected");
        }

        if !self.write_delay.is_zero() {
            tokio::time::sleep(self.write_delay).await;
        }

        self.display_writes.lock().await.push(jpeg_data.to_vec());
        Ok(())
    }
}

fn display_device_info(
    device_id: DeviceId,
    has_display: bool,
    width: u32,
    height: u32,
    circular: bool,
) -> DeviceInfo {
    let zones = if has_display {
        vec![ZoneInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width,
                height,
                circular,
            },
            color_format: DeviceColorFormat::Jpeg,
        }]
    } else {
        vec![ZoneInfo {
            name: "Ring".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Ring { count: 24 },
            color_format: DeviceColorFormat::Rgb,
        }]
    };

    DeviceInfo {
        id: device_id,
        name: "Corsair Test Device".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::Corsair,
        model: None,
        connection_type: ConnectionType::Usb,
        zones,
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: if has_display { 0 } else { 24 },
            supports_direct: !has_display,
            supports_brightness: false,
            has_display,
            display_resolution: has_display.then_some((width, height)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn layout_with_zones(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "layout-test".to_owned(),
        name: "Layout Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn display_zone(
    device_id: &str,
    position: NormalizedPosition,
    size: NormalizedPosition,
) -> DeviceZone {
    DeviceZone {
        id: "zone-display".to_owned(),
        name: "Display Zone".to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        group_id: None,
        position,
        size,
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

fn default_power_state_rx() -> watch::Receiver<OutputPowerState> {
    let (tx, rx) = watch::channel(OutputPowerState::default());
    let _ = Box::leak(Box::new(tx));
    rx
}

const TEST_STATIC_HOLD_REFRESH_INTERVAL: Duration = Duration::from_millis(60);

fn led_zone(
    device_id: &str,
    zone_name: &str,
    position: NormalizedPosition,
    size: NormalizedPosition,
) -> DeviceZone {
    DeviceZone {
        id: format!("zone-{}", zone_name.to_lowercase().replace(' ', "-")),
        name: zone_name.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: Some(zone_name.to_owned()),
        group_id: None,
        position,
        size,
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 16,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
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

fn sample_canvas() -> Canvas {
    let mut canvas = Canvas::new(320, 200);
    canvas.fill(Rgba::new(8, 16, 24, 255));
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(319, 0, Rgba::new(0, 255, 0, 255));
    canvas.set_pixel(0, 199, Rgba::new(0, 0, 255, 255));
    canvas.set_pixel(319, 199, Rgba::new(255, 255, 255, 255));
    canvas
}

fn split_color_canvas() -> Canvas {
    let mut canvas = Canvas::new(320, 200);
    for y in 0..200 {
        for x in 0..320 {
            let color = if x < 160 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 0, 255, 255)
            };
            canvas.set_pixel(x, y, color);
        }
    }
    canvas
}

fn solid_canvas(color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(320, 200);
    canvas.fill(color);
    canvas
}

async fn wait_for_display_writes(display_writes: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let writes = display_writes.lock().await.clone();
            if !writes.is_empty() {
                return writes;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("display output should arrive within timeout")
}

async fn wait_for_display_write_count(
    display_writes: &Arc<Mutex<Vec<Vec<u8>>>>,
    expected_count: usize,
) -> Vec<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let writes = display_writes.lock().await.clone();
            if writes.len() >= expected_count {
                return writes;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("display output should reach expected write count within timeout")
}

fn decode_jpeg(bytes: &[u8]) -> image::RgbaImage {
    image::load_from_memory(bytes)
        .expect("display output should decode as an image")
        .into_rgba8()
}

fn mixed_led_display_device_info(device_id: DeviceId, width: u32, height: u32) -> DeviceInfo {
    DeviceInfo {
        id: device_id,
        name: "Hybrid Display Device".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::Corsair,
        model: Some("hybrid-display".to_owned()),
        connection_type: ConnectionType::Usb,
        zones: vec![
            ZoneInfo {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Display".to_owned(),
                led_count: 0,
                topology: DeviceTopologyHint::Display {
                    width,
                    height,
                    circular: false,
                },
                color_format: DeviceColorFormat::Jpeg,
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 64,
            supports_direct: true,
            supports_brightness: false,
            has_display: true,
            display_resolution: Some((width, height)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[tokio::test]
async fn automatic_display_output_mirrors_canvas_to_layout_mapped_display_devices() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 480, 480, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = sample_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    assert!(!writes[0].is_empty());
    assert_eq!(&writes[0][..3], &[0xFF, 0xD8, 0xFF]);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_devices_without_display_capabilities() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));

    let tracked_id = device_registry
        .add(display_device_info(device_id, false, 480, 480, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = sample_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(display_writes.lock().await.is_empty());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_display_devices_that_are_not_in_layout() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = sample_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(display_writes.lock().await.is_empty());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_uses_layout_zone_viewport() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.25, 0.5),
            NormalizedPosition::new(0.5, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = split_color_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > 200,
        "expected red-dominant viewport, got {pixel:?}"
    );
    assert!(
        pixel[2] < 80,
        "expected viewport to exclude blue half, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_defaults_mixed_devices_to_full_canvas_without_display_zone() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![led_zone(
            &format!("device:{device_id}"),
            "Pads",
            NormalizedPosition::new(0.25, 0.5),
            NormalizedPosition::new(0.5, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "push2:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(mixed_led_display_device_info(device_id, 320, 200))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = split_color_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let left_pixel = image.get_pixel(image.width() / 4, image.height() / 2);
    let right_pixel = image.get_pixel((image.width() * 3) / 4, image.height() / 2);

    assert!(
        left_pixel[0] > 200 && left_pixel[2] < 80,
        "expected left side to stay red under full-canvas fallback, got {left_pixel:?}"
    );
    assert!(
        right_pixel[2] > 200 && right_pixel[0] < 80,
        "expected right side to stay blue under full-canvas fallback, got {right_pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_drops_stale_frames_for_slow_displays() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(
        RecordingDisplayBackend::new(device_id, Arc::clone(&display_writes))
            .with_write_delay(Duration::from_millis(180)),
    ));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let green = solid_canvas(Rgba::new(0, 255, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 1, 16));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&green, 2, 32));
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&blue, 3, 48));

    tokio::time::sleep(Duration::from_millis(550)).await;

    let writes = display_writes.lock().await.clone();
    assert!(
        !writes.is_empty(),
        "slow display backend should still receive frames"
    );
    assert!(
        writes.len() <= 2,
        "expected latest-only display worker to drop stale frames, got {} writes",
        writes.len()
    );

    let final_image = decode_jpeg(writes.last().expect("expected at least one display frame"));
    let pixel = final_image.get_pixel(final_image.width() / 2, final_image.height() / 2);
    assert!(
        pixel[2] > 200,
        "expected final display frame to be blue, got {pixel:?}"
    );
    assert!(
        pixel[0] < 80,
        "expected final display frame to drop stale red, got {pixel:?}"
    );
    assert!(
        pixel[1] < 80,
        "expected final display frame to drop stale green, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_uses_latest_pending_frame_for_paced_writes() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let green = solid_canvas(Rgba::new(0, 255, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    let first_image = decode_jpeg(
        writes
            .first()
            .expect("expected initial paced display frame"),
    );
    let first_pixel = first_image.get_pixel(first_image.width() / 2, first_image.height() / 2);
    assert!(
        first_pixel[0] > 200,
        "expected first paced frame to be red, got {first_pixel:?}"
    );

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&green, 2, 32));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&blue, 3, 48));

    let writes = wait_for_display_write_count(&display_writes, 2).await;
    let second_image = decode_jpeg(writes.last().expect("expected second paced display frame"));
    let second_pixel = second_image.get_pixel(second_image.width() / 2, second_image.height() / 2);
    assert!(
        second_pixel[2] > 200,
        "expected paced display update to use the latest pending blue frame, got {second_pixel:?}"
    );
    assert!(
        second_pixel[1] < 80,
        "expected stale green frame to be superseded before the paced write, got {second_pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_unchanged_frames() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    assert_eq!(writes.len(), 1);

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 2, 32));
    tokio::time::sleep(Duration::from_millis(140)).await;
    assert_eq!(
        display_writes.lock().await.len(),
        1,
        "identical frame should not trigger another LCD write"
    );

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&blue, 3, 48));
    let writes = wait_for_display_write_count(&display_writes, 2).await;
    assert_eq!(writes.len(), 2);

    let final_image = decode_jpeg(writes.last().expect("expected changed display frame"));
    let pixel = final_image.get_pixel(final_image.width() / 2, final_image.height() / 2);
    assert!(
        pixel[2] > 200,
        "expected changed frame to be blue, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_refreshes_cached_targets_when_layout_changes() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.25, 0.5),
            NormalizedPosition::new(0.5, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let canvas = split_color_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    let first_image = decode_jpeg(writes.first().expect("expected initial display frame"));
    let first_pixel = first_image.get_pixel(first_image.width() / 2, first_image.height() / 2);
    assert!(
        first_pixel[0] > 200,
        "expected initial viewport to be red, got {first_pixel:?}"
    );

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.75, 0.5),
            NormalizedPosition::new(0.5, 1.0),
        )]));
    }

    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 2, 32));
    let writes = wait_for_display_write_count(&display_writes, 2).await;
    let second_image = decode_jpeg(writes.last().expect("expected refreshed display frame"));
    let second_pixel = second_image.get_pixel(second_image.width() / 2, second_image.height() / 2);
    assert!(
        second_pixel[2] > 200,
        "expected refreshed viewport to be blue after layout change, got {second_pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_refreshes_static_hold_frames_while_sleeping() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let (power_tx, power_state) = watch::channel(OutputPowerState::default());

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            &format!("device:{device_id}"),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true, 320, 200, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        power_state,
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
    });

    let black = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&black, 1, 16));
    let _ = wait_for_display_write_count(&display_writes, 1).await;

    power_tx.send_replace(OutputPowerState {
        sleeping: true,
        off_output_behavior: OffOutputBehavior::Static,
        ..OutputPowerState::default()
    });

    let writes = wait_for_display_write_count(&display_writes, 2).await;
    assert_eq!(
        writes.len(),
        2,
        "expected static hold refresh to re-send the last LCD frame while sleeping"
    );

    thread.shutdown().await.expect("display thread should stop");
}
