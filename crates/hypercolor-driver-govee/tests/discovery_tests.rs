use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_driver_api::{DriverTrackedDevice, TrackedDeviceCtx};
use hypercolor_driver_govee::cloud::V1Device;
use hypercolor_driver_govee::{
    GoveeKnownDevice, build_cloud_discovered_device, build_device_info,
    govee_device_control_surface, merge_cloud_inventory, parse_scan_response,
    resolve_govee_probe_devices, resolve_govee_probe_devices_from_sources,
};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::controls::{ControlAccess, ControlSurfaceScope, ControlValue};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    ZoneInfo,
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

#[test]
fn resolve_probe_devices_merges_cached_runtime_hints() {
    let config = GoveeConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..GoveeConfig::default()
    };
    let cached = vec![GoveeKnownDevice {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        sku: Some("H619A".to_owned()),
        mac: Some("001122334455".to_owned()),
    }];

    let resolved = resolve_govee_probe_devices(&config, &[], &cached);

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    assert_eq!(resolved[0].sku.as_deref(), Some("H619A"));
    assert_eq!(resolved[0].mac.as_deref(), Some("001122334455"));
}

#[test]
fn cloud_inventory_device_uses_mac_fingerprint_when_device_id_is_mac() {
    let discovered = build_cloud_discovered_device(V1Device {
        device: "AA:BB:CC:DD:EE:FF".to_owned(),
        model: "H6163".to_owned(),
        device_name: "Desk Strip".to_owned(),
        controllable: true,
        retrievable: true,
        support_cmds: vec!["turn".to_owned(), "brightness".to_owned()],
        properties: None,
    });

    assert_eq!(discovered.fingerprint.0, "net:govee:aabbccddeeff");
    assert_eq!(discovered.info.name, "Desk Strip");
    assert_eq!(
        discovered.metadata.get("mac"),
        Some(&"aabbccddeeff".to_owned())
    );
    assert_eq!(
        discovered.metadata.get("cloud_device_id"),
        Some(&"AA:BB:CC:DD:EE:FF".to_owned())
    );
    assert!(!discovered.connect_behavior.should_auto_connect());
}

#[test]
fn cloud_inventory_merges_with_lan_device_without_overriding_lan_metadata() {
    let lan_device = parse_scan_response(
        br#"{"msg":{"data":{"ip":"10.0.0.8","device":"001122334455","sku":"H619A"}}}"#,
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
    )
    .expect("scan response should parse");
    let mut devices = vec![hypercolor_driver_api::DriverDiscoveredDevice::from(
        hypercolor_driver_api::DiscoveredDevice {
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("govee", "govee", ConnectionType::Network),
            name: build_device_info(&lan_device).name,
            family: DeviceFamily::Govee,
            fingerprint: DeviceFingerprint("net:govee:001122334455".to_owned()),
            connect_behavior: hypercolor_driver_api::DiscoveryConnectBehavior::AutoConnect,
            info: build_device_info(&lan_device),
            metadata: HashMap::from([
                ("backend_id".to_owned(), "govee".to_owned()),
                ("ip".to_owned(), "10.0.0.8".to_owned()),
                ("sku".to_owned(), "H619A".to_owned()),
                ("mac".to_owned(), "001122334455".to_owned()),
            ]),
        },
    )];

    merge_cloud_inventory(
        &mut devices,
        vec![V1Device {
            device: "00:11:22:33:44:55".to_owned(),
            model: "H619A".to_owned(),
            device_name: "Cloud Name".to_owned(),
            controllable: true,
            retrievable: true,
            support_cmds: vec!["color".to_owned()],
            properties: None,
        }],
    );

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].metadata.get("ip"), Some(&"10.0.0.8".to_owned()));
    assert_eq!(devices[0].info.name, "RGBIC Pro Strip H619A");
    assert_eq!(
        devices[0].metadata.get("cloud_device_id"),
        Some(&"00:11:22:33:44:55".to_owned())
    );
    assert_eq!(
        devices[0].metadata.get("cloud_support_cmds"),
        Some(&"color".to_owned())
    );
    assert!(devices[0].connect_behavior.should_auto_connect());
}

#[test]
fn govee_device_control_surface_exposes_lan_metadata() {
    let tracked = tracked_govee_device("10.0.0.5", "H619A", "001122334455");
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let surface = govee_device_control_surface(&device);

    assert_eq!(
        surface.surface_id,
        format!("driver:govee:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "govee".to_owned(),
        }
    );
    assert!(surface.revision > 0);
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "ip" && field.access == ControlAccess::ReadOnly)
    );
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "razer_streaming" && field.access == ControlAccess::ReadOnly)
    );
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.5".to_owned())
    );
    assert_eq!(
        surface.values["sku"],
        ControlValue::String("H619A".to_owned())
    );
    assert_eq!(
        surface.values["mac"],
        ControlValue::MacAddress("001122334455".to_owned())
    );
    assert_eq!(surface.values["razer_streaming"], ControlValue::Bool(true));
    assert_eq!(surface.values["led_count"], ControlValue::Integer(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(10));
}

#[test]
fn govee_device_control_surface_exposes_cloud_metadata() {
    let discovered = build_cloud_discovered_device(V1Device {
        device: "AA:BB:CC:DD:EE:FF".to_owned(),
        model: "H6163".to_owned(),
        device_name: "Cloud Strip".to_owned(),
        controllable: true,
        retrievable: false,
        support_cmds: vec!["turn".to_owned(), "brightness".to_owned()],
        properties: None,
    });
    let device = TrackedDeviceCtx {
        device_id: discovered.info.id,
        info: &discovered.info,
        metadata: Some(&discovered.metadata),
        current_state: &DeviceState::Known,
    };

    let surface = govee_device_control_surface(&device);

    assert_eq!(
        surface.values["cloud_device_id"],
        ControlValue::String("AA:BB:CC:DD:EE:FF".to_owned())
    );
    assert_eq!(
        surface.values["cloud_controllable"],
        ControlValue::Bool(true)
    );
    assert_eq!(
        surface.values["cloud_retrievable"],
        ControlValue::Bool(false)
    );
    assert_eq!(
        surface.values["cloud_support_cmds"],
        ControlValue::List(vec![
            ControlValue::String("turn".to_owned()),
            ControlValue::String("brightness".to_owned()),
        ])
    );
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
            origin: DeviceOrigin::native("govee", "govee", ConnectionType::Network),
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
