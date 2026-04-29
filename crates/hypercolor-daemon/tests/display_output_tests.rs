use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::bus::{CanvasFrame, DisplayGroupTarget, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_driver_api::{BackendInfo, DeviceBackend};
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    OwnedDisplayFramePayload, ZoneInfo,
};
use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, RenderGroupId};
use hypercolor_types::session::OffOutputBehavior;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};

use hypercolor_daemon::display_frames::{DisplayFrameRuntime, DisplayFrameSnapshot};
use hypercolor_daemon::display_output::{DisplayOutputState, DisplayOutputThread};
use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::session::OutputPowerState;
use hypercolor_daemon::simulators::SIMULATED_DISPLAY_BACKEND_ID;

const DISPLAY_TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn display_output_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn display_output_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    display_output_test_lock().lock().await
}

async fn insert_default_logical_device(
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    device_id: DeviceId,
) -> String {
    let logical_id = format!("display:{device_id}");
    logical_devices.write().await.insert(
        logical_id.clone(),
        LogicalDevice {
            id: logical_id.clone(),
            physical_device_id: device_id,
            name: "Test Display".to_owned(),
            led_start: 0,
            led_count: 64,
            enabled: true,
            kind: LogicalDeviceKind::Default,
        },
    );
    logical_id
}

struct RecordingDisplayBackend {
    expected_device_id: DeviceId,
    backend_id: String,
    connected: bool,
    display_writes: Arc<Mutex<Vec<Vec<u8>>>>,
    display_write_times: Option<Arc<Mutex<Vec<Instant>>>>,
    display_write_attempt_times: Option<Arc<Mutex<Vec<Instant>>>>,
    write_delay: Duration,
    transient_display_failures: usize,
}

impl RecordingDisplayBackend {
    fn new(expected_device_id: DeviceId, display_writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        Self {
            expected_device_id,
            backend_id: "usb".to_owned(),
            connected: false,
            display_writes,
            display_write_times: None,
            display_write_attempt_times: None,
            write_delay: Duration::ZERO,
            transient_display_failures: 0,
        }
    }

    fn with_backend_id(mut self, backend_id: &str) -> Self {
        backend_id.clone_into(&mut self.backend_id);
        self
    }

    fn with_write_delay(mut self, write_delay: Duration) -> Self {
        self.write_delay = write_delay;
        self
    }

    fn with_timestamps(mut self, display_write_times: Arc<Mutex<Vec<Instant>>>) -> Self {
        self.display_write_times = Some(display_write_times);
        self
    }

    fn with_attempt_timestamps(
        mut self,
        display_write_attempt_times: Arc<Mutex<Vec<Instant>>>,
    ) -> Self {
        self.display_write_attempt_times = Some(display_write_attempt_times);
        self
    }

    fn with_transient_display_failures(mut self, failure_count: usize) -> Self {
        self.transient_display_failures = failure_count;
        self
    }

    async fn record_display_write_data(&mut self, id: &DeviceId, data: &[u8]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("display write while disconnected");
        }

        if let Some(display_write_attempt_times) = &self.display_write_attempt_times {
            display_write_attempt_times
                .lock()
                .await
                .push(Instant::now());
        }
        if self.transient_display_failures > 0 {
            self.transient_display_failures -= 1;
            bail!("intentional transient display write failure");
        }

        if !self.write_delay.is_zero() {
            tokio::time::sleep(self.write_delay).await;
        }

        self.display_writes.lock().await.push(data.to_vec());
        if let Some(display_write_times) = &self.display_write_times {
            display_write_times.lock().await.push(Instant::now());
        }
        Ok(())
    }
}

struct FailingDisplayBackend {
    expected_device_id: DeviceId,
    connected: bool,
}

impl FailingDisplayBackend {
    fn new(expected_device_id: DeviceId) -> Self {
        Self {
            expected_device_id,
            connected: false,
        }
    }
}

#[async_trait]
impl DeviceBackend for RecordingDisplayBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: self.backend_id.clone(),
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
        self.record_display_write_data(id, jpeg_data).await
    }

    async fn write_display_payload_owned(
        &mut self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        self.record_display_write_data(id, payload.data.as_slice())
            .await
    }
}

#[async_trait]
impl DeviceBackend for FailingDisplayBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "USB Failing".to_owned(),
            description: "Test backend that rejects display writes".to_owned(),
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

    async fn write_display_frame(&mut self, id: &DeviceId, _jpeg_data: &[u8]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("display write while disconnected");
        }

        bail!("intentional display write failure")
    }
}

fn display_device_info(
    device_id: DeviceId,
    has_display: bool,
    width: u32,
    height: u32,
    circular: bool,
) -> DeviceInfo {
    display_device_info_with_format_and_max_fps(
        device_id,
        has_display,
        width,
        height,
        circular,
        DeviceColorFormat::Jpeg,
        30,
    )
}

fn simulated_display_device_info(device_id: DeviceId, width: u32, height: u32) -> DeviceInfo {
    let mut info = display_device_info(device_id, true, width, height, true);
    "Simulated Display".clone_into(&mut info.name);
    "Hypercolor".clone_into(&mut info.vendor);
    info.family = DeviceFamily::named(SIMULATED_DISPLAY_BACKEND_ID.to_owned());
    info.connection_type = ConnectionType::Network;
    info.origin = DeviceOrigin::native(
        SIMULATED_DISPLAY_BACKEND_ID,
        SIMULATED_DISPLAY_BACKEND_ID,
        ConnectionType::Network,
    );
    info
}

fn display_device_info_with_max_fps(
    device_id: DeviceId,
    has_display: bool,
    width: u32,
    height: u32,
    circular: bool,
    max_fps: u32,
) -> DeviceInfo {
    display_device_info_with_format_and_max_fps(
        device_id,
        has_display,
        width,
        height,
        circular,
        DeviceColorFormat::Jpeg,
        max_fps,
    )
}

fn display_device_info_with_format(
    device_id: DeviceId,
    width: u32,
    height: u32,
    color_format: DeviceColorFormat,
) -> DeviceInfo {
    display_device_info_with_format_and_max_fps(
        device_id,
        true,
        width,
        height,
        false,
        color_format,
        30,
    )
}

fn display_device_info_with_format_and_max_fps(
    device_id: DeviceId,
    has_display: bool,
    width: u32,
    height: u32,
    circular: bool,
    color_format: DeviceColorFormat,
    max_fps: u32,
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
            color_format,
            layout_hint: None,
        }]
    } else {
        vec![ZoneInfo {
            name: "Ring".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Ring { count: 24 },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    };

    DeviceInfo {
        id: device_id,
        name: "Corsair Test Device".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("corsair", "usb", ConnectionType::Usb),
        zones,
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: if has_display { 0 } else { 24 },
            supports_direct: !has_display,
            supports_brightness: false,
            has_display,
            display_resolution: has_display.then_some((width, height)),
            max_fps,
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

fn simulated_display_metadata() -> HashMap<String, String> {
    HashMap::from([
        (
            "backend_id".to_owned(),
            SIMULATED_DISPLAY_BACKEND_ID.to_owned(),
        ),
        ("simulator".to_owned(), "true".to_owned()),
    ])
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
        brightness: None,
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
        brightness: None,
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

fn transparent_white_canvas() -> Canvas {
    let mut canvas = Canvas::new(320, 200);
    canvas.fill(Rgba::new(255, 255, 255, 0));
    canvas
}

fn publish_display_face_route(
    event_bus: &HypercolorBus,
    group_id: RenderGroupId,
    display_target: DisplayFaceTarget,
) {
    let display_target = display_target.normalized();
    event_bus.upsert_display_group_target(
        group_id,
        DisplayGroupTarget {
            device_id: display_target.device_id,
            blend_mode: display_target.blend_mode,
            opacity: display_target.opacity,
        },
    );
}

fn publish_direct_display_face_route(
    event_bus: &HypercolorBus,
    device_id: DeviceId,
    group_id: RenderGroupId,
) {
    publish_display_face_route(event_bus, group_id, DisplayFaceTarget::new(device_id));
}

async fn wait_for_display_writes(display_writes: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<Vec<u8>> {
    wait_for_display_writes_with_timeout(display_writes, DISPLAY_TEST_TIMEOUT).await
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
    tokio::time::timeout(DISPLAY_TEST_TIMEOUT, async {
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

async fn wait_for_display_attempt_count(
    display_write_attempt_times: &Arc<Mutex<Vec<Instant>>>,
    expected_count: usize,
) -> Vec<Instant> {
    tokio::time::timeout(DISPLAY_TEST_TIMEOUT, async {
        loop {
            let attempt_times = display_write_attempt_times.lock().await.clone();
            if attempt_times.len() >= expected_count {
                return attempt_times;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("display output should reach expected attempt count within timeout")
}

async fn wait_for_display_frame_snapshot(
    display_frames: &Arc<RwLock<DisplayFrameRuntime>>,
    device_id: DeviceId,
) -> DisplayFrameSnapshot {
    tokio::time::timeout(DISPLAY_TEST_TIMEOUT, async {
        loop {
            if let Some(frame) = display_frames.read().await.frame(device_id) {
                return frame;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("display preview frame should arrive within timeout")
}

async fn wait_for_scene_canvas_receiver_count(event_bus: &HypercolorBus, expected_count: usize) {
    tokio::time::timeout(DISPLAY_TEST_TIMEOUT, async {
        loop {
            if event_bus.scene_canvas_receiver_count() >= expected_count {
                return;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("authoritative scene canvas receiver should appear within timeout");
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
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: Some("hybrid-display".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("corsair", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
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
                layout_hint: None,
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
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let canvas = sample_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    assert!(!writes[0].is_empty());
    assert_eq!(&writes[0][..3], &[0xFF, 0xD8, 0xFF]);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_sends_raw_rgb_for_rgb_display_zones() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        .connect_device("usb", device_id, "push2:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info_with_format(
            device_id,
            320,
            200,
            DeviceColorFormat::Rgb,
        ))
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::clone(&display_frames),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let canvas = sample_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    assert_eq!(writes[0].len(), 320 * 200 * 3);
    assert_eq!(&writes[0][..3], &[255, 0, 0]);
    assert_ne!(&writes[0][..3], &[0xFF, 0xD8, 0xFF]);
    assert!(display_frames.read().await.frame(device_id).is_none());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_subscribes_to_authoritative_scene_canvas_not_preview_runtime() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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

    let preview_runtime = Arc::new(PreviewRuntime::new(Arc::clone(&event_bus)));
    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::clone(&preview_runtime),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;
    assert_eq!(event_bus.canvas_receiver_count(), 0);
    assert_eq!(preview_runtime.canvas_receiver_count(), 0);
    assert_eq!(preview_runtime.tracked_canvas_receiver_count(), 0);

    let canvas = sample_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));
    let _ = wait_for_display_writes(&display_writes).await;

    let preview_snapshot = preview_runtime.snapshot();
    assert_eq!(preview_snapshot.canvas_receivers, 0);
    assert_eq!(preview_snapshot.canvas_frames_published, 0);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_simulators_without_display_preview_subscribers() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(
        RecordingDisplayBackend::new(device_id, Arc::clone(&display_writes))
            .with_backend_id(SIMULATED_DISPLAY_BACKEND_ID),
    ));
    backend_manager
        .connect_device(
            SIMULATED_DISPLAY_BACKEND_ID,
            device_id,
            "simulator:test-display",
        )
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add_with_fingerprint_and_metadata(
            simulated_display_device_info(device_id, 480, 480),
            DeviceFingerprint(format!("simulator:{device_id}")),
            simulated_display_metadata(),
        )
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::clone(&display_frames),
    });

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&sample_canvas(), 1, 16));
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(display_writes.lock().await.is_empty());
    assert!(display_frames.read().await.frame(device_id).is_none());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_reacts_when_simulator_preview_subscriber_appears() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(
        RecordingDisplayBackend::new(device_id, Arc::clone(&display_writes))
            .with_backend_id(SIMULATED_DISPLAY_BACKEND_ID),
    ));
    backend_manager
        .connect_device(
            SIMULATED_DISPLAY_BACKEND_ID,
            device_id,
            "simulator:test-display",
        )
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add_with_fingerprint_and_metadata(
            simulated_display_device_info(device_id, 480, 480),
            DeviceFingerprint(format!("simulator:{device_id}")),
            simulated_display_metadata(),
        )
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::clone(&display_frames),
    });

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(display_writes.lock().await.is_empty());

    let _preview_rx = display_frames.write().await.subscribe(device_id);
    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 255, 0, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    assert!(!writes.is_empty());
    assert!(display_frames.read().await.frame(device_id).is_some());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_devices_without_display_capabilities() {
    let _guard = display_output_test_guard().await;
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let canvas = sample_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(display_writes.lock().await.is_empty());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_display_devices_that_are_not_in_layout() {
    let _guard = display_output_test_guard().await;
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let canvas = sample_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(display_writes.lock().await.is_empty());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_uses_layout_zone_viewport() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let canvas = split_color_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));

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
async fn automatic_display_output_uses_logical_device_viewport_alias() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = "desk-left-display".to_owned();

    {
        let mut store = logical_devices.write().await;
        store.insert(
            logical_id.clone(),
            LogicalDevice {
                id: logical_id.clone(),
                physical_device_id: device_id,
                name: "Desk Left Display".to_owned(),
                led_start: 0,
                led_count: 0,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );
    }

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let canvas = split_color_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > 200,
        "expected logical-device viewport to keep the red half, got {pixel:?}"
    );
    assert!(
        pixel[2] < 80,
        "expected logical-device viewport to exclude the blue half, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_defaults_mixed_devices_to_full_canvas_without_display_zone() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![led_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let canvas = split_color_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));

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
async fn display_group_canvas_routes_to_device_worker() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_direct_display_face_route(event_bus.as_ref(), device_id, group_id);

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            1,
            16,
        ));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[2] > 200,
        "expected direct display-face canvas to win over the red scene canvas, got {pixel:?}"
    );
    assert!(
        pixel[0] < 80,
        "expected direct display-face canvas to bypass the scene canvas path, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_updates_direct_faces_without_scene_canvas_ticks() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_direct_display_face_route(event_bus.as_ref(), device_id, group_id);

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            1,
            16,
        ));
    let first_writes = wait_for_display_write_count(&display_writes, 1).await;
    let first_image = decode_jpeg(&first_writes[0]);
    let first_pixel = first_image.get_pixel(first_image.width() / 2, first_image.height() / 2);
    assert!(first_pixel[2] > 200);

    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 255, 0, 255)),
            2,
            32,
        ));
    let writes = wait_for_display_write_count(&display_writes, 2).await;
    let second_image = decode_jpeg(&writes[1]);
    let second_pixel = second_image.get_pixel(second_image.width() / 2, second_image.height() / 2);

    assert!(
        second_pixel[1] > 200,
        "expected direct face updates to keep flowing without scene canvas wakeups, got {second_pixel:?}"
    );
    assert!(
        second_pixel[2] < 80,
        "expected the newer green face frame to replace the stale blue frame, got {second_pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_alpha_blends_face_with_effect_canvas() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Alpha,
            opacity: 0.5,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > 70 && pixel[2] > 70,
        "expected alpha blend to preserve both effect and face colors, got {pixel:?}"
    );
    assert!(
        pixel[1] < 60,
        "expected the red/blue blend to stay magenta rather than drifting green, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_alpha_waits_for_effect_frame_before_blending() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Alpha,
            opacity: 0.5,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        display_writes.lock().await.is_empty(),
        "expected blended display faces to wait for an effect frame before publishing"
    );

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            2,
            32,
        ));
    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > 70 && pixel[2] > 70,
        "expected the first published blended frame to include both effect and face colors, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_output_uses_render_published_face_route_metadata() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    event_bus.upsert_display_group_target(
        group_id,
        DisplayGroupTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Replace,
            opacity: 1.0,
        },
    );

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[2] > 200,
        "expected render-published replace metadata to preserve the direct face color, got {pixel:?}"
    );
    assert!(
        pixel[0] < 80,
        "expected render-published replace metadata to keep display routing deterministic, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_replace_keeps_transparent_face_pixels_from_bleeding_effect_canvas() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_direct_display_face_route(event_bus.as_ref(), device_id, group_id);

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(&transparent_white_canvas(), 2, 32));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] < 40 && pixel[1] < 40 && pixel[2] < 40,
        "expected replace mode to isolate the face and keep fully transparent pixels dark, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alpha_display_faces_keep_default_30_fps_cadence_on_60_fps_devices() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let display_write_times = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(
        RecordingDisplayBackend::new(device_id, Arc::clone(&display_writes))
            .with_timestamps(Arc::clone(&display_write_times)),
    ));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info_with_max_fps(
            device_id, true, 320, 320, true, 60,
        ))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Alpha,
            opacity: 0.5,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            1,
            16,
        ));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    let _ = wait_for_display_write_count(&display_writes, 1).await;

    display_writes.lock().await.clear();
    display_write_times.lock().await.clear();

    for frame in 0_u32..12 {
        let red = u8::try_from(20_u32.saturating_mul(frame.saturating_add(1))).unwrap_or(u8::MAX);
        event_bus
            .scene_canvas_sender()
            .send_replace(CanvasFrame::from_canvas(
                &solid_canvas(Rgba::new(red, 0, 0, 255)),
                frame.saturating_add(2),
                frame.saturating_add(2).saturating_mul(16),
            ));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    tokio::time::timeout(DISPLAY_TEST_TIMEOUT, async {
        loop {
            if display_write_times.lock().await.len() >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("display output should produce multiple writes within timeout");

    let write_times = display_write_times.lock().await.clone();
    let cadence = write_times[1].saturating_duration_since(write_times[0]);
    assert!(
        cadence >= Duration::from_millis(24),
        "expected alpha display-face cadence to stay near 30 fps on a 60 fps device, got {cadence:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_screen_blends_face_color_with_effect_canvas() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Screen,
            opacity: 1.0,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > 200 && pixel[2] > 200,
        "expected screen blend to saturate both effect and face channels, got {pixel:?}"
    );
    assert!(
        pixel[1] < 50,
        "expected screen blend to keep the red/blue mix magenta, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_tint_turns_face_into_effect_tinted_material() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Tint,
            opacity: 1.0,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 255, 255, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(0, 0, 255, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[2] > pixel[0] + 35 && pixel[2] > pixel[1] + 35,
        "expected tint blend to bias the effect toward the face chroma, got {pixel:?}"
    );
    assert!(
        pixel[0] > 35 && pixel[1] > 35,
        "expected tint blend to behave like filtered material rather than hard replace, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn display_group_luma_reveal_lets_bright_face_regions_adopt_effect_color() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();

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
        .add(display_device_info(device_id, true, 320, 320, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    publish_display_face_route(
        event_bus.as_ref(),
        group_id,
        DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::LumaReveal,
            opacity: 1.0,
        },
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 0, 0, 255)),
            1,
            16,
        ));
    tokio::time::sleep(Duration::from_millis(40)).await;
    display_writes.lock().await.clear();
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(
            &solid_canvas(Rgba::new(255, 255, 255, 255)),
            2,
            32,
        ));

    let writes = wait_for_display_writes(&display_writes).await;
    let image = decode_jpeg(&writes[0]);
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);

    assert!(
        pixel[0] > pixel[1] + 50 && pixel[0] > pixel[2] + 50,
        "expected luma reveal to let bright face pixels inherit the effect hue, got {pixel:?}"
    );
    assert!(
        pixel[1] < 190 && pixel[2] < 190,
        "expected luma reveal to avoid a flat white replace result, got {pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_drops_stale_frames_for_slow_displays() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let green = solid_canvas(Rgba::new(0, 255, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
    tokio::time::sleep(Duration::from_millis(20)).await;
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&green, 2, 32));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&blue, 3, 48));

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
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let green = solid_canvas(Rgba::new(0, 255, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
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

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&green, 2, 32));
    tokio::time::sleep(Duration::from_millis(20)).await;
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&blue, 3, 48));

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
async fn automatic_display_output_keeps_preview_frame_when_backend_write_fails() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(FailingDisplayBackend::new(device_id)));
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::clone(&display_frames),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));

    let frame = wait_for_display_frame_snapshot(&display_frames, device_id).await;
    let image = decode_jpeg(frame.jpeg_data.as_slice());
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);
    assert!(
        pixel[0] > 200,
        "expected preview frame to preserve the rendered red image, got {pixel:?}"
    );
    assert_eq!(frame.width, 320);
    assert_eq!(frame.height, 200);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_retries_unchanged_frame_after_transient_write_failure() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let display_write_attempt_times = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        )]));
    }

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(
        RecordingDisplayBackend::new(device_id, Arc::clone(&display_writes))
            .with_attempt_timestamps(Arc::clone(&display_write_attempt_times))
            .with_transient_display_failures(1),
    ));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info_with_max_fps(
            device_id, true, 320, 200, false, 10,
        ))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let display_frames = Arc::new(RwLock::new(DisplayFrameRuntime::new()));
    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        spatial_engine: Arc::clone(&spatial_engine),
        logical_devices: Arc::clone(&logical_devices),
        event_bus: Arc::clone(&event_bus),
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::clone(&display_frames),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));

    let writes = wait_for_display_write_count(&display_writes, 1).await;
    let attempt_times = wait_for_display_attempt_count(&display_write_attempt_times, 2).await;
    assert_eq!(writes.len(), 1);
    assert_eq!(attempt_times.len(), 2);
    assert!(
        attempt_times[1].duration_since(attempt_times[0]) >= Duration::from_millis(70),
        "retry should wait for target cadence instead of spinning"
    );

    let image = decode_jpeg(writes.first().expect("expected retried display frame"));
    let pixel = image.get_pixel(image.width() / 2, image.height() / 2);
    assert!(
        pixel[0] > 200,
        "expected retried unchanged display frame to be red, got {pixel:?}"
    );

    let metrics = display_frames.read().await.metrics_snapshot();
    assert_eq!(metrics.write_attempts_total, 2);
    assert_eq!(metrics.write_successes_total, 1);
    assert_eq!(metrics.write_failures_total, 1);
    assert_eq!(metrics.retry_attempts_total, 1);
    assert!(metrics.last_failure_age_ms.is_some());

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_unchanged_frames() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    assert_eq!(writes.len(), 1);

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 2, 32));
    tokio::time::sleep(Duration::from_millis(140)).await;
    assert_eq!(
        display_writes.lock().await.len(),
        1,
        "identical frame should not trigger another LCD write"
    );

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&blue, 3, 48));
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
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 1, 16);
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_surface(surface.clone()));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    assert_eq!(writes.len(), 1);

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_surface(
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
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));

    let _ = device_registry
        .update_user_settings(&device_id, None, None, Some(0.0))
        .await;
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
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
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 2, 32));
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
async fn automatic_display_output_skips_repeated_zero_brightness_frames() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    let _ = device_registry
        .update_user_settings(&device_id, None, None, Some(0.0))
        .await;
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    assert_eq!(writes.len(), 1);

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&blue, 2, 32));
    tokio::time::sleep(Duration::from_millis(140)).await;
    assert_eq!(
        display_writes.lock().await.len(),
        1,
        "zero-brightness output should reuse the cached black frame",
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_refreshes_cached_targets_when_layout_changes() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let canvas = split_color_canvas();
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 1, 16));
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
            logical_id.as_str(),
            NormalizedPosition::new(0.75, 0.5),
            NormalizedPosition::new(0.5, 1.0),
        )]));
    }

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&canvas, 2, 32));
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
async fn automatic_display_output_refreshes_cached_targets_when_display_face_route_changes() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let group_id = RenderGroupId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state: default_power_state_rx(),
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    let red = solid_canvas(Rgba::new(255, 0, 0, 255));
    let blue = solid_canvas(Rgba::new(0, 0, 255, 255));

    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 1, 16));
    let writes = wait_for_display_write_count(&display_writes, 1).await;
    let first_image = decode_jpeg(writes.first().expect("expected initial display frame"));
    let first_pixel = first_image.get_pixel(first_image.width() / 2, first_image.height() / 2);
    assert!(
        first_pixel[0] > 200,
        "expected scene canvas route to start red, got {first_pixel:?}"
    );

    publish_direct_display_face_route(event_bus.as_ref(), device_id, group_id);
    event_bus
        .group_canvas_sender(group_id)
        .send_replace(CanvasFrame::from_canvas(&blue, 2, 32));
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&red, 3, 48));

    let writes = wait_for_display_write_count(&display_writes, 2).await;
    let second_image = decode_jpeg(writes.last().expect("expected refreshed direct-face frame"));
    let second_pixel = second_image.get_pixel(second_image.width() / 2, second_image.height() / 2);
    assert!(
        second_pixel[2] > 200,
        "expected face-route refresh to switch the cached target to blue, got {second_pixel:?}"
    );

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_refreshes_static_hold_frames_while_sleeping() {
    let _guard = display_output_test_guard().await;
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(layout_with_zones(vec![]))));
    let logical_devices = Arc::new(RwLock::new(HashMap::<String, LogicalDevice>::new()));
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();
    let logical_id = insert_default_logical_device(&logical_devices, device_id).await;
    let (power_tx, power_state) = watch::channel(OutputPowerState::default());

    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout_with_zones(vec![display_zone(
            logical_id.as_str(),
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
        preview_runtime: Arc::new(PreviewRuntime::new(Arc::clone(&event_bus))),
        power_state,
        static_hold_refresh_interval: TEST_STATIC_HOLD_REFRESH_INTERVAL,
        display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
    });

    wait_for_scene_canvas_receiver_count(event_bus.as_ref(), 1).await;

    let black = solid_canvas(Rgba::BLACK);
    event_bus
        .scene_canvas_sender()
        .send_replace(CanvasFrame::from_canvas(&black, 1, 16));
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
