use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, watch};
use uuid::Uuid;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend, DeviceRegistry};
use hypercolor_core::overlay::{
    OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer, OverlaySize,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::overlay::{
    Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, HourFormat, HtmlOverlayConfig, ImageFit,
    ImageOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId,
    OverlaySource, SensorDisplayStyle, SensorOverlayConfig, TextAlign, TextOverlayConfig,
};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};

use hypercolor_daemon::device_settings::DeviceSettingsStore;
use hypercolor_daemon::display_output::overlay::{
    DefaultOverlayRendererFactory, OverlayRendererBinding, OverlayRendererFactory,
};
use hypercolor_daemon::display_output::{DisplayOutputState, DisplayOutputThread};
use hypercolor_daemon::display_overlays::{
    DisplayOverlayRegistry, DisplayOverlayRuntimeRegistry, OverlaySlotRuntime, OverlaySlotStatus,
};
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

fn default_sensor_snapshot_rx() -> watch::Receiver<Arc<SystemSnapshot>> {
    let (tx, rx) = watch::channel(Arc::new(SystemSnapshot::empty()));
    let _ = Box::leak(Box::new(tx));
    rx
}

fn default_display_overlays() -> Arc<DisplayOverlayRegistry> {
    Arc::new(DisplayOverlayRegistry::new())
}

fn default_display_overlay_runtime() -> Arc<DisplayOverlayRuntimeRegistry> {
    Arc::new(DisplayOverlayRuntimeRegistry::new())
}

fn default_device_settings() -> Arc<RwLock<DeviceSettingsStore>> {
    Arc::new(RwLock::new(DeviceSettingsStore::new(
        std::path::PathBuf::from("device-settings.json"),
    )))
}

fn default_overlay_factory() -> Arc<dyn OverlayRendererFactory> {
    Arc::new(DefaultOverlayRendererFactory::new())
}

fn default_text_overlay_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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

struct SolidOverlayRenderer {
    color: [u8; 4],
}

impl OverlayRenderer for SolidOverlayRenderer {
    fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
        Ok(())
    }

    fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
        Ok(())
    }

    fn render_into(
        &mut self,
        _input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), OverlayError> {
        for pixel in target.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&self.color);
        }
        Ok(())
    }
}

struct SolidOverlayFactory {
    color: [u8; 4],
    interval: Duration,
}

impl OverlayRendererFactory for SolidOverlayFactory {
    fn build(
        &self,
        _slot: &OverlaySlot,
        _target_size: OverlaySize,
    ) -> std::result::Result<OverlayRendererBinding, OverlayError> {
        Ok(OverlayRendererBinding {
            renderer: Box::new(SolidOverlayRenderer { color: self.color }),
            render_interval: self.interval,
        })
    }
}

async fn wait_for_display_writes(display_writes: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<Vec<u8>> {
    wait_for_display_writes_with_timeout(display_writes, Duration::from_secs(1)).await
}

async fn wait_for_display_writes_with_timeout(
    display_writes: &Arc<Mutex<Vec<Vec<u8>>>>,
    timeout: Duration,
) -> Vec<Vec<u8>> {
    tokio::time::timeout(timeout, async {
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

async fn wait_for_overlay_runtime(
    runtime_registry: &Arc<DisplayOverlayRuntimeRegistry>,
    device_id: DeviceId,
    slot_id: OverlaySlotId,
) -> OverlaySlotRuntime {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(runtime) = runtime_registry.get(device_id).await.slot(slot_id).cloned() {
                return runtime;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("overlay runtime should arrive within timeout")
}

fn decode_jpeg(bytes: &[u8]) -> image::RgbaImage {
    image::load_from_memory(bytes)
        .expect("display output should decode as an image")
        .into_rgba8()
}

fn write_png(path: &Path, color: [u8; 4], width: u32, height: u32) {
    image::ImageBuffer::from_pixel(width, height, image::Rgba(color))
        .save(path)
        .expect("png should save");
}

fn region_contains_visible_pixels(
    image: &image::RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> bool {
    let max_x = (x + width).min(image.width());
    let max_y = (y + height).min(image.height());
    for sample_y in y..max_y {
        for sample_x in x..max_x {
            let pixel = image.get_pixel(sample_x, sample_y);
            if pixel[0] > 20 || pixel[1] > 20 || pixel[2] > 20 {
                return true;
            }
        }
    }

    false
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
async fn automatic_display_output_composites_mock_overlay_into_display_frame() {
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
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

    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: OverlaySlotId::from(Uuid::now_v7()),
                    name: "Mock Overlay".to_owned(),
                    source: OverlaySource::Text(TextOverlayConfig {
                        text: "mock".to_owned(),
                        font_family: None,
                        font_size: 12.0,
                        color: "#ffffff".to_owned(),
                        align: TextAlign::Center,
                        scroll: false,
                        scroll_speed: 30.0,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 60,
                        offset_y: 60,
                        width: 120,
                        height: 120,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 0.5,
                    enabled: true,
                }],
            },
        )
        .await;

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: Arc::new(SolidOverlayFactory {
            color: [255, 0, 0, 255],
            interval: Duration::from_secs(1),
        }),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let inside_overlay = image.get_pixel(120, 120);
    let outside_overlay = image.get_pixel(16, 16);

    assert!(
        inside_overlay[0] > 80 && inside_overlay[1] < 40 && inside_overlay[2] < 40,
        "expected red-tinted overlay pixel, got {inside_overlay:?}"
    );
    assert!(
        outside_overlay[0] < 30 && outside_overlay[1] < 30 && outside_overlay[2] < 30,
        "expected untouched black background outside overlay, got {outside_overlay:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_hydrates_persisted_overlay_config_on_worker_spawn() {
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let device_settings = Arc::new(RwLock::new(DeviceSettingsStore::new(
        temp_dir.path().join("device-settings.json"),
    )));
    let device_id = DeviceId::new();

    {
        let mut store = device_settings.write().await;
        store.set_display_overlays(
            &device_id.to_string(),
            Some(DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: OverlaySlotId::from(Uuid::now_v7()),
                    name: "Persisted Overlay".to_owned(),
                    source: OverlaySource::Text(TextOverlayConfig {
                        text: "persisted".to_owned(),
                        font_family: None,
                        font_size: 12.0,
                        color: "#ffffff".to_owned(),
                        align: TextAlign::Center,
                        scroll: false,
                        scroll_speed: 30.0,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 60,
                        offset_y: 60,
                        width: 120,
                        height: 120,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 0.5,
                    enabled: true,
                }],
            }),
        );
    }

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

    assert!(overlays.get(device_id).await.is_empty());

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        device_settings: Arc::clone(&device_settings),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: Arc::new(SolidOverlayFactory {
            color: [255, 0, 0, 255],
            interval: Duration::from_secs(1),
        }),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let inside_overlay = image.get_pixel(120, 120);

    assert!(
        inside_overlay[0] > 80 && inside_overlay[1] < 40 && inside_overlay[2] < 40,
        "expected persisted overlay pixel to be tinted red, got {inside_overlay:?}"
    );
    assert_eq!(overlays.get(device_id).await.overlays.len(), 1);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_publishes_overlay_runtime_failures() {
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let overlay_runtime = default_display_overlay_runtime();
    let device_id = DeviceId::new();
    let slot_id = OverlaySlotId::from(Uuid::now_v7());

    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: slot_id,
                    name: "Broken Overlay".to_owned(),
                    source: OverlaySource::Html(HtmlOverlayConfig {
                        path: "overlay.html".to_owned(),
                        properties: HashMap::new(),
                        render_interval_ms: 1_000,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 16,
                        offset_y: 16,
                        width: 120,
                        height: 48,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                }],
            },
        )
        .await;

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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: Arc::clone(&overlay_runtime),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let runtime = wait_for_overlay_runtime(&overlay_runtime, device_id, slot_id).await;
    assert_eq!(runtime.status, OverlaySlotStatus::Failed);
    let error = runtime.last_error.as_deref().expect("error should exist");
    assert!(error.contains("overlay renderer is not implemented yet"));
    assert!(runtime.last_rendered_at.is_none());

    thread.shutdown().await.expect("display thread should stop");
    assert!(
        overlay_runtime.get(device_id).await.slot(slot_id).is_none(),
        "worker shutdown should clear runtime state"
    );
}

#[tokio::test]
async fn automatic_display_output_renders_clock_overlay_with_default_factory() {
    let _guard = default_text_overlay_test_lock().lock().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let overlay_runtime = default_display_overlay_runtime();
    let device_id = DeviceId::new();
    let slot_id = OverlaySlotId::from(Uuid::now_v7());

    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: slot_id,
                    name: "Clock Overlay".to_owned(),
                    source: OverlaySource::Clock(ClockConfig {
                        style: ClockStyle::Digital,
                        hour_format: HourFormat::TwentyFour,
                        show_seconds: true,
                        show_date: true,
                        date_format: Some("%Y-%m-%d".to_owned()),
                        font_family: None,
                        color: "#ffffff".to_owned(),
                        secondary_color: Some("#80ffea".to_owned()),
                        template: None,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 72,
                        offset_y: 48,
                        width: 240,
                        height: 120,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                }],
            },
        )
        .await;

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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: Arc::clone(&overlay_runtime),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes =
        wait_for_display_writes_with_timeout(&display_writes, Duration::from_secs(5)).await;
    let image = decode_jpeg(&writes[0]);
    assert!(
        region_contains_visible_pixels(&image, 72, 48, 240, 120),
        "expected clock overlay to paint visible pixels inside its bounds"
    );

    let runtime = wait_for_overlay_runtime(&overlay_runtime, device_id, slot_id).await;
    assert_eq!(runtime.status, OverlaySlotStatus::Active);
    assert!(runtime.last_error.is_none());
    assert!(runtime.last_rendered_at.is_some());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_renders_text_overlay_with_default_factory() {
    let _guard = default_text_overlay_test_lock().lock().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let overlay_runtime = default_display_overlay_runtime();
    let device_id = DeviceId::new();
    let slot_id = OverlaySlotId::from(Uuid::now_v7());

    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: slot_id,
                    name: "Text Overlay".to_owned(),
                    source: OverlaySource::Text(TextOverlayConfig {
                        text: "CPU {sensor:cpu_temp}".to_owned(),
                        font_family: None,
                        font_size: 32.0,
                        color: "#ffffff".to_owned(),
                        align: TextAlign::Center,
                        scroll: false,
                        scroll_speed: 30.0,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 72,
                        offset_y: 72,
                        width: 240,
                        height: 96,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                }],
            },
        )
        .await;

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

    let (sensor_tx, sensor_rx) = watch::channel(Arc::new(SystemSnapshot {
        cpu_temp_celsius: Some(72.0),
        ..SystemSnapshot::empty()
    }));
    let _ = Box::leak(Box::new(sensor_tx));

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: Arc::clone(&overlay_runtime),
        sensor_snapshot_rx: sensor_rx,
        overlay_factory: default_overlay_factory(),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes =
        wait_for_display_writes_with_timeout(&display_writes, Duration::from_secs(5)).await;
    let image = decode_jpeg(&writes[0]);
    assert!(
        region_contains_visible_pixels(&image, 72, 72, 240, 96),
        "expected text overlay to paint visible pixels inside its bounds"
    );

    let runtime = wait_for_overlay_runtime(&overlay_runtime, device_id, slot_id).await;
    assert_eq!(runtime.status, OverlaySlotStatus::Active);
    assert!(runtime.last_error.is_none());
    assert!(runtime.last_rendered_at.is_some());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_renders_sensor_overlay_with_default_factory() {
    let _guard = default_text_overlay_test_lock().lock().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let overlay_runtime = default_display_overlay_runtime();
    let device_id = DeviceId::new();
    let slot_id = OverlaySlotId::from(Uuid::now_v7());

    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: slot_id,
                    name: "Sensor Overlay".to_owned(),
                    source: OverlaySource::Sensor(SensorOverlayConfig {
                        sensor: "cpu_temp".to_owned(),
                        style: SensorDisplayStyle::Gauge,
                        unit_label: None,
                        range_min: 20.0,
                        range_max: 100.0,
                        color_min: "#80ffea".to_owned(),
                        color_max: "#ff6ac1".to_owned(),
                        font_family: None,
                        template: None,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 88,
                        offset_y: 56,
                        width: 176,
                        height: 176,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                }],
            },
        )
        .await;

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

    let (sensor_tx, sensor_rx) = watch::channel(Arc::new(SystemSnapshot {
        cpu_temp_celsius: Some(72.0),
        ..SystemSnapshot::empty()
    }));
    let _ = Box::leak(Box::new(sensor_tx));

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: Arc::clone(&overlay_runtime),
        sensor_snapshot_rx: sensor_rx,
        overlay_factory: default_overlay_factory(),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes =
        wait_for_display_writes_with_timeout(&display_writes, Duration::from_secs(5)).await;
    let image = decode_jpeg(&writes[0]);
    assert!(
        region_contains_visible_pixels(&image, 88, 56, 176, 176),
        "expected sensor overlay to paint visible pixels inside its bounds"
    );

    let runtime = wait_for_overlay_runtime(&overlay_runtime, device_id, slot_id).await;
    assert_eq!(runtime.status, OverlaySlotStatus::Active);
    assert!(runtime.last_error.is_none());
    assert!(runtime.last_rendered_at.is_some());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_renders_image_overlay_with_default_factory() {
    let event_bus = Arc::new(HypercolorBus::new());
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(
        Vec::new(),
    ))));
    let logical_devices = Arc::new(RwLock::new(HashMap::new()));
    let display_writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let device_registry = DeviceRegistry::new();
    let overlays = default_display_overlays();
    let overlay_runtime = default_display_overlay_runtime();
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let image_path = temp_dir.path().join("overlay.png");
    let device_id = DeviceId::new();
    let slot_id = OverlaySlotId::from(Uuid::now_v7());

    write_png(&image_path, [255, 0, 0, 160], 1, 1);
    overlays
        .set(
            device_id,
            DisplayOverlayConfig {
                overlays: vec![OverlaySlot {
                    id: slot_id,
                    name: "Image Overlay".to_owned(),
                    source: OverlaySource::Image(ImageOverlayConfig {
                        path: image_path.to_string_lossy().into_owned(),
                        speed: 1.0,
                        fit: ImageFit::Stretch,
                    }),
                    position: OverlayPosition::Anchored {
                        anchor: Anchor::TopLeft,
                        offset_x: 96,
                        offset_y: 96,
                        width: 120,
                        height: 120,
                    },
                    blend_mode: OverlayBlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                }],
            },
        )
        .await;

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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: Arc::clone(&overlays),
        display_overlay_runtime: Arc::clone(&overlay_runtime),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
    });

    let canvas = solid_canvas(Rgba::BLACK);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let inside_overlay = image.get_pixel(120, 120);
    let outside_overlay = image.get_pixel(16, 16);

    assert!(
        inside_overlay[0] > 90 && inside_overlay[1] < 40 && inside_overlay[2] < 40,
        "expected red-tinted image overlay pixel, got {inside_overlay:?}"
    );
    assert!(
        outside_overlay[0] < 30 && outside_overlay[1] < 30 && outside_overlay[2] < 30,
        "expected untouched black background outside overlay, got {outside_overlay:?}"
    );

    let runtime = wait_for_overlay_runtime(&overlay_runtime, device_id, slot_id).await;
    assert_eq!(runtime.status, OverlaySlotStatus::Active);
    assert!(runtime.last_error.is_none());
    assert!(runtime.last_rendered_at.is_some());

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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
async fn automatic_display_output_skips_metadata_only_owned_surface_updates() {
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
    });

    let surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 1, 16);
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_surface(surface.clone()));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    assert_eq!(writes.len(), 1);

    let _ = event_bus.canvas_sender().send(CanvasFrame::from_surface(
        surface.with_frame_metadata(2, 32),
    ));
    tokio::time::sleep(Duration::from_millis(140)).await;
    assert_eq!(
        display_writes.lock().await.len(),
        1,
        "metadata-only updates for owned surfaces should not trigger another LCD write"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_applies_device_brightness_before_encoding() {
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));

    let _ = device_registry
        .update_user_settings(&device_id, None, None, Some(0.0))
        .await;
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    let black_image = decode_jpeg(
        writes
            .last()
            .expect("expected zero-brightness display frame"),
    );
    let black_pixel = black_image.get_pixel(black_image.width() / 2, black_image.height() / 2);
    assert!(
        black_pixel[0] <= 8 && black_pixel[1] <= 8 && black_pixel[2] <= 8,
        "expected zero-brightness display output to stay black, got {black_pixel:?}"
    );

    let _ = device_registry
        .update_user_settings(&device_id, None, None, Some(0.5))
        .await;
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&red, 2, 32));
    let writes = wait_for_display_write_count(&display_writes, 2).await;
    let dimmed_image = decode_jpeg(
        writes
            .last()
            .expect("expected dimmed display frame after brightness update"),
    );
    let dimmed_pixel = dimmed_image.get_pixel(dimmed_image.width() / 2, dimmed_image.height() / 2);
    assert!(
        (90..=170).contains(&dimmed_pixel[0]) && dimmed_pixel[1] <= 32 && dimmed_pixel[2] <= 32,
        "expected half-bright red display output, got {dimmed_pixel:?}"
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
        device_settings: default_device_settings(),
        event_bus: Arc::clone(&event_bus),
        power_state,
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_overlays: default_display_overlays(),
        display_overlay_runtime: default_display_overlay_runtime(),
        sensor_snapshot_rx: default_sensor_snapshot_rx(),
        overlay_factory: default_overlay_factory(),
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
