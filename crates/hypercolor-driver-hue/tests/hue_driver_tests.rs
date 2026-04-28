use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    DriverConfigProvider, DriverModule, DriverTrackedDevice, TrackedDeviceCtx,
};
use hypercolor_driver_hue::{
    HueConfig, HueDriverModule, hue_device_control_surface, hue_driver_control_surface,
    resolve_hue_probe_bridges_from_sources,
};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlAvailabilityState, ControlSurfaceScope, ControlValue,
};
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
            family: DeviceFamily::new_static("hue", "Philips Hue"),
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
            ("bridge_name".to_owned(), "Studio Bridge".to_owned()),
            ("entertainment_config_id".to_owned(), "config-1".to_owned()),
            (
                "entertainment_config_name".to_owned(),
                "Studio Area".to_owned(),
            ),
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

#[test]
fn hue_config_validation_rejects_non_routable_bridge_ips() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let module = HueDriverModule::new(
        Arc::new(
            CredentialStore::open_blocking(tempdir.path()).expect("credential store should open"),
        ),
        false,
    );
    let mut config = module
        .config()
        .expect("Hue should expose config provider")
        .default_config();
    config
        .settings
        .insert("bridge_ips".to_owned(), serde_json::json!(["127.0.0.1"]));

    let error = module
        .validate_config(&config)
        .expect_err("loopback bridge IP should be rejected");
    assert!(error.to_string().contains("invalid Hue bridge IP"));
}

#[test]
fn hue_driver_control_surface_exposes_typed_config_fields() {
    let config = HueConfig {
        bridge_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10))],
        use_cie_xy: false,
        ..HueConfig::default()
    };

    let surface = hue_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:hue");
    let ControlValue::List(bridge_ips) = &surface.values["bridge_ips"] else {
        panic!("bridge IPs should be a list");
    };
    assert_eq!(
        bridge_ips,
        &[ControlValue::IpAddress("10.0.0.10".to_owned())]
    );
    assert_eq!(surface.values["use_cie_xy"], ControlValue::Bool(false));
    assert!(surface.fields.iter().any(
        |field| field.id == "bridge_ips" && field.apply_impact == ApplyImpact::DiscoveryRescan
    ));
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "use_cie_xy"
                && field.apply_impact == ApplyImpact::BackendRebind)
    );
}

#[test]
fn hue_device_control_surface_exposes_tracked_metadata() {
    let tracked = tracked_hue_device();
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let surface = hue_device_control_surface(&device);

    assert_eq!(
        surface.surface_id,
        format!("driver:hue:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "hue".to_owned(),
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
            .any(|field| field.id == "entertainment_config_name"
                && field.access == ControlAccess::ReadOnly)
    );
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.20".to_owned())
    );
    assert_eq!(surface.values["api_port"], ControlValue::Integer(8443));
    assert_eq!(
        surface.values["entertainment_config_name"],
        ControlValue::String("Studio Area".to_owned())
    );
    assert_eq!(surface.values["led_count"], ControlValue::Integer(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(0));
    assert_eq!(
        surface.values["state"],
        ControlValue::String("Known".to_owned())
    );
    assert_eq!(
        surface.availability["ip"].state,
        ControlAvailabilityState::Available
    );
}
