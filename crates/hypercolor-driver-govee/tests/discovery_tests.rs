use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_driver_api::DriverTrackedDevice;
use hypercolor_driver_govee::{
    build_device_info, parse_scan_response, resolve_govee_probe_devices_from_sources,
};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};

#[test]
fn parses_lan_scan_response_into_stable_govee_identity() {
    let response = br#"{
        "msg": {
            "cmd": "scan",
            "data": {
                "ip": "192.168.1.44",
                "device": "AA:BB:CC:DD:EE:FF",
                "sku": "H6163",
                "wifiVersionSoft": "1.02.03"
            }
        }
    }"#;

    let device = parse_scan_response(response, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)))
        .expect("scan response should parse");

    assert_eq!(device.ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 44)));
    assert_eq!(device.sku, "H6163");
    assert_eq!(device.mac, "aabbccddeeff");
    assert_eq!(device.firmware_version.as_deref(), Some("1.02.03"));
}

#[test]
fn build_device_info_uses_validated_razer_count_only() {
    let device = parse_scan_response(
        br#"{"msg":{"data":{"ip":"10.0.0.8","device":"001122334455","sku":"H619A"}}}"#,
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
    )
    .expect("scan response should parse");

    let info = build_device_info(&device);

    assert_eq!(info.family, DeviceFamily::Govee);
    assert_eq!(info.model.as_deref(), Some("H619A"));
    assert_eq!(info.total_led_count(), 20);
    assert_eq!(info.capabilities.max_fps, 25);
}

#[test]
fn resolve_probe_devices_merges_config_and_tracked_metadata() {
    let config = GoveeConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..GoveeConfig::default()
    };
    let tracked = vec![tracked_govee_device("10.0.0.5", "H6163", "aabbccddeeff")];

    let resolved = resolve_govee_probe_devices_from_sources(&config, &tracked);

    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    assert_eq!(resolved[1].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)));
    assert_eq!(resolved[1].sku.as_deref(), Some("H6163"));
    assert_eq!(resolved[1].mac.as_deref(), Some("aabbccddeeff"));
}

fn tracked_govee_device(ip: &str, sku: &str, mac: &str) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Desk Govee".to_owned(),
            vendor: "Govee".to_owned(),
            family: DeviceFamily::Govee,
            model: Some(sku.to_owned()),
            connection_type: ConnectionType::Network,
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 1,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 1,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 10,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), ip.to_owned()),
            ("sku".to_owned(), sku.to_owned()),
            ("mac".to_owned(), mac.to_owned()),
        ]),
        fingerprint: Some(DeviceFingerprint(format!("net:govee:{mac}"))),
        current_state: DeviceState::Known,
    }
}
