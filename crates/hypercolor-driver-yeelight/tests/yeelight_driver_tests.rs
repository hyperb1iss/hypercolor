//! Driver module surface: descriptor, config, controls, probe targets
//! (Spec 94 §8).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use hypercolor_driver_api::{
    DriverConfigView, DriverModule, DriverTrackedDevice, TrackedDeviceCtx,
};
use hypercolor_driver_yeelight::config::YeelightConfig;
use hypercolor_driver_yeelight::{
    DESCRIPTOR, YeelightDriverModule, resolve_yeelight_probe_targets,
    yeelight_device_control_surface, yeelight_driver_control_surface,
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
    let module = YeelightDriverModule::new();
    assert_eq!(DESCRIPTOR.id, "yeelight");
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
    assert_eq!(presentation.label, "Yeelight");
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Light)
    );
}

#[test]
fn default_config_is_enabled_and_valid() {
    let module = YeelightDriverModule::new();
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
    .parse_settings::<YeelightConfig>()
    .expect("default config parses");
    assert_eq!(parsed, YeelightConfig::default());
    assert!(parsed.multicast_discovery);
    assert!(parsed.music_mode);
    assert_eq!(parsed.max_fps, 25);
}

#[test]
fn empty_settings_fall_back_to_defaults() {
    let parsed = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry(serde_json::json!({})),
    }
    .parse_settings::<YeelightConfig>()
    .expect("empty settings parse");
    assert_eq!(parsed, YeelightConfig::default());
}

#[test]
fn validation_rejects_bad_ips_and_rates() {
    let module = YeelightDriverModule::new();
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
        id: hypercolor_driver_yeelight::scanner::fingerprint_for_id("0x000000000015243f")
            .stable_device_id(),
        name: "Desk Lamp".to_owned(),
        vendor: "Yeelight".to_owned(),
        family: DeviceFamily::new_static("yeelight", "Yeelight"),
        model: Some("color".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("yeelight", "yeelight", ConnectionType::Network),
        segments: Vec::new(),
        firmware_version: Some("18".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 1,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 25,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn probe_targets_merge_config_and_tracked_devices() {
    let config = YeelightConfig {
        device_ips: vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        ..YeelightConfig::default()
    };
    let tracked = [
        tracked(HashMap::from([
            ("ip".to_owned(), "192.168.1.51".to_owned()),
            ("search_port".to_owned(), "40000".to_owned()),
        ])),
        tracked(HashMap::from([(
            "ip".to_owned(),
            "192.168.1.50".to_owned(),
        )])),
        tracked(HashMap::from([("ip".to_owned(), "not an ip".to_owned())])),
    ];
    assert_eq!(
        resolve_yeelight_probe_targets(&config, &tracked),
        [
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 50), 1982)),
            SocketAddr::from((Ipv4Addr::new(192, 168, 1, 51), 40000)),
        ]
    );
}

#[test]
fn driver_surface_exposes_connection_and_output_fields() {
    let surface = yeelight_driver_control_surface(&YeelightConfig::default());
    let ids: Vec<&str> = surface
        .fields
        .iter()
        .map(|field| field.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["device_ips", "multicast_discovery", "music_mode", "max_fps"]
    );
    assert_eq!(surface.values["music_mode"], ControlValue::Bool(true));
    assert_eq!(surface.values["max_fps"], ControlValue::Int(25));
}

#[test]
fn device_surface_shows_diagnostics() {
    let info = sample_info();
    let metadata = HashMap::from([
        ("ip".to_owned(), "192.168.1.50".to_owned()),
        ("yeelight_id".to_owned(), "0x000000000015243f".to_owned()),
        ("light_class".to_owned(), "color".to_owned()),
        ("music_mode".to_owned(), "on".to_owned()),
        ("stream_mode".to_owned(), "music".to_owned()),
    ]);
    let state = DeviceState::Known;
    let surface = yeelight_device_control_surface(&TrackedDeviceCtx {
        device_id: info.id,
        info: &info,
        metadata: Some(&metadata),
        current_state: &state,
    });
    assert!(surface.actions.is_empty());
    let values: Vec<&str> = surface.values.keys().map(String::as_str).collect();
    for expected in [
        "ip",
        "yeelight_id",
        "light_class",
        "music_mode",
        "stream_mode",
        "max_fps",
    ] {
        assert!(
            values.contains(&expected),
            "missing {expected} in {values:?}"
        );
    }
}
