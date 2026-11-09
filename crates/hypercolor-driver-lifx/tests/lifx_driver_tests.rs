//! Driver module surface: descriptor, config, controls, probe targets
//! (Spec 88 §11).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_api::{
    DriverConfigView, DriverModule, DriverTrackedDevice, TrackedDeviceCtx,
};
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::products::lookup_product;
use hypercolor_driver_lifx::protocol::{FirmwareVersion, Serial};
use hypercolor_driver_lifx::topology::{LifxDeviceProfile, LifxTopology, fingerprint_for_serial};
use hypercolor_driver_lifx::{
    DESCRIPTOR, LifxDriverModule, TrackedRoute, lifx_device_control_surface,
    lifx_driver_control_surface, resolve_lifx_probe_targets, tracked_routes,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceClassHint, DeviceState, DriverTransportKind};

fn entry(settings: serde_json::Value) -> DriverConfigEntry {
    let settings: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(settings).expect("settings map");
    DriverConfigEntry::enabled(settings)
}

#[test]
fn descriptor_is_an_enabled_network_driver_without_pairing() {
    let module = LifxDriverModule::new();
    assert_eq!(DESCRIPTOR.id, "lifx");
    assert_eq!(DESCRIPTOR.transport, DriverTransportKind::Network);
    let descriptor = module.module_descriptor();
    assert!(descriptor.default_enabled);
    assert!(descriptor.capabilities.discovery);
    assert!(descriptor.capabilities.config);
    assert!(descriptor.capabilities.controls);
    assert!(descriptor.capabilities.presentation);
    assert!(descriptor.capabilities.output_backend);
    assert!(!descriptor.capabilities.pairing);
    assert!(!descriptor.capabilities.credentials);
    assert!(module.output().is_output());
    let presentation = module
        .presentation()
        .expect("presentation provider")
        .presentation();
    assert_eq!(presentation.label, "LIFX");
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Light)
    );
}

#[test]
fn default_config_is_enabled_and_valid() {
    let module = LifxDriverModule::new();
    let config = module.config().expect("config provider");
    let default = config.default_config();
    assert!(default.enabled);
    config
        .validate_config(&default)
        .expect("default config validates");
    let parsed = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &default,
    }
    .parse_settings::<LifxConfig>()
    .expect("default config parses");
    assert_eq!(parsed, LifxConfig::default());
    assert!(parsed.broadcast_discovery);
    assert_eq!(parsed.max_messages_per_second, 20);
    assert_eq!(parsed.transition_ms, 0);
    assert_eq!(parsed.white_kelvin, 6_500);
}

#[test]
fn empty_settings_fall_back_to_defaults() {
    let parsed = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry(serde_json::json!({})),
    }
    .parse_settings::<LifxConfig>()
    .expect("empty settings parse");
    assert_eq!(parsed, LifxConfig::default());
}

#[test]
fn invalid_configs_are_rejected() {
    let module = LifxDriverModule::new();
    let config = module.config().expect("config provider");
    for settings in [
        serde_json::json!({ "max_messages_per_second": 0 }),
        serde_json::json!({ "max_messages_per_second": 201 }),
        serde_json::json!({ "transition_ms": 10_001 }),
        serde_json::json!({ "white_kelvin": 1_000 }),
        serde_json::json!({ "white_kelvin": 9_500 }),
        serde_json::json!({ "device_ips": ["127.0.0.1"] }),
        serde_json::json!({ "device_ips": ["255.255.255.255"] }),
    ] {
        assert!(
            config.validate_config(&entry(settings.clone())).is_err(),
            "{settings} should be rejected"
        );
    }
    config
        .validate_config(&entry(serde_json::json!({
            "device_ips": ["192.168.1.40"],
            "broadcast_discovery": false,
            "max_messages_per_second": 60,
            "transition_ms": 100,
            "white_kelvin": 2_700,
        })))
        .expect("a full config validates");
}

#[test]
fn driver_surface_exposes_every_config_field() {
    let config = LifxConfig {
        device_ips: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))],
        broadcast_discovery: false,
        max_messages_per_second: 30,
        transition_ms: 50,
        white_kelvin: 4_000,
    };
    let surface = lifx_driver_control_surface(&config);
    assert_eq!(surface.surface_id, "driver:lifx");
    let ids: Vec<&str> = surface
        .fields
        .iter()
        .map(|field| field.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "device_ips",
            "broadcast_discovery",
            "max_messages_per_second",
            "transition_ms",
            "white_kelvin",
        ]
    );
    assert_eq!(
        surface.values["broadcast_discovery"],
        ControlValue::Bool(false)
    );
    assert_eq!(
        surface.values["max_messages_per_second"],
        ControlValue::Int(30)
    );
    assert_eq!(surface.values["white_kelvin"], ControlValue::Int(4_000));
    assert!(matches!(&surface.values["device_ips"], ControlValue::List(items) if items.len() == 1));
}

fn tracked(ip: &str, port: &str) -> DriverTrackedDevice {
    let profile = LifxDeviceProfile {
        serial: Serial([0xd0, 0x73, 0xd5, 1, 2, 3]),
        addr: SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 56_700)),
        label: "Lamp".to_owned(),
        vendor_id: 1,
        product_id: 27,
        product: lookup_product(27),
        firmware: FirmwareVersion::new(3, 70),
        topology: LifxTopology::SingleZone,
    };
    DriverTrackedDevice {
        info: profile.device_info(20),
        metadata: HashMap::from([
            ("ip".to_owned(), ip.to_owned()),
            ("port".to_owned(), port.to_owned()),
        ]),
        fingerprint: None,
        current_state: DeviceState::Known,
    }
}

#[test]
fn probe_targets_merge_config_and_tracked_devices() {
    let config = LifxConfig {
        device_ips: vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        ..LifxConfig::default()
    };
    let targets = resolve_lifx_probe_targets(
        &config,
        &[
            tracked("192.168.1.40", "56700"),
            tracked("192.168.1.41", "56701"),
            tracked("127.0.0.1", "56700"),
            tracked("not-an-ip", "56700"),
        ],
    );
    assert_eq!(
        targets,
        vec![
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 40), 56_700)),
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 41), 56_701)),
        ]
    );
}

#[test]
fn device_surface_shows_diagnostics_and_the_refresh_action() {
    let mut device = tracked("192.168.1.40", "56700");
    device
        .metadata
        .insert("serial".to_owned(), "d073d5010203".to_owned());
    device
        .metadata
        .insert("device_class".to_owned(), "single_zone".to_owned());
    let ctx = TrackedDeviceCtx {
        device_id: device.info.id,
        info: &device.info,
        metadata: Some(&device.metadata),
        current_state: &device.current_state,
    };
    let surface = lifx_device_control_surface(&ctx);
    assert_eq!(
        surface.values["serial"],
        ControlValue::Text("d073d5010203".to_owned())
    );
    assert_eq!(surface.values["led_count"], ControlValue::Int(1));
    assert_eq!(surface.values["max_fps"], ControlValue::Int(20));
    assert_eq!(
        surface.values["model"],
        ControlValue::Text("LIFX (A19)".to_owned())
    );
    assert!(surface.values.contains_key("ip"));
    assert_eq!(surface.actions.len(), 1);
    assert_eq!(surface.actions[0].id, "refresh_topology");
    assert_ne!(surface.revision, 0);
}

#[test]
fn tracked_routes_come_from_the_metadata_a_probe_verified() {
    let mut located = tracked("192.168.1.40", "56700");
    located
        .metadata
        .insert("serial".to_owned(), "d073d5010203".to_owned());
    let fingerprint = fingerprint_for_serial(Serial([0xd0, 0x73, 0xd5, 1, 2, 3]));
    located.fingerprint = Some(fingerprint.clone());
    let unregistered = DriverTrackedDevice {
        fingerprint: None,
        ..located.clone()
    };
    let unverified = tracked("192.168.1.41", "56700");

    let routes = tracked_routes(&[located, unregistered, unverified]);

    assert_eq!(routes.len(), 1, "a route needs a fingerprint and a serial");
    assert_eq!(
        routes[&Serial([0xd0, 0x73, 0xd5, 1, 2, 3])],
        TrackedRoute {
            addr: SocketAddr::from((Ipv4Addr::new(192, 168, 1, 40), 56_700)),
            fingerprint,
        }
    );
}
