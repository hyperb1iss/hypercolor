//! Driver module surface: descriptor, config, controls, probe targets
//! (Spec 93 §9).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_api::{
    DriverConfigView, DriverModule, DriverTrackedDevice, TrackedDeviceCtx,
};
use hypercolor_driver_wiz::config::WizConfig;
use hypercolor_driver_wiz::{
    DESCRIPTOR, WizDriverModule, resolve_wiz_probe_targets, wiz_device_control_surface,
    wiz_driver_control_surface,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceInfo, DeviceOrigin, DeviceState, DriverTransportKind,
};

fn entry(settings: serde_json::Value) -> DriverConfigEntry {
    let settings: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(settings).expect("settings map");
    DriverConfigEntry::enabled(settings)
}

#[test]
fn descriptor_is_an_enabled_network_driver_without_pairing() {
    let module = WizDriverModule::new();
    assert_eq!(DESCRIPTOR.id, "wiz");
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
    let presentation = module
        .presentation()
        .expect("presentation provider")
        .presentation();
    assert_eq!(presentation.label, "WiZ");
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Light)
    );
}

#[test]
fn default_config_is_enabled_and_valid() {
    let module = WizDriverModule::new();
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
    .parse_settings::<WizConfig>()
    .expect("default config parses");
    assert_eq!(parsed, WizConfig::default());
    assert!(parsed.broadcast_discovery);
    assert_eq!(parsed.max_fps, 20);
    assert_eq!(parsed.signing_key, None);
}

#[test]
fn empty_settings_fall_back_to_defaults() {
    let parsed = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry(serde_json::json!({})),
    }
    .parse_settings::<WizConfig>()
    .expect("empty settings parse");
    assert_eq!(parsed, WizConfig::default());
}

#[test]
fn validation_rejects_bad_ips_and_rates() {
    let module = WizDriverModule::new();
    let config = module.config().expect("config provider");
    assert!(
        config
            .validate_config(&entry(serde_json::json!({ "device_ips": ["127.0.0.1"] })))
            .is_err()
    );
    assert!(
        config
            .validate_config(&entry(serde_json::json!({ "max_fps": 0 })))
            .is_err()
    );
    assert!(
        config
            .validate_config(&entry(serde_json::json!({ "max_fps": 61 })))
            .is_err()
    );
    assert!(
        config
            .validate_config(&entry(serde_json::json!({ "signing_key": "not-hex" })))
            .is_err()
    );
    assert!(
        config
            .validate_config(&entry(serde_json::json!({
                "signing_key": "00112233445566778899aabbccddeeff",
            })))
            .is_ok()
    );
    assert!(
        config
            .validate_config(&entry(serde_json::json!({
                "device_ips": ["192.168.1.40"],
                "max_fps": 30,
            })))
            .is_ok()
    );
}

fn tracked(metadata: HashMap<String, String>) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: sample_info(),
        metadata,
        fingerprint: None,
        current_state: DeviceState::Known,
    }
}

fn sample_info() -> DeviceInfo {
    DeviceInfo {
        id: hypercolor_driver_wiz::scanner::fingerprint_for_mac("a8:bb:50:00:00:01")
            .stable_device_id(),
        name: "WiZ Color 000001".to_owned(),
        vendor: "WiZ".to_owned(),
        family: DeviceFamily::new_static("wiz", "WiZ"),
        model: Some("ESP01_SHRGB1C_31".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wiz", "wiz", ConnectionType::Network),
        segments: Vec::new(),
        firmware_version: Some("1.25.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 1,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 20,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn probe_targets_merge_config_and_tracked_devices() {
    let config = WizConfig {
        device_ips: vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        ..WizConfig::default()
    };
    let tracked = [
        tracked(HashMap::from([
            ("ip".to_owned(), "192.168.1.51".to_owned()),
            ("port".to_owned(), "40000".to_owned()),
        ])),
        tracked(HashMap::from([(
            "ip".to_owned(),
            "192.168.1.50".to_owned(),
        )])),
        tracked(HashMap::from([("ip".to_owned(), "not an ip".to_owned())])),
    ];
    assert_eq!(
        resolve_wiz_probe_targets(&config, &tracked),
        [
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 50), 38899)),
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 51), 40000)),
        ]
    );
}

#[test]
fn driver_surface_exposes_connection_and_output_fields() {
    let surface = wiz_driver_control_surface(&WizConfig {
        signing_key: Some("00112233445566778899aabbccddeeff".to_owned()),
        ..WizConfig::default()
    });
    let ids: Vec<&str> = surface
        .fields
        .iter()
        .map(|field| field.id.as_str())
        .collect();
    assert_eq!(ids, ["device_ips", "broadcast_discovery", "max_fps"]);
    assert_eq!(
        surface.values["broadcast_discovery"],
        ControlValue::Bool(true)
    );
    assert_eq!(surface.values["max_fps"], ControlValue::Int(20));
    assert!(
        !surface.values.contains_key("signing_key"),
        "the signing key never reaches a control surface"
    );
}

#[test]
fn device_surface_shows_diagnostics() {
    let info = sample_info();
    let metadata = HashMap::from([
        ("ip".to_owned(), "192.168.1.50".to_owned()),
        ("mac".to_owned(), "a8:bb:50:00:00:01".to_owned()),
        ("module_name".to_owned(), "ESP01_SHRGB1C_31".to_owned()),
        ("module_class".to_owned(), "rgb".to_owned()),
        ("min_dimming".to_owned(), "10".to_owned()),
    ]);
    let state = DeviceState::Known;
    let surface = wiz_device_control_surface(&TrackedDeviceCtx {
        device_id: info.id,
        info: &info,
        metadata: Some(&metadata),
        current_state: &state,
    });
    assert!(surface.actions.is_empty());
    let values: Vec<&str> = surface.values.keys().map(String::as_str).collect();
    for expected in [
        "ip",
        "mac",
        "module_name",
        "module_class",
        "min_dimming",
        "max_fps",
    ] {
        assert!(
            values.contains(&expected),
            "missing {expected} in {values:?}"
        );
    }
}
