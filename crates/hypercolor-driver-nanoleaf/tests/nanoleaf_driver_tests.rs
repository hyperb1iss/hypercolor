use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    DriverConfigProvider, DriverTrackedDevice, NetworkDriverFactory, TrackedDeviceCtx,
};
use hypercolor_driver_nanoleaf::{
    NanoleafConfig, NanoleafDriverFactory, nanoleaf_device_control_surface,
    nanoleaf_driver_control_surface, resolve_nanoleaf_probe_devices_from_sources,
};
use hypercolor_types::controls::{ApplyImpact, ControlAccess, ControlValue};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint, ZoneInfo,
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
            origin: DeviceOrigin::native("nanoleaf", "nanoleaf", ConnectionType::Network),
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

#[test]
fn nanoleaf_config_validation_rejects_non_routable_device_ips() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let factory = NanoleafDriverFactory::new(
        Arc::new(
            CredentialStore::open_blocking(tempdir.path()).expect("credential store should open"),
        ),
        false,
    );
    let mut config = factory
        .config()
        .expect("Nanoleaf should expose config provider")
        .default_config();
    config
        .settings
        .insert("device_ips".to_owned(), serde_json::json!(["127.0.0.1"]));

    let error = factory
        .validate_config(&config)
        .expect_err("loopback device IP should be rejected");
    assert!(error.to_string().contains("invalid Nanoleaf device IP"));
}

#[test]
fn nanoleaf_driver_control_surface_exposes_typed_config_fields() {
    let config = NanoleafConfig {
        device_ips: vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 25))],
        transition_time: 4,
    };

    let surface = nanoleaf_driver_control_surface(&config);

    assert_eq!(surface.surface_id, "driver:nanoleaf");
    let ControlValue::List(device_ips) = &surface.values["device_ips"] else {
        panic!("device IPs should be a list");
    };
    assert_eq!(
        device_ips,
        &[ControlValue::IpAddress("10.0.0.25".to_owned())]
    );
    assert_eq!(surface.values["transition_time"], ControlValue::Integer(4));
    assert!(surface.fields.iter().any(
        |field| field.id == "device_ips" && field.apply_impact == ApplyImpact::DiscoveryRescan
    ));
    assert!(
        surface
            .fields
            .iter()
            .any(|field| field.id == "transition_time"
                && field.apply_impact == ApplyImpact::BackendRebind)
    );
}

#[test]
fn nanoleaf_device_control_surface_exposes_tracked_metadata() {
    let tracked = tracked_nanoleaf_device();
    let device = TrackedDeviceCtx {
        device_id: tracked.info.id,
        info: &tracked.info,
        metadata: Some(&tracked.metadata),
        current_state: &tracked.current_state,
    };

    let surface = nanoleaf_device_control_surface(&device);

    assert_eq!(
        surface.surface_id,
        format!("driver:nanoleaf:device:{}", tracked.info.id)
    );
    assert_eq!(
        surface.scope,
        hypercolor_types::controls::ControlSurfaceScope::Device {
            device_id: tracked.info.id,
            driver_id: "nanoleaf".to_owned(),
        }
    );
    assert!(surface.revision > 0);
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "ip" && field.access == ControlAccess::ReadOnly })
    );
    assert!(
        surface
            .fields
            .iter()
            .any(|field| { field.id == "api_port" && field.access == ControlAccess::ReadOnly })
    );
    assert_eq!(
        surface.values["ip"],
        ControlValue::IpAddress("10.0.0.30".to_owned())
    );
    assert_eq!(surface.values["api_port"], ControlValue::Integer(16021));
    assert_eq!(
        surface.values["device_key"],
        ControlValue::String("nanoleaf-shapes".to_owned())
    );
    assert_eq!(
        surface.values["model"],
        ControlValue::String("NL42".to_owned())
    );
    assert_eq!(
        surface.values["firmware_version"],
        ControlValue::String("9.4.0".to_owned())
    );
    assert_eq!(surface.values["led_count"], ControlValue::Integer(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Integer(30));
    assert_eq!(
        surface.values["state"],
        ControlValue::String("Known".to_owned())
    );
}
