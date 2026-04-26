use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_core::device::wled::WledKnownTarget;
use hypercolor_driver_api::{DriverTrackedDevice, NetworkDriverFactory};
use hypercolor_driver_wled::{
    WledConfig, WledDriverFactory, WledProtocolConfig, resolve_wled_probe_ips_from_sources,
    resolve_wled_probe_targets_from_sources, wled_driver_control_surface,
};
use hypercolor_types::controls::{ApplyImpact, ControlValue};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    ZoneInfo,
};

fn tracked_wled_device(ip: &str, hostname: &str, name: &str) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: name.to_owned(),
            vendor: "WLED".to_owned(),
            family: DeviceFamily::Wled,
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            zones: vec![ZoneInfo {
                name: "Main".to_owned(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgbw,
            }],
            firmware_version: Some("0.15.0".to_owned()),
            capabilities: DeviceCapabilities {
                led_count: 60,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 55,
                color_space: hypercolor_types::device::DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        },
        metadata: HashMap::from([
            ("ip".to_owned(), ip.to_owned()),
            ("hostname".to_owned(), hostname.to_owned()),
        ]),
        fingerprint: Some(DeviceFingerprint(format!("net:{hostname}"))),
        current_state: DeviceState::Known,
    }
}

#[test]
fn resolve_probe_ips_merges_all_sources() {
    let config = WledConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        ..WledConfig::default()
    };
    let tracked = vec![tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip")];
    let cached_probe_ips = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3))];
    let cached_targets = vec![WledKnownTarget::from_ip(IpAddr::V4(Ipv4Addr::new(
        10, 0, 0, 4,
    )))];

    let resolved =
        resolve_wled_probe_ips_from_sources(&config, &tracked, &cached_probe_ips, &cached_targets);

    assert_eq!(
        resolved,
        vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
        ]
    );
}

#[test]
fn resolve_probe_targets_prefers_tracked_metadata() {
    let tracked = vec![tracked_wled_device("10.0.0.5", "desk.local", "Desk Strip")];
    let resolved = resolve_wled_probe_targets_from_sources(
        &WledConfig::default(),
        &tracked,
        &[],
        &[WledKnownTarget {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            hostname: None,
            fingerprint: None,
            name: None,
            led_count: None,
            firmware_version: None,
            max_fps: None,
            rgbw: None,
        }],
    );

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name.as_deref(), Some("Desk Strip"));
    assert_eq!(resolved[0].hostname.as_deref(), Some("desk.local"));
    assert_eq!(resolved[0].led_count, Some(60));
    assert_eq!(resolved[0].rgbw, Some(true));
}

#[test]
fn wled_factory_advertises_control_surface_capability() {
    let descriptor = WledDriverFactory::new(false).module_descriptor();

    assert!(descriptor.capabilities.controls);
    assert!(descriptor.capabilities.discovery);
    assert!(descriptor.capabilities.backend_factory);
}

#[test]
fn wled_driver_control_surface_exposes_typed_config_fields() {
    let config = WledConfig {
        known_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))],
        default_protocol: WledProtocolConfig::E131,
        realtime_http_enabled: false,
        dedup_threshold: 7,
    };

    let surface = wled_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:wled");
    assert_eq!(surface.fields.len(), 4);
    assert!(surface.fields.iter().any(|field| {
        field.id == "known_ips" && field.apply_impact == ApplyImpact::DiscoveryRescan
    }));
    assert!(surface.fields.iter().any(|field| {
        field.id == "default_protocol" && field.apply_impact == ApplyImpact::BackendRebind
    }));
    assert_eq!(
        surface.values["known_ips"],
        ControlValue::List(vec![ControlValue::IpAddress("10.0.0.2".to_owned())])
    );
    assert_eq!(
        surface.values["default_protocol"],
        ControlValue::Enum("e131".to_owned())
    );
    assert_eq!(
        surface.values["realtime_http_enabled"],
        ControlValue::Bool(false)
    );
    assert_eq!(surface.values["dedup_threshold"], ControlValue::Integer(7));
}
