//! Driver module, discovery, config provider, and control surface tests.

use std::collections::HashMap;

use hypercolor_driver_adalight::controls::{
    FIELD_DEVICES, adalight_config_values, parse_validated_config, validate_adalight_changes,
    values_entry,
};
use hypercolor_driver_adalight::{
    AdalightConfig, AdalightDriverModule, DESCRIPTOR, METADATA_DEVICE_CONFIG,
    adalight_device_control_surface, adalight_driver_control_surface, configured_devices,
};
use hypercolor_driver_api::{ControlApplyTarget, DriverConfigView, DriverModule, TrackedDeviceCtx};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{ApplyImpact, ControlChange};
use hypercolor_types::device::{
    ConnectionType, DeviceColorFormat, DeviceFingerprint, DeviceState, DriverTransportKind,
    FingerprintNamespace,
};
use serde_json::json;

fn sample_config() -> AdalightConfig {
    serde_json::from_value(json!({
        "devices": [
            { "id": "desk", "name": "Desk Strip", "port": "/dev/ttyUSB0", "led_count": 300 },
            { "id": "tv", "port": "COM7", "protocol": "awa", "led_count": 120,
              "color_order": "grb", "white": "host", "max_fps": 45 }
        ]
    }))
    .expect("sample config")
}

fn entry(value: serde_json::Value) -> DriverConfigEntry {
    DriverConfigEntry::enabled(
        value
            .as_object()
            .expect("object")
            .clone()
            .into_iter()
            .collect(),
    )
}

#[test]
fn module_descriptor_is_a_default_enabled_serial_driver() {
    let module = AdalightDriverModule::new();
    let descriptor = module.module_descriptor();
    assert_eq!(descriptor.id, "adalight");
    assert!(descriptor.default_enabled);
    assert_eq!(descriptor.transports.len(), 1);
    assert_eq!(descriptor.transports[0].kind, DriverTransportKind::Serial);
    assert!(descriptor.capabilities.config);
    assert!(descriptor.capabilities.discovery);
    assert!(descriptor.capabilities.controls);
    assert!(descriptor.capabilities.output_backend);
    assert!(descriptor.capabilities.presentation);
    assert!(!descriptor.capabilities.pairing);
    assert!(!descriptor.capabilities.protocol_catalog);
    assert!(module.output().is_output());
    assert_eq!(DESCRIPTOR.transport, DriverTransportKind::Serial);
}

#[test]
fn default_config_lists_no_devices_and_validates() {
    let module = AdalightDriverModule::new();
    let provider = module.config().expect("config provider");
    let entry = provider.default_config();
    assert!(entry.enabled);
    provider
        .validate_config(&entry)
        .expect("default config should validate");
    let config = parse_validated_config(&entry).expect("default parses");
    assert!(config.devices.is_empty());
    assert!(configured_devices(&config).expect("no devices").is_empty());
}

#[test]
fn validate_config_reports_device_errors() {
    let module = AdalightDriverModule::new();
    let error = module
        .config()
        .expect("config provider")
        .validate_config(&entry(json!({
            "devices": [{ "id": "x", "port": "/dev/ttyUSB0", "led_count": 10, "white": "host" }]
        })))
        .expect_err("white without AWA should be rejected");
    assert!(error.to_string().contains("need protocol = \"awa\""));
}

#[test]
fn configured_devices_carry_identity_topology_and_line_budget() {
    let devices = configured_devices(&sample_config()).expect("resolve");
    assert_eq!(devices.len(), 2);

    let desk = &devices[0];
    let fingerprint = DeviceFingerprint::mint(FingerprintNamespace::Usb, "adalight", "desk");
    assert_eq!(desk.fingerprint, fingerprint);
    assert_eq!(desk.info.id, fingerprint.stable_device_id());
    assert_eq!(desk.info.name, "Desk Strip");
    assert_eq!(desk.info.driver_id(), "adalight");
    assert_eq!(desk.info.origin.transport, DriverTransportKind::Serial);
    assert_eq!(desk.info.connection_type, ConnectionType::Usb);
    assert_eq!(desk.info.model.as_deref(), Some("Adalight"));
    assert_eq!(desk.info.total_led_count(), 300);
    assert_eq!(desk.info.segments.len(), 1);
    assert_eq!(desk.info.segments[0].color_format, DeviceColorFormat::Rgb);
    // 906 bytes at 115200 baud: the line carries 12 frames per second.
    assert_eq!(desk.info.capabilities.max_fps, 12);
    assert!(
        desk.claim.is_none(),
        "configured devices never claim hardware"
    );
    assert_eq!(desk.metadata["port"], "/dev/ttyUSB0");
    assert_eq!(desk.metadata["baud_rate"], "115200");
    assert_eq!(desk.metadata["protocol"], "ada");
    assert_eq!(desk.metadata["frame_bytes"], "906");
    assert_eq!(desk.metadata["line_max_fps"], "12");

    let tv = &devices[1];
    assert_eq!(tv.info.name, "tv", "name falls back to the id");
    assert_eq!(tv.info.model.as_deref(), Some("AWA (HyperSerial)"));
    assert_eq!(tv.info.segments[0].color_format, DeviceColorFormat::Rgbw);
    assert_eq!(tv.info.capabilities.max_fps, 45, "user cap below the line");
    assert_eq!(tv.metadata["baud_rate"], "2000000");
    // 120 LEDs at four bytes each, plus header and trailer.
    assert_eq!(tv.metadata["frame_bytes"], "489");

    let stored: serde_json::Value =
        serde_json::from_str(&tv.metadata[METADATA_DEVICE_CONFIG]).expect("JSON");
    assert_eq!(stored["port"], "COM7");
    assert_eq!(stored["white"], "host");
}

#[test]
fn device_ids_stay_stable_when_ports_move() {
    let mut config = sample_config();
    let before = configured_devices(&config).expect("resolve")[0].info.id;
    config.devices[0].port = "/dev/ttyACM3".to_owned();
    config.devices[0].name = Some("Renamed".to_owned());
    let after = configured_devices(&config).expect("resolve")[0].info.id;
    assert_eq!(before, after);
}

#[test]
fn control_values_round_trip_through_driver_settings() {
    let config: AdalightConfig = serde_json::from_value(json!({
        "devices": [{
            "id": "tv", "name": "TV", "port": "/dev/ttyUSB1", "baud_rate": 1_000_000,
            "protocol": "awa", "led_count": 200, "color_order": "brg", "white": "firmware",
            "white_calibration": { "gain": 250, "red": 1, "green": 2, "blue": 3 },
            "startup_delay_ms": 2_000, "latch_ms": 3, "max_fps": 40
        }]
    }))
    .expect("config");
    let values = adalight_config_values(&config);
    let parsed = parse_validated_config(&values_entry(&values).expect("serialize values"))
        .expect("round trip should validate");
    assert_eq!(parsed, config);
}

#[test]
fn driver_surface_edits_the_devices_list() {
    let document = adalight_driver_control_surface(&sample_config());
    assert_eq!(document.fields.len(), 1);
    let field = &document.fields[0];
    assert_eq!(field.id, FIELD_DEVICES);
    assert_eq!(field.apply_impact, ApplyImpact::DiscoveryRescan);
    let ControlValue::List(devices) = &document.values[FIELD_DEVICES] else {
        panic!("devices should be a list");
    };
    assert_eq!(devices.len(), 2);
}

#[test]
fn driver_changes_validate_against_the_resulting_config() {
    let config_entry = entry(json!({ "devices": [] }));
    let target = ControlApplyTarget::Driver {
        driver_id: DESCRIPTOR.id,
        config: DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: &config_entry,
        },
    };

    let good = adalight_config_values(&sample_config())[FIELD_DEVICES].clone();
    validate_adalight_changes(
        &target,
        &[ControlChange {
            field_id: FIELD_DEVICES.to_owned(),
            value: good,
        }],
    )
    .expect("valid devices list");

    let mut duplicate = sample_config();
    duplicate.devices[1].port = "/dev/ttyUSB0".to_owned();
    let bad = adalight_config_values(&duplicate)[FIELD_DEVICES].clone();
    let error = validate_adalight_changes(
        &target,
        &[ControlChange {
            field_id: FIELD_DEVICES.to_owned(),
            value: bad,
        }],
    )
    .expect_err("duplicate port should fail");
    assert!(format!("{error:#}").contains("both use port /dev/ttyUSB0"));
}

#[test]
fn device_surface_is_read_only_and_shows_the_line_limit() {
    let devices = configured_devices(&sample_config()).expect("resolve");
    let desk = &devices[0];
    let metadata: HashMap<String, String> = desk.metadata.clone();
    let state = DeviceState::Known;
    let ctx = TrackedDeviceCtx {
        device_id: desk.info.id,
        info: &desk.info,
        metadata: Some(&metadata),
        current_state: &state,
    };
    let document = adalight_device_control_surface(&ctx);
    assert_eq!(
        document.values["port"],
        ControlValue::Text("/dev/ttyUSB0".to_owned())
    );
    assert_eq!(document.values["frame_bytes"], ControlValue::Int(906));
    assert_eq!(document.values["line_max_fps"], ControlValue::Int(12));
    assert_eq!(document.values["output_fps"], ControlValue::Int(12));
    assert!(
        document
            .fields
            .iter()
            .all(|field| field.apply_impact == ApplyImpact::None),
        "device fields are diagnostics only"
    );

    let error = validate_adalight_changes(
        &ControlApplyTarget::Device { device: &ctx },
        &[ControlChange {
            field_id: "port".to_owned(),
            value: ControlValue::Text("/dev/ttyUSB9".to_owned()),
        }],
    )
    .expect_err("device fields are read-only");
    assert!(error.to_string().contains("read-only"));
}
