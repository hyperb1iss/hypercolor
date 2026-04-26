use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_driver_api::DriverTrackedDevice;
use hypercolor_driver_hue::{HueConfig, resolve_hue_probe_bridges_from_sources};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint, ZoneInfo,
};

fn tracked_hue_device() -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Studio Bridge".to_owned(),
            vendor: "Philips Hue".to_owned(),
            family: DeviceFamily::Hue,
            model: Some("BSB002".to_owned()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("hue", "hue", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Bridge".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("1969152010".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 0,
                supports_direct: false,
                supports_brightness: false,
                has_display: false,
                display_resolution: None,
                max_fps: 0,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), "10.0.0.20".to_owned()),
            ("api_port".to_owned(), "8443".to_owned()),
            ("bridge_id".to_owned(), "bridge-123".to_owned()),
        ]),
        fingerprint: None,
        current_state: DeviceState::Known,
    }
}

#[test]
fn resolve_hue_probe_bridges_merges_tracked_metadata() {
    let config = HueConfig {
        bridge_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10))],
        ..HueConfig::default()
    };

    let resolved = resolve_hue_probe_bridges_from_sources(&config, &[tracked_hue_device()]);

    assert_eq!(resolved.len(), 2);
    let tracked = resolved
        .iter()
        .find(|bridge| bridge.ip == IpAddr::V4(Ipv4Addr::new(10, 0, 0, 20)))
        .expect("tracked bridge should be present");
    assert_eq!(tracked.api_port, 8443);
    assert_eq!(tracked.bridge_id, "bridge-123");
    assert_eq!(tracked.name, "Studio Bridge");
    assert_eq!(tracked.model_id, "BSB002");
}
