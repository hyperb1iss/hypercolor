//! Driver module surface: descriptor, config, controls, probe targets
//! (Spec 92 §10).

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};

use hypercolor_driver_api::{
    DriverConfigView, DriverModule, DriverTrackedDevice, TrackedDeviceCtx,
};
use hypercolor_driver_twinkly::config::TwinklyConfig;
use hypercolor_driver_twinkly::{
    DESCRIPTOR, TwinklyDriverModule, TwinklyEndpoint, resolve_twinkly_probe_targets,
    twinkly_device_control_surface, twinkly_driver_control_surface,
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
    let module = TwinklyDriverModule::new();
    assert_eq!(DESCRIPTOR.id, "twinkly");
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
    assert_eq!(presentation.label, "Twinkly");
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Light)
    );
}

#[test]
fn default_config_is_enabled_and_valid() {
    let module = TwinklyDriverModule::new();
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
    .parse_settings::<TwinklyConfig>()
    .expect("default config parses");
    assert_eq!(parsed, TwinklyConfig::default());
    assert!(parsed.broadcast_discovery);
    assert_eq!(parsed.max_fps, 60);
}

#[test]
fn empty_settings_fall_back_to_defaults() {
    let parsed = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry(serde_json::json!({})),
    }
    .parse_settings::<TwinklyConfig>()
    .expect("empty settings parse");
    assert_eq!(parsed, TwinklyConfig::default());
}

#[test]
fn validation_rejects_bad_ips_and_rates() {
    let module = TwinklyDriverModule::new();
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
            .validate_config(&entry(serde_json::json!({ "max_fps": 121 })))
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
        id: hypercolor_driver_twinkly::scanner::fingerprint_for_mac("98:f4:ab:1c:c1:90")
            .stable_device_id(),
        name: "Porch String".to_owned(),
        vendor: "Twinkly".to_owned(),
        family: DeviceFamily::new_static("twinkly", "Twinkly"),
        model: Some("TWS250STP".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("twinkly", "twinkly", ConnectionType::Network),
        segments: vec![hypercolor_driver_twinkly::layout::strip_segment(250)],
        firmware_version: Some("2.8.3".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 250,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn probe_targets_merge_config_and_tracked_devices() {
    let config = TwinklyConfig {
        device_ips: vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        ..TwinklyConfig::default()
    };
    let tracked = [
        tracked(HashMap::from([
            ("ip".to_owned(), "192.168.1.41".to_owned()),
            ("http_port".to_owned(), "8080".to_owned()),
        ])),
        tracked(HashMap::from([(
            "ip".to_owned(),
            "192.168.1.40".to_owned(),
        )])),
        tracked(HashMap::from([("ip".to_owned(), "not an ip".to_owned())])),
    ];
    let targets = resolve_twinkly_probe_targets(&config, &tracked);
    assert_eq!(
        targets,
        [
            TwinklyEndpoint::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40))),
            TwinklyEndpoint {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 41)),
                http_port: 8080,
                rt_port: 7777,
            },
        ]
    );
}

#[test]
fn driver_surface_exposes_connection_and_output_fields() {
    let surface = twinkly_driver_control_surface(&TwinklyConfig::default());
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
    assert_eq!(surface.values["max_fps"], ControlValue::Int(60));
}

#[test]
fn device_surface_shows_diagnostics_and_layout_refresh() {
    let info = sample_info();
    let metadata = HashMap::from([
        ("ip".to_owned(), "192.168.1.40".to_owned()),
        ("mac".to_owned(), "98:f4:ab:1c:c1:90".to_owned()),
        ("rt_protocol".to_owned(), "v3".to_owned()),
        ("led_profile".to_owned(), "RGB".to_owned()),
        ("layout_source".to_owned(), "3d".to_owned()),
        (
            "control_state".to_owned(),
            "yielded_to_another_app".to_owned(),
        ),
    ]);
    let state = DeviceState::Known;
    let surface = twinkly_device_control_surface(&TrackedDeviceCtx {
        device_id: info.id,
        info: &info,
        metadata: Some(&metadata),
        current_state: &state,
    });
    assert_eq!(surface.actions.len(), 1);
    assert_eq!(surface.actions[0].id, "refresh_layout");
    let values: Vec<&str> = surface.values.keys().map(String::as_str).collect();
    for expected in [
        "ip",
        "mac",
        "rt_protocol",
        "led_profile",
        "layout_source",
        "led_count",
    ] {
        assert!(
            values.contains(&expected),
            "missing {expected} in {values:?}"
        );
    }
    assert_eq!(surface.values["led_count"], ControlValue::Int(250));
}
