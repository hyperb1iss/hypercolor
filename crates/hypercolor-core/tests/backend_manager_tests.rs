//! Tests for the `BackendManager` — device routing and frame dispatch.

use std::io;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend, SegmentRange};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    ZoneAttachment,
};
use tokio::sync::Mutex;
use tracing_subscriber::fmt::writer::MakeWriter;

// ── Slow Test Backend ────────────────────────────────────────────────────────

struct SlowRecordingBackend {
    expected_device_id: DeviceId,
    delay: Duration,
    writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
    write_count: Arc<AtomicUsize>,
    target_fps: Option<u32>,
    write_times: Option<Arc<Mutex<Vec<Instant>>>>,
}

impl SlowRecordingBackend {
    fn new(
        expected_device_id: DeviceId,
        delay: Duration,
        writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
        write_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            expected_device_id,
            delay,
            writes,
            write_count,
            target_fps: None,
            write_times: None,
        }
    }

    fn with_target_fps(mut self, target_fps: u32) -> Self {
        self.target_fps = Some(target_fps);
        self
    }

    fn with_write_times(mut self, write_times: Arc<Mutex<Vec<Instant>>>) -> Self {
        self.write_times = Some(write_times);
        self
    }
}

#[async_trait::async_trait]
impl DeviceBackend for SlowRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "slow".to_owned(),
            name: "Slow Recording Backend".to_owned(),
            description: "Sleeps during writes to test non-blocking dispatch".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: self.expected_device_id,
            name: "Slow Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("Test".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 10,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }])
    }

    async fn connect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        tokio::time::sleep(self.delay).await;
        self.write_count.fetch_add(1, Ordering::Relaxed);
        if let Some(write_times) = &self.write_times {
            write_times.lock().await.push(Instant::now());
        }
        self.writes.lock().await.push(colors.to_vec());
        Ok(())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        if *id == self.expected_device_id {
            self.target_fps
        } else {
            None
        }
    }
}

struct DirectControlRecordingBackend {
    expected_device_id: DeviceId,
    connected: bool,
    writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
    brightness_writes: Arc<Mutex<Vec<u8>>>,
}

impl DirectControlRecordingBackend {
    fn new(
        expected_device_id: DeviceId,
        writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
        brightness_writes: Arc<Mutex<Vec<u8>>>,
    ) -> Self {
        Self {
            expected_device_id,
            connected: false,
            writes,
            brightness_writes,
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for DirectControlRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "recording".to_owned(),
            name: "Direct Control Recording Backend".to_owned(),
            description: "Records direct writes and brightness changes for tests".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: self.expected_device_id,
            name: "Recording Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("Test".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 4,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
            },
        }])
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

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("write while disconnected");
        }

        self.writes.lock().await.push(colors.to_vec());
        Ok(())
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("brightness change while disconnected");
        }

        self.brightness_writes.lock().await.push(brightness);
        Ok(())
    }
}

struct MetadataRefreshingBackend {
    expected_device_id: DeviceId,
    connected: bool,
    refreshed_info: DeviceInfo,
}

impl MetadataRefreshingBackend {
    fn new(expected_device_id: DeviceId) -> Self {
        Self {
            expected_device_id,
            connected: false,
            refreshed_info: DeviceInfo {
                id: expected_device_id,
                name: "Connected Metadata Device".to_owned(),
                vendor: "Test".to_owned(),
                family: DeviceFamily::Custom("Test".to_owned()),
                model: Some("Connected".to_owned()),
                connection_type: ConnectionType::Network,
                zones: vec![
                    ZoneInfo {
                        name: "Pump Ring".to_owned(),
                        led_count: 4,
                        topology: DeviceTopologyHint::Ring { count: 4 },
                        color_format: DeviceColorFormat::Rgb,
                    },
                    ZoneInfo {
                        name: "Case Strip".to_owned(),
                        led_count: 8,
                        topology: DeviceTopologyHint::Strip,
                        color_format: DeviceColorFormat::Rgb,
                    },
                ],
                firmware_version: Some("2.0.0".to_owned()),
                capabilities: DeviceCapabilities {
                    led_count: 12,
                    supports_direct: true,
                    supports_brightness: false,
                    has_display: false,
                    display_resolution: None,
                    max_fps: 30,
                },
            },
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for MetadataRefreshingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "metadata".to_owned(),
            name: "Metadata Refreshing Backend".to_owned(),
            description: "Returns connected-device metadata after handshake".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: self.expected_device_id,
            name: "Initial Metadata Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("Test".to_owned()),
            model: Some("Initial".to_owned()),
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("1.0.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 1,
                supports_direct: true,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
            },
        }])
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        Ok(self.connected.then_some(self.refreshed_info.clone()))
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
}

struct DisplayRecordingBackend {
    expected_device_id: DeviceId,
    connected: bool,
    display_writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl DisplayRecordingBackend {
    fn new(expected_device_id: DeviceId, display_writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        Self {
            expected_device_id,
            connected: false,
            display_writes,
        }
    }
}

struct DiscoverRetryBackend {
    expected_device_id: DeviceId,
    connected: bool,
    connect_attempts: u32,
    discover_count: u32,
    target_fps: u32,
}

impl DiscoverRetryBackend {
    fn new(expected_device_id: DeviceId, target_fps: u32) -> Self {
        Self {
            expected_device_id,
            connected: false,
            connect_attempts: 0,
            discover_count: 0,
            target_fps,
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for DiscoverRetryBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "retry".to_owned(),
            name: "Discover Retry Backend".to_owned(),
            description: "Fails the first connect and succeeds after discover".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        self.discover_count = self.discover_count.saturating_add(1);
        Ok(vec![DeviceInfo {
            id: self.expected_device_id,
            name: "Retry Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("Test".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 4,
                supports_direct: true,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: self.target_fps,
            },
        }])
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        self.connect_attempts = self.connect_attempts.saturating_add(1);
        if self.connect_attempts == 1 {
            bail!("first connect attempt should fail");
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
        if !self.connected {
            bail!("write while disconnected");
        }

        Ok(())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        (*id == self.expected_device_id && self.connected).then_some(self.target_fps)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for DisplayRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "display".to_owned(),
            name: "Display Recording Backend".to_owned(),
            description: "Records display payloads for tests".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![DeviceInfo {
            id: self.expected_device_id,
            name: "Display Device".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::Custom("Test".to_owned()),
            model: None,
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Display".to_owned(),
                led_count: 0,
                topology: DeviceTopologyHint::Display {
                    width: 480,
                    height: 480,
                    circular: true,
                },
                color_format: DeviceColorFormat::Jpeg,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 0,
                supports_direct: false,
                supports_brightness: false,
                has_display: true,
                display_resolution: Some((480, 480)),
                max_fps: 30,
            },
        }])
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

        self.display_writes.lock().await.push(jpeg_data.to_vec());
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<StdMutex<Vec<u8>>>);

struct SharedLogWriter {
    buffer: Arc<StdMutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .expect("shared log buffer lock should not be poisoned")
                .clone(),
        )
        .expect("captured test logs should be valid UTF-8")
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            buffer: Arc::clone(&self.0),
        }
    }
}

impl io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer
            .lock()
            .expect("shared log buffer lock should not be poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn make_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test-layout".into(),
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

fn make_zone(id: &str, device_id: &str, led_count: u32) -> DeviceZone {
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

// ── Registration Tests ──────────────────────────────────────────────────────

#[test]
fn new_manager_is_empty() {
    let manager = BackendManager::new();
    assert_eq!(manager.backend_count(), 0);
    assert_eq!(manager.mapped_device_count(), 0);
}

#[test]
fn register_backend() {
    let mut manager = BackendManager::new();
    let backend = MockDeviceBackend::new();
    manager.register_backend(Box::new(backend));

    assert_eq!(manager.backend_count(), 1);
    let ids = manager.backend_ids();
    assert!(ids.contains(&"mock"));
}

#[test]
fn register_replaces_existing_backend() {
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(MockDeviceBackend::new()));
    manager.register_backend(Box::new(MockDeviceBackend::new()));

    // Still only one backend — replaced, not duplicated.
    assert_eq!(manager.backend_count(), 1);
}

#[test]
fn routing_snapshot_marks_registered_backend() {
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(MockDeviceBackend::new()));

    let device_id = DeviceId::new();
    manager.map_device("mock:device_1", "mock", device_id);

    let routing = manager.routing_snapshot();
    assert_eq!(routing.backend_ids, vec!["mock".to_string()]);
    assert_eq!(routing.mapping_count, 1);
    assert!(routing.mappings[0].backend_registered);
}

#[test]
fn debug_snapshot_is_empty_for_new_manager() {
    let manager = BackendManager::new();
    let snapshot = manager.debug_snapshot();
    assert_eq!(snapshot.queue_count, 0);
    assert_eq!(snapshot.mapped_device_count, 0);
    assert!(snapshot.queues.is_empty());
}

#[test]
fn routing_snapshot_is_empty_for_new_manager() {
    let manager = BackendManager::new();
    let snapshot = manager.routing_snapshot();
    assert_eq!(snapshot.backend_ids.len(), 0);
    assert_eq!(snapshot.mapping_count, 0);
    assert_eq!(snapshot.queue_count, 0);
    assert!(snapshot.mappings.is_empty());
    assert!(snapshot.orphaned_queues.is_empty());
}

// ── Device Mapping Tests ────────────────────────────────────────────────────

#[test]
fn map_and_unmap_device() {
    let mut manager = BackendManager::new();
    let device_id = DeviceId::new();

    manager.map_device("wled:strip_1", "wled", device_id);
    assert_eq!(manager.mapped_device_count(), 1);
    let routing = manager.routing_snapshot();
    assert_eq!(routing.mapping_count, 1);
    assert_eq!(routing.mappings[0].layout_device_id, "wled:strip_1");
    assert_eq!(routing.mappings[0].backend_id, "wled");
    assert_eq!(routing.mappings[0].device_id, device_id.to_string());
    assert!(!routing.mappings[0].backend_registered);
    assert!(!routing.mappings[0].queue_active);

    assert!(manager.unmap_device("wled:strip_1"));
    assert_eq!(manager.mapped_device_count(), 0);

    // Second unmap returns false.
    assert!(!manager.unmap_device("wled:strip_1"));
}

#[test]
fn unmap_device_clears_cached_target_fps_when_last_mapping_is_removed() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));
    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(5),
        Arc::clone(&writes),
        Arc::clone(&write_count),
    )
    .with_target_fps(33);

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let runtime = tokio::runtime::Runtime::new().expect("create tokio runtime");
    runtime
        .block_on(manager.connect_device("slow", device_id, "slow:cache-cleanup"))
        .expect("connect should succeed");

    assert_eq!(manager.cached_target_fps("slow", device_id), Some(33));
    assert!(manager.unmap_device("slow:cache-cleanup"));
    assert_eq!(manager.cached_target_fps("slow", device_id), None);
}

#[tokio::test]
async fn connect_device_connects_backend_and_maps_layout_device() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Connect Flow Device".into(),
        led_count: 6,
        topology: LedTopology::Strip {
            count: 6,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let backend = MockDeviceBackend::new().with_device(&mock_config);
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    manager
        .connect_device("mock", device_id, "mock:connect-flow")
        .await
        .expect("connect_device should connect and map");

    assert_eq!(manager.mapped_device_count(), 1);

    let layout = make_layout(vec![make_zone("zone_0", "mock:connect-flow", 6)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[12, 34, 56]; 6],
    }];
    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 6);
    assert!(stats.errors.is_empty());
}

#[tokio::test]
async fn connect_device_fails_for_unknown_backend() {
    let mut manager = BackendManager::new();

    let error = manager
        .connect_device("missing-backend", DeviceId::new(), "missing:device")
        .await
        .expect_err("unknown backend should fail");
    assert!(
        error.to_string().contains("not registered"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn disconnect_device_disconnects_and_unmaps_layout_device() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Disconnect Flow Device".into(),
        led_count: 5,
        topology: LedTopology::Strip {
            count: 5,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let backend = MockDeviceBackend::new().with_device(&mock_config);
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    manager
        .connect_device("mock", device_id, "mock:disconnect-flow")
        .await
        .expect("connect should succeed");
    assert_eq!(manager.mapped_device_count(), 1);

    manager
        .disconnect_device("mock", device_id, "mock:disconnect-flow")
        .await
        .expect("disconnect should succeed");
    assert_eq!(manager.mapped_device_count(), 0);

    let layout = make_layout(vec![make_zone("zone_0", "mock:disconnect-flow", 5)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[200, 200, 200]; 5],
    }];
    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 0);
    assert!(stats.errors.is_empty());
}

#[tokio::test]
async fn connect_device_caches_backend_target_fps_for_output_queue() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));
    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(5),
        Arc::clone(&writes),
        Arc::clone(&write_count),
    )
    .with_target_fps(37);

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager
        .connect_device("slow", device_id, "slow:fps-cache")
        .await
        .expect("connect should succeed");

    let layout = make_layout(vec![make_zone("zone_0", "slow:fps-cache", 4)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[1, 2, 3]; 4],
    }];
    manager.write_frame(&zone_colors, &layout).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    assert_eq!(manager.cached_target_fps("slow", device_id), Some(37));

    let snapshot = manager.debug_snapshot();
    let queue = snapshot
        .queues
        .first()
        .expect("expected one queue snapshot");
    assert_eq!(queue.target_fps, 37);
    assert_eq!(queue.frames_sent, 1);
}

#[tokio::test]
async fn disconnect_device_surfaces_backend_errors() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Disconnect Error Device".into(),
        led_count: 3,
        topology: LedTopology::Strip {
            count: 3,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let backend = MockDeviceBackend::new().with_device(&mock_config);
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let error = manager
        .disconnect_device("mock", device_id, "mock:error")
        .await
        .expect_err("disconnect of non-connected device should fail");
    assert!(
        error.to_string().contains("failed to disconnect device"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn write_device_colors_writes_immediately_to_connected_device() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Direct Write Device".into(),
        led_count: 4,
        topology: LedTopology::Strip {
            count: 4,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    manager
        .write_device_colors("mock", device_id, &[[1, 2, 3]; 4])
        .await
        .expect("direct write should succeed");
}

#[tokio::test]
async fn backend_io_connect_with_refresh_retries_and_caches_target_fps() {
    let device_id = DeviceId::new();
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(DiscoverRetryBackend::new(device_id, 48)));

    let io = manager
        .backend_io("retry")
        .expect("backend io handle should exist");
    let target_fps = io
        .connect_with_refresh(device_id)
        .await
        .expect("connect with refresh should succeed");

    manager.set_cached_target_fps("retry", device_id, target_fps);
    manager.map_device("retry:device", "retry", device_id);

    assert_eq!(target_fps, 48);
    assert_eq!(manager.cached_target_fps("retry", device_id), Some(48));
}

#[tokio::test]
async fn backend_io_write_colors_targets_backend_directly() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let brightness_writes = Arc::new(Mutex::new(Vec::new()));
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(DirectControlRecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&brightness_writes),
    )));

    let io = manager
        .backend_io("recording")
        .expect("backend io handle should exist");
    io.connect_with_refresh(device_id)
        .await
        .expect("connect should succeed");
    io.write_colors(device_id, &[[9, 8, 7]; 4])
        .await
        .expect("direct write should succeed");

    assert_eq!(*writes.lock().await, vec![vec![[9, 8, 7]; 4]]);
    assert!(brightness_writes.lock().await.is_empty());
}

#[tokio::test]
async fn backend_io_disconnect_stops_future_direct_writes() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let brightness_writes = Arc::new(Mutex::new(Vec::new()));
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(DirectControlRecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&brightness_writes),
    )));

    let io = manager
        .backend_io("recording")
        .expect("backend io handle should exist");
    io.connect_with_refresh(device_id)
        .await
        .expect("connect should succeed");
    io.disconnect(device_id)
        .await
        .expect("disconnect should succeed");

    let error = io
        .write_colors(device_id, &[[1, 2, 3]; 4])
        .await
        .expect_err("writes should fail after disconnect");
    assert!(error.to_string().contains("failed to write"));
}

#[tokio::test]
async fn backend_io_connected_device_info_returns_backend_metadata() {
    let device_id = DeviceId::new();
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(MetadataRefreshingBackend::new(device_id)));

    let io = manager
        .backend_io("metadata")
        .expect("backend io handle should exist");
    io.connect_with_refresh(device_id)
        .await
        .expect("connect should succeed");

    let info = io
        .connected_device_info(device_id)
        .await
        .expect("metadata refresh should succeed")
        .expect("connected metadata should exist");

    assert_eq!(info.name, "Connected Metadata Device");
    assert_eq!(info.capabilities.led_count, 12);
}

#[tokio::test]
async fn backend_io_write_display_frame_targets_backend_directly() {
    let device_id = DeviceId::new();
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(DisplayRecordingBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));

    let io = manager
        .backend_io("display")
        .expect("backend io handle should exist");
    io.connect_with_refresh(device_id)
        .await
        .expect("connect should succeed");

    let jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xD9];
    io.write_display_frame(device_id, &jpeg_data)
        .await
        .expect("display write should succeed");

    assert_eq!(*display_writes.lock().await, vec![jpeg_data]);
}

#[tokio::test]
async fn write_device_colors_fails_for_unknown_backend() {
    let mut manager = BackendManager::new();
    let error = manager
        .write_device_colors("missing", DeviceId::new(), &[[0, 0, 0]; 1])
        .await
        .expect_err("missing backend should fail");

    assert!(
        error.to_string().contains("not registered"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn set_device_brightness_targets_backend_directly() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let brightness_writes = Arc::new(Mutex::new(Vec::new()));

    let mut backend = DirectControlRecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&brightness_writes),
    );
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    manager
        .set_device_brightness("recording", device_id, 128)
        .await
        .expect("brightness write should succeed");

    assert!(
        writes.lock().await.is_empty(),
        "brightness should not write colors"
    );
    assert_eq!(*brightness_writes.lock().await, vec![128]);
}

#[tokio::test]
async fn connected_device_info_returns_backend_metadata() {
    let device_id = DeviceId::new();
    let mut backend = MetadataRefreshingBackend::new(device_id);
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let info = manager
        .connected_device_info("metadata", device_id)
        .await
        .expect("metadata lookup should succeed")
        .expect("connected device metadata should exist");

    assert_eq!(info.id, device_id);
    assert_eq!(info.name, "Connected Metadata Device");
    assert_eq!(info.firmware_version.as_deref(), Some("2.0.0"));
    assert_eq!(info.zones.len(), 2);
    assert_eq!(
        info.zones[0].topology,
        DeviceTopologyHint::Ring { count: 4 }
    );
    assert_eq!(info.capabilities.led_count, 12);
    assert_eq!(info.capabilities.max_fps, 30);
}

#[tokio::test]
async fn write_device_display_frame_targets_backend_directly() {
    let device_id = DeviceId::new();
    let display_writes = Arc::new(Mutex::new(Vec::new()));

    let mut backend = DisplayRecordingBackend::new(device_id, Arc::clone(&display_writes));
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xDB];
    manager
        .write_device_display_frame("display", device_id, &jpeg_data)
        .await
        .expect("display write should succeed");

    assert_eq!(*display_writes.lock().await, vec![jpeg_data]);
}

#[tokio::test]
async fn direct_control_suppresses_queued_writes_until_released() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let brightness_writes = Arc::new(Mutex::new(Vec::new()));

    let mut backend = DirectControlRecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&brightness_writes),
    );
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("recording:device", "recording", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "recording:device", 4)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[9, 9, 9]; 4],
    }];

    assert_eq!(manager.begin_direct_control("recording", device_id), 1);
    assert!(manager.is_direct_control_active("recording", device_id));

    let suppressed_stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(suppressed_stats.devices_written, 0);
    assert_eq!(suppressed_stats.total_leds, 0);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        writes.lock().await.is_empty(),
        "queued writes should be suppressed"
    );

    manager
        .write_device_colors("recording", device_id, &[[1, 2, 3]; 4])
        .await
        .expect("direct writes should still succeed");
    assert_eq!(*writes.lock().await, vec![vec![[1, 2, 3]; 4]]);

    assert_eq!(manager.end_direct_control("recording", device_id), 0);
    assert!(!manager.is_direct_control_active("recording", device_id));

    let resumed_stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(resumed_stats.devices_written, 1);
    assert_eq!(resumed_stats.total_leds, 4);
    tokio::time::sleep(Duration::from_millis(30)).await;

    let recorded = writes.lock().await.clone();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1], vec![[9, 9, 9]; 4]);
    assert!(
        brightness_writes.lock().await.is_empty(),
        "direct-control suppression should not touch brightness"
    );
}

// ── write_frame Tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn write_frame_routes_to_correct_backend() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Test Strip".into(),
        led_count: 10,
        topology: LedTopology::Strip {
            count: 10,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("mock:strip_1", "mock", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "mock:strip_1", 10)]);

    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[255, 0, 0]; 10],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 10);
    assert!(stats.errors.is_empty());
}

#[tokio::test]
async fn write_frame_scales_device_output_brightness() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let brightness_writes = Arc::new(Mutex::new(Vec::new()));
    let mut backend = DirectControlRecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&brightness_writes),
    );
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("recording:strip", "recording", device_id);
    manager.set_device_output_brightness(device_id, 0.5);

    let layout = make_layout(vec![make_zone("zone_0", "recording:strip", 4)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[200, 100, 50]; 4],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 4);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        *writes.lock().await,
        vec![vec![[99, 49, 24]; 4]],
        "software output brightness should scale queued colors"
    );
    assert!(
        brightness_writes.lock().await.is_empty(),
        "per-device output brightness should not issue hardware brightness writes"
    );
}

#[tokio::test]
async fn write_frame_empty_layout_produces_no_writes() {
    let mut manager = BackendManager::new();
    let layout = make_layout(Vec::new());

    let stats = manager.write_frame(&[], &layout).await;
    assert_eq!(stats.devices_written, 0);
    assert_eq!(stats.total_leds, 0);
    assert!(stats.errors.is_empty());
}

#[test]
fn connected_devices_without_layout_targets_reports_unreferenced_mappings() {
    let device_id = DeviceId::new();
    let mut manager = BackendManager::new();
    manager.map_device("usb:1532:0099:001-6-4-4", "usb", device_id);

    let layout = make_layout(Vec::new());

    let inactive = manager.connected_devices_without_layout_targets(&layout);
    assert_eq!(inactive, vec![("usb".to_owned(), device_id)]);
}

#[test]
fn connected_devices_without_layout_targets_treats_any_alias_as_active() {
    let device_id = DeviceId::new();
    let canonical = "usb:1532:0099:001-6-4-4";
    let legacy = format!("device:{device_id}");

    let mut manager = BackendManager::new();
    manager.map_device(canonical, "usb", device_id);
    manager.map_device(legacy.clone(), "usb", device_id);

    let layout = make_layout(vec![make_zone("zone_0", &legacy, 11)]);

    let inactive = manager.connected_devices_without_layout_targets(&layout);
    assert!(inactive.is_empty());
}

#[tokio::test]
async fn write_frame_unmapped_zones_are_silently_skipped() {
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(MockDeviceBackend::new()));

    let layout = make_layout(vec![make_zone("zone_0", "wled:unknown_device", 5)]);

    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 255, 0]; 5],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    // No mapping for "wled:unknown_device" — silently skipped.
    assert_eq!(stats.devices_written, 0);
    assert!(stats.errors.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn write_frame_unmapped_zone_warns_once_until_mapping_changes() {
    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(MockDeviceBackend::new()));

    let layout_device_id = "wled:unknown_device";
    let layout = make_layout(vec![make_zone("zone_0", layout_device_id, 5)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 255, 0]; 5],
    }];
    let warning =
        "zone skipped because the target layout device is not mapped to a connected backend device";

    manager.write_frame(&zone_colors, &layout).await;
    manager.write_frame(&zone_colors, &layout).await;

    assert_eq!(buffer.contents().matches(warning).count(), 1);

    manager.map_device(layout_device_id, "mock", DeviceId::new());
    assert!(manager.unmap_device(layout_device_id));

    manager.write_frame(&zone_colors, &layout).await;

    assert_eq!(buffer.contents().matches(warning).count(), 2);
}

#[tokio::test]
async fn write_frame_missing_backend_reports_error() {
    let device_id = DeviceId::new();
    let mut manager = BackendManager::new();
    // Map a device to a backend that isn't registered.
    manager.map_device("ghost:device_1", "ghost", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "ghost:device_1", 3)]);

    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 0, 255]; 3],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 0);
    assert_eq!(stats.errors.len(), 1);
    assert!(stats.errors[0].contains("ghost"));
}

#[tokio::test]
async fn write_frame_backend_errors_are_not_reported_synchronously() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Failing Strip".into(),
        led_count: 5,
        topology: LedTopology::Strip {
            count: 5,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");
    backend.fail_write = true;

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("mock:failing", "mock", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "mock:failing", 5)]);

    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[128, 128, 128]; 5],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert!(
        stats.errors.is_empty(),
        "queueing should succeed even if async write later fails"
    );

    tokio::time::sleep(Duration::from_millis(40)).await;
    let snapshot = manager.debug_snapshot();
    assert_eq!(snapshot.queue_count, 1);

    let queue = snapshot
        .queues
        .first()
        .expect("expected one queue snapshot");
    assert_eq!(queue.frames_received, 1);
    assert_eq!(queue.frames_sent, 0);
    assert!(
        queue
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("mock write failure"),
        "expected async queue error details for debugging"
    );

    let failures = manager.async_write_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].backend_id, "mock");
    assert_eq!(failures[0].device_id, device_id);
    assert!(failures[0].error.contains("mock write failure"));
}

#[tokio::test]
async fn write_frame_groups_multiple_zones_per_device() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Multi-Zone Device".into(),
        led_count: 8,
        topology: LedTopology::Strip {
            count: 8,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("mock:multi", "mock", device_id);

    // Two zones map to the same device — colors should be concatenated.
    let layout = make_layout(vec![
        make_zone("zone_top", "mock:multi", 4),
        make_zone("zone_bottom", "mock:multi", 4),
    ]);

    let zone_colors = vec![
        ZoneColors {
            zone_id: "zone_top".into(),
            colors: vec![[255, 0, 0]; 4],
        },
        ZoneColors {
            zone_id: "zone_bottom".into(),
            colors: vec![[0, 0, 255]; 4],
        },
    ];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 8); // 4 + 4 grouped into one write.
    assert!(stats.errors.is_empty());
}

#[tokio::test]
async fn write_frame_places_colors_into_configured_segments() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device_with_segment(
        "mock:left-segment",
        "slow",
        device_id,
        Some(SegmentRange::new(0, 2)),
    );
    manager.map_device_with_segment(
        "mock:right-segment",
        "slow",
        device_id,
        Some(SegmentRange::new(4, 2)),
    );

    let layout = make_layout(vec![
        make_zone("zone_left", "mock:left-segment", 2),
        make_zone("zone_right", "mock:right-segment", 2),
    ]);
    let zone_colors = vec![
        ZoneColors {
            zone_id: "zone_left".into(),
            colors: vec![[255, 0, 0], [255, 0, 0]],
        },
        ZoneColors {
            zone_id: "zone_right".into(),
            colors: vec![[0, 0, 255], [0, 0, 255]],
        },
    ];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 6);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(
        frame.as_slice(),
        &[
            [255, 0, 0],
            [255, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 255],
            [0, 0, 255],
        ]
    );
}

#[tokio::test]
async fn write_frame_routes_multi_zone_device_by_zone_name() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device_with_segment(
        "usb:dygma-defy",
        "slow",
        device_id,
        Some(SegmentRange::new(0, 6)),
    );
    assert!(manager.set_device_zone_segments(
        "usb:dygma-defy",
        &DeviceInfo {
            id: device_id,
            name: "Dygma Defy".to_owned(),
            vendor: "Dygma".to_owned(),
            family: DeviceFamily::Dygma,
            model: Some("defy_wired".to_owned()),
            connection_type: ConnectionType::Usb,
            zones: vec![
                ZoneInfo {
                    name: "Left Keys".to_owned(),
                    led_count: 2,
                    topology: DeviceTopologyHint::Custom,
                    color_format: DeviceColorFormat::Rgb,
                },
                ZoneInfo {
                    name: "Right Keys".to_owned(),
                    led_count: 4,
                    topology: DeviceTopologyHint::Custom,
                    color_format: DeviceColorFormat::Rgb,
                },
            ],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 6,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 10,
            },
        }
    ));

    let mut left_zone = make_zone("zone_left_keys", "usb:dygma-defy", 2);
    left_zone.zone_name = Some("Left Keys".to_owned());
    let mut right_zone = make_zone("zone_right_keys", "usb:dygma-defy", 4);
    right_zone.zone_name = Some("Right Keys".to_owned());

    let layout = make_layout(vec![left_zone, right_zone]);
    let zone_colors = vec![
        ZoneColors {
            zone_id: "zone_left_keys".into(),
            colors: vec![[255, 0, 0]; 2],
        },
        ZoneColors {
            zone_id: "zone_right_keys".into(),
            colors: vec![[0, 0, 255]; 4],
        },
    ];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 6);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(
        frame.as_slice(),
        &[
            [255, 0, 0],
            [255, 0, 0],
            [0, 0, 255],
            [0, 0, 255],
            [0, 0, 255],
            [0, 0, 255],
        ]
    );
}

#[tokio::test]
async fn write_frame_pads_single_multi_zone_write_to_full_device_length() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device_with_segment(
        "usb:dygma-defy",
        "slow",
        device_id,
        Some(SegmentRange::new(0, 6)),
    );
    assert!(manager.set_device_zone_segments(
        "usb:dygma-defy",
        &DeviceInfo {
            id: device_id,
            name: "Dygma Defy".to_owned(),
            vendor: "Dygma".to_owned(),
            family: DeviceFamily::Dygma,
            model: Some("defy_wired".to_owned()),
            connection_type: ConnectionType::Usb,
            zones: vec![
                ZoneInfo {
                    name: "Left Keys".to_owned(),
                    led_count: 2,
                    topology: DeviceTopologyHint::Custom,
                    color_format: DeviceColorFormat::Rgb,
                },
                ZoneInfo {
                    name: "Right Keys".to_owned(),
                    led_count: 4,
                    topology: DeviceTopologyHint::Custom,
                    color_format: DeviceColorFormat::Rgb,
                },
            ],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 6,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 10,
            },
        }
    ));

    let mut left_zone = make_zone("zone_left_keys", "usb:dygma-defy", 2);
    left_zone.zone_name = Some("Left Keys".to_owned());

    let layout = make_layout(vec![left_zone]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_left_keys".into(),
        colors: vec![[255, 0, 0]; 2],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 6);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(
        frame.as_slice(),
        &[
            [255, 0, 0],
            [255, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
            [0, 0, 0],
        ]
    );
}

#[tokio::test]
async fn write_frame_applies_zone_led_mapping_before_segment_copy() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device_with_segment(
        "mock:mapped-zone",
        "slow",
        device_id,
        Some(SegmentRange::new(0, 3)),
    );

    let mut zone = make_zone("zone_mapped", "mock:mapped-zone", 3);
    zone.led_mapping = Some(vec![2, 0, 1]);
    let layout = make_layout(vec![zone]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_mapped".into(),
        colors: vec![[10, 0, 0], [20, 0, 0], [30, 0, 0]],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 3);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(frame.as_slice(), &[[20, 0, 0], [30, 0, 0], [10, 0, 0]]);
}

#[tokio::test]
async fn write_frame_uses_attachment_led_range_within_mapped_device() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device_with_segment(
        "usb:prismrgb-prism-s",
        "slow",
        device_id,
        Some(SegmentRange::new(0, 282)),
    );

    let mut zone = make_zone("zone_gpu", "usb:prismrgb-prism-s", 160);
    zone.attachment = Some(ZoneAttachment {
        template_id: "strimer-gpu".into(),
        slot_id: "gpu-strimer".into(),
        instance: 0,
        led_start: Some(120),
        led_count: Some(160),
        led_mapping: None,
    });
    let layout = make_layout(vec![zone]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_gpu".into(),
        colors: vec![[0, 0, 255]; 160],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 280);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(frame.len(), 280);
    assert!(frame[..120].iter().all(|color| *color == [0, 0, 0]));
    assert!(frame[120..280].iter().all(|color| *color == [0, 0, 255]));
}

#[tokio::test]
async fn write_frame_uses_sampled_led_count_when_attachment_metadata_is_stale() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));
    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(10),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("usb:corsair-aio", "slow", device_id);

    let mut zone = make_zone("zone_aio", "usb:corsair-aio", 24);
    zone.attachment = Some(ZoneAttachment {
        template_id: "stale-aio-template".into(),
        slot_id: "pump".into(),
        instance: 0,
        led_start: Some(0),
        led_count: Some(44),
        led_mapping: None,
    });
    let layout = make_layout(vec![zone]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_aio".into(),
        colors: vec![[12, 34, 56]; 24],
    }];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 24);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(80)).await;
    let writes = writes.lock().await;
    let frame = writes.first().expect("one frame should be written");
    assert_eq!(frame.len(), 24);
    assert!(frame.iter().all(|color| *color == [12, 34, 56]));
}

#[tokio::test]
async fn write_frame_unknown_zone_id_warns_but_continues() {
    let device_id = DeviceId::new();
    let mock_config = MockDeviceConfig {
        name: "Strip".into(),
        led_count: 5,
        topology: LedTopology::Strip {
            count: 5,
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        id: Some(device_id),
    };

    let mut backend = MockDeviceBackend::new().with_device(&mock_config);
    backend.connect(&device_id).await.expect("connect");

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("mock:strip", "mock", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "mock:strip", 5)]);

    // Zone colors include a zone_id that doesn't exist in the layout.
    let zone_colors = vec![
        ZoneColors {
            zone_id: "zone_0".into(),
            colors: vec![[255, 255, 0]; 5],
        },
        ZoneColors {
            zone_id: "nonexistent_zone".into(),
            colors: vec![[0, 0, 0]; 3],
        },
    ];

    let stats = manager.write_frame(&zone_colors, &layout).await;
    // Only zone_0 is written; nonexistent_zone is skipped.
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 5);
    assert!(stats.errors.is_empty());
}

#[tokio::test]
async fn write_frame_returns_immediately_with_slow_backend() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(160),
        writes.clone(),
        write_count.clone(),
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("slow:strip", "slow", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "slow:strip", 10)]);
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[10, 20, 30]; 10],
    }];

    let started = Instant::now();
    let stats = manager.write_frame(&zone_colors, &layout).await;
    let elapsed = started.elapsed();

    assert_eq!(stats.devices_written, 1);
    assert!(
        elapsed < Duration::from_millis(110),
        "write_frame should enqueue quickly, elapsed={elapsed:?}"
    );
    assert_eq!(
        write_count.load(Ordering::Relaxed),
        0,
        "async writer should still be running"
    );

    tokio::time::sleep(Duration::from_millis(260)).await;
    assert_eq!(write_count.load(Ordering::Relaxed), 1);
    assert_eq!(writes.lock().await.len(), 1);

    let snapshot = manager.debug_snapshot();
    let queue = snapshot
        .queues
        .first()
        .expect("expected one queue snapshot");
    assert_eq!(queue.frames_received, 1);
    assert_eq!(queue.frames_sent, 1);
    assert_eq!(queue.frames_dropped, 0);
    assert!(queue.avg_latency_ms > 0);
}

#[tokio::test]
async fn write_frame_drops_stale_intermediate_payloads() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::from_millis(140),
        writes.clone(),
        write_count,
    );

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager.map_device("slow:strip", "slow", device_id);

    let layout = make_layout(vec![make_zone("zone_0", "slow:strip", 4)]);

    let first = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[255, 0, 0]; 4],
    }];
    let second = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 255, 0]; 4],
    }];
    let third = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 0, 255]; 4],
    }];

    manager.write_frame(&first, &layout).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    manager.write_frame(&second, &layout).await;
    manager.write_frame(&third, &layout).await;

    tokio::time::sleep(Duration::from_millis(420)).await;

    let writes = writes.lock().await.clone();
    assert!(
        !writes.is_empty(),
        "slow backend should receive at least one payload"
    );
    assert!(
        writes.len() <= 2,
        "stale intermediate payloads should be dropped"
    );
    let last_frame = writes.last().expect("expected at least one write");
    assert_eq!(
        last_frame[0],
        [0, 0, 255],
        "latest payload should win after overlap"
    );
    assert!(
        !writes.iter().any(|frame| frame[0] == [0, 255, 0]),
        "intermediate frame should have been dropped"
    );

    let snapshot = manager.debug_snapshot();
    assert_eq!(snapshot.queue_count, 1);
    let queue = snapshot
        .queues
        .first()
        .expect("expected one queue snapshot");
    assert_eq!(queue.frames_received, 3);
    assert!(
        queue.frames_dropped >= 1,
        "debug snapshot should track dropped stale frames"
    );
    assert_eq!(queue.mapped_layout_ids, vec!["slow:strip".to_string()]);
}

#[tokio::test]
async fn write_frame_uses_interval_pacing_for_cached_target_fps() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));
    let write_times = Arc::new(Mutex::new(Vec::<Instant>::new()));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::ZERO,
        Arc::clone(&writes),
        Arc::clone(&write_count),
    )
    .with_target_fps(10)
    .with_write_times(Arc::clone(&write_times));

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager
        .connect_device("slow", device_id, "slow:paced")
        .await
        .expect("connect should succeed");

    let layout = make_layout(vec![make_zone("zone_0", "slow:paced", 4)]);
    let first = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[255, 0, 0]; 4],
    }];
    let second = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 0, 255]; 4],
    }];

    manager.write_frame(&first, &layout).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    manager.write_frame(&second, &layout).await;

    tokio::time::sleep(Duration::from_millis(220)).await;

    let write_times = write_times.lock().await.clone();
    assert!(
        write_times.len() >= 2,
        "expected paced backend to receive two writes"
    );
    let delta = write_times[1].saturating_duration_since(write_times[0]);
    assert!(
        delta >= Duration::from_millis(70),
        "expected interval pacing between writes, delta={delta:?}"
    );

    let snapshot = manager.debug_snapshot();
    let queue = snapshot
        .queues
        .first()
        .expect("expected one queue snapshot");
    assert_eq!(queue.target_fps, 10);
    assert_eq!(queue.frames_sent, 2);
}

#[tokio::test]
async fn write_frame_sends_latest_pending_payload_at_paced_deadline() {
    let device_id = DeviceId::new();
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let write_count = Arc::new(AtomicUsize::new(0));

    let backend = SlowRecordingBackend::new(
        device_id,
        Duration::ZERO,
        Arc::clone(&writes),
        Arc::clone(&write_count),
    )
    .with_target_fps(10);

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    manager
        .connect_device("slow", device_id, "slow:latest")
        .await
        .expect("connect should succeed");

    let layout = make_layout(vec![make_zone("zone_0", "slow:latest", 4)]);
    let red = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[255, 0, 0]; 4],
    }];
    let green = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 255, 0]; 4],
    }];
    let blue = vec![ZoneColors {
        zone_id: "zone_0".into(),
        colors: vec![[0, 0, 255]; 4],
    }];

    manager.write_frame(&red, &layout).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while write_count.load(Ordering::Relaxed) < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first paced write should complete");

    manager.write_frame(&green, &layout).await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    manager.write_frame(&blue, &layout).await;

    tokio::time::sleep(Duration::from_millis(160)).await;

    let writes = writes.lock().await.clone();
    assert!(
        writes.len() >= 2,
        "expected paced queue to deliver the initial frame and one follow-up write"
    );
    assert_eq!(writes[0][0], [255, 0, 0]);
    assert_eq!(
        writes[1][0],
        [0, 0, 255],
        "paced send should use the freshest pending payload at the send deadline"
    );
    assert!(
        !writes[1..].iter().any(|frame| frame[0] == [0, 255, 0]),
        "older pending payloads should be superseded before the paced write fires"
    );
}
