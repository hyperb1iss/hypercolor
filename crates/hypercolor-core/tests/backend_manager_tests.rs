//! Tests for the `BackendManager` — device routing and frame dispatch.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};
use tokio::sync::Mutex;

// ── Slow Test Backend ────────────────────────────────────────────────────────

struct SlowRecordingBackend {
    expected_device_id: DeviceId,
    delay: Duration,
    writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
    write_count: Arc<AtomicUsize>,
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
        }
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
        self.writes.lock().await.push(colors.to_vec());
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test-layout".into(),
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

fn make_zone(id: &str, device_id: &str, led_count: u32) -> DeviceZone {
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
            direction: hypercolor_types::spatial::StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
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
async fn write_frame_empty_layout_produces_no_writes() {
    let mut manager = BackendManager::new();
    let layout = make_layout(Vec::new());

    let stats = manager.write_frame(&[], &layout).await;
    assert_eq!(stats.devices_written, 0);
    assert_eq!(stats.total_leds, 0);
    assert!(stats.errors.is_empty());
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
