use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_driver_api::DriverTrackedDevice;
use hypercolor_driver_nanoleaf::resolve_nanoleaf_probe_devices_from_sources;
use hypercolor_types::config::NanoleafConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};

fn tracked_nanoleaf_device() -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Shapes".to_owned(),
            vendor: "Nanoleaf".to_owned(),
            family: DeviceFamily::Nanoleaf,
            model: Some("NL42".to_owned()),
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Panels".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Point,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: Some("9.4.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 0,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 30,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), "10.0.0.30".to_owned()),
            ("api_port".to_owned(), "16021".to_owned()),
            ("device_key".to_owned(), "nanoleaf-shapes".to_owned()),
        ]),
        fingerprint: None,
        current_state: DeviceState::Known,
    }
}

#[test]
fn resolve_nanoleaf_probe_devices_merges_tracked_metadata() {
    let config = NanoleafConfig {
        device_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 25))],
        ..NanoleafConfig::default()
    };

    let resolved =
        resolve_nanoleaf_probe_devices_from_sources(&config, &[tracked_nanoleaf_device()]);

    assert_eq!(resolved.len(), 2);
    let tracked = resolved
        .iter()
        .find(|device| device.ip == IpAddr::V4(Ipv4Addr::new(10, 0, 0, 30)))
        .expect("tracked device should be present");
    assert_eq!(tracked.port, 16021);
    assert_eq!(tracked.device_id, "nanoleaf-shapes");
    assert_eq!(tracked.name, "Shapes");
    assert_eq!(tracked.model, "NL42");
}
