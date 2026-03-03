//! Tests for the `BackendManager` — device routing and frame dispatch.

use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig};
use hypercolor_core::device::{BackendManager, DeviceBackend};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};

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

// ── Device Mapping Tests ────────────────────────────────────────────────────

#[test]
fn map_and_unmap_device() {
    let mut manager = BackendManager::new();
    let device_id = DeviceId::new();

    manager.map_device("wled:strip_1", "wled", device_id);
    assert_eq!(manager.mapped_device_count(), 1);

    assert!(manager.unmap_device("wled:strip_1"));
    assert_eq!(manager.mapped_device_count(), 0);

    // Second unmap returns false.
    assert!(!manager.unmap_device("wled:strip_1"));
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
async fn write_frame_backend_error_is_collected() {
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
    assert_eq!(stats.devices_written, 0);
    assert_eq!(stats.errors.len(), 1);
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
