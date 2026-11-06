//! Driver module, discovery, config provider, and control surface tests.

use std::collections::HashMap;

use hypercolor_driver_api::{ControlApplyTarget, DriverConfigView, DriverModule, TrackedDeviceCtx};
use hypercolor_driver_pixel::controls::{
    FIELD_DEVICES, FIELD_PRIORITY, FIELD_SOURCE_NAME, parse_validated_config, pixel_config_values,
    validate_pixel_changes, values_entry,
};
use hypercolor_driver_pixel::{
    DESCRIPTOR, METADATA_DEVICE_CONFIG, PixelConfig, PixelDriverModule, configured_devices,
    pixel_device_control_surface, pixel_driver_control_surface,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::{ControlValue, IpText};
use hypercolor_types::controls::{ApplyImpact, ControlChange};
use hypercolor_types::device::{
    DeviceColorFormat, DeviceFingerprint, DeviceState, DriverModuleKind, FingerprintNamespace,
};
use serde_json::json;

fn sample_config() -> PixelConfig {
    serde_json::from_value(json!({
        "source_name": "Garage",
        "priority": 120,
        "devices": [
            { "id": "roofline", "name": "Roofline", "host": "10.0.0.60", "protocol": "e131",
              "segments": [
                { "name": "Front", "pixel_count": 300 },
                { "pixel_count": 150, "start_universe": 4, "start_channel": 1 }
              ] },
            { "id": "pars", "host": "10.0.0.70", "protocol": "artnet", "pixel_count": 4,
              "color_order": "rgbw", "start_channel": 17, "max_fps": 40 },
            { "id": "tree", "host": "10.0.0.61", "port": 4049, "protocol": "ddp",
              "pixel_count": 1000, "ddp_offset": 3 }
        ]
    }))
    .expect("sample config")
}

#[test]
fn module_descriptor_is_a_default_enabled_network_driver() {
    let module = PixelDriverModule::new();
    let descriptor = module.module_descriptor();
    assert_eq!(descriptor.id, "pixel");
    assert_eq!(descriptor.module_kind, DriverModuleKind::Network);
    assert!(descriptor.default_enabled);
    assert!(descriptor.capabilities.config);
    assert!(descriptor.capabilities.discovery);
    assert!(descriptor.capabilities.controls);
    assert!(descriptor.capabilities.output_backend);
    assert!(descriptor.capabilities.presentation);
    assert!(!descriptor.capabilities.pairing);
    assert!(module.output().is_output());
}

#[test]
fn default_config_is_empty_and_valid() {
    let module = PixelDriverModule::new();
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
    let module = PixelDriverModule::new();
    let entry = DriverConfigEntry::enabled(
        json!({ "devices": [{ "id": "x", "host": "239.255.0.1", "protocol": "e131", "pixel_count": 1 }] })
            .as_object()
            .expect("object")
            .clone()
            .into_iter()
            .collect(),
    );
    let error = module
        .config()
        .expect("config provider")
        .validate_config(&entry)
        .expect_err("multicast host should be rejected");
    assert!(error.to_string().contains("not a unicast address"));
}

#[test]
fn configured_devices_carry_identity_topology_and_metadata() {
    let devices = configured_devices(&sample_config()).expect("resolve");
    assert_eq!(devices.len(), 3);

    let roofline = &devices[0];
    let fingerprint = DeviceFingerprint::mint(FingerprintNamespace::Net, "pixel", "roofline");
    assert_eq!(roofline.fingerprint, fingerprint);
    assert_eq!(roofline.info.id, fingerprint.stable_device_id());
    assert_eq!(roofline.info.name, "Roofline");
    assert_eq!(roofline.info.driver_id(), "pixel");
    assert_eq!(roofline.info.model.as_deref(), Some("E1.31 (sACN)"));
    assert_eq!(roofline.info.total_led_count(), 450);
    assert_eq!(roofline.info.segments.len(), 2);
    assert_eq!(roofline.info.segments[0].name, "Front");
    assert_eq!(roofline.info.segments[1].name, "Output 2");
    assert_eq!(roofline.info.capabilities.max_fps, 60);
    assert!(roofline.claim.is_none());
    assert_eq!(roofline.metadata["ip"], "10.0.0.60");
    assert_eq!(roofline.metadata["port"], "5568");
    assert_eq!(roofline.metadata["protocol"], "e131");
    assert_eq!(roofline.metadata["universes"], "1-2, 4");

    // Driver-level defaults are baked into the metadata the backend adopts.
    let effective: serde_json::Value =
        serde_json::from_str(&roofline.metadata[METADATA_DEVICE_CONFIG]).expect("JSON");
    assert_eq!(effective["priority"], 120);
    assert_eq!(effective["source_name"], "Garage");

    let pars = &devices[1];
    assert_eq!(pars.info.segments[0].color_format, DeviceColorFormat::Rgbw);
    assert_eq!(pars.info.capabilities.max_fps, 40);
    assert_eq!(pars.info.name, "pars", "name falls back to the id");
    let pars_effective: serde_json::Value =
        serde_json::from_str(&pars.metadata[METADATA_DEVICE_CONFIG]).expect("JSON");
    assert!(
        pars_effective.get("priority").is_none(),
        "E1.31 defaults stay off Art-Net"
    );

    let tree = &devices[2];
    assert_eq!(tree.metadata["port"], "4049");
    assert!(!tree.metadata.contains_key("universes"));
}

#[test]
fn device_ids_stay_stable_when_hosts_move() {
    let mut config = sample_config();
    let before = configured_devices(&config).expect("resolve")[2].info.id;
    config.devices[2].host = "10.0.0.99".parse().expect("valid IP");
    config.devices[2].name = Some("Renamed".to_owned());
    let after = configured_devices(&config).expect("resolve")[2].info.id;
    assert_eq!(before, after);
}

#[test]
fn control_values_round_trip_through_driver_settings() {
    let config = sample_config();
    let values = pixel_config_values(&config);
    let entry = values_entry(&values).expect("serialize values");
    let parsed = parse_validated_config(&entry).expect("round trip should validate");
    assert_eq!(parsed, config);
}

#[test]
fn driver_surface_lists_fields_and_values() {
    let surface = pixel_driver_control_surface(&sample_config());
    let ids: Vec<_> = surface
        .fields
        .iter()
        .map(|field| field.id.as_str())
        .collect();
    assert_eq!(ids, vec![FIELD_DEVICES, FIELD_SOURCE_NAME, FIELD_PRIORITY]);
    assert!(
        surface
            .fields
            .iter()
            .all(|field| field.apply_impact == ApplyImpact::DiscoveryRescan)
    );
    for field in &surface.fields {
        let value = surface
            .values
            .get(&field.id)
            .expect("every field has a value");
        field
            .value_type
            .validate_value(value)
            .unwrap_or_else(|error| panic!("{} value should match its type: {error}", field.id));
    }
    assert_eq!(surface.values[FIELD_PRIORITY], ControlValue::Int(120));
}

fn driver_target_changes(changes: &[ControlChange]) -> anyhow::Result<()> {
    let config = sample_config();
    let entry = values_entry(&pixel_config_values(&config)).expect("entry");
    let target = ControlApplyTarget::Driver {
        driver_id: DESCRIPTOR.id,
        config: DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: &entry,
        },
    };
    validate_pixel_changes(&target, changes).map(|_| ())
}

#[test]
fn control_changes_are_validated_against_the_whole_config() {
    let mut devices = match pixel_config_values(&sample_config()).remove(FIELD_DEVICES) {
        Some(ControlValue::List(devices)) => devices,
        other => panic!("devices should be a list, got {other:?}"),
    };
    driver_target_changes(&[ControlChange {
        field_id: FIELD_PRIORITY.to_owned(),
        value: ControlValue::Int(150),
    }])
    .expect("priority change is valid");

    // Point the DDP tree at the roofline's universes on the same receiver
    // and protocol: the whole-config check must catch the conflict.
    let ControlValue::Map(tree) = &mut devices[2] else {
        panic!("device entries are maps");
    };
    tree.insert(
        "host".to_owned(),
        ControlValue::Ip(IpText::new("10.0.0.60").expect("valid IP")),
    );
    tree.insert("protocol".to_owned(), ControlValue::Enum("e131".to_owned()));
    tree.remove("ddp_offset");
    tree.remove("port");
    let error = driver_target_changes(&[ControlChange {
        field_id: FIELD_DEVICES.to_owned(),
        value: ControlValue::List(devices),
    }])
    .expect_err("universe conflict should be rejected");
    assert!(format!("{error:#}").contains("both send E1.31 (sACN) universe 1"));

    let error = driver_target_changes(&[ControlChange {
        field_id: FIELD_PRIORITY.to_owned(),
        value: ControlValue::Int(201),
    }])
    .expect_err("out-of-range priority should be rejected");
    assert!(format!("{error:#}").contains("priority"));
}

#[test]
fn device_surface_is_read_only_diagnostics() {
    let devices = configured_devices(&sample_config()).expect("resolve");
    let roofline = &devices[0];
    let metadata: HashMap<String, String> = roofline.metadata.clone();
    let ctx = TrackedDeviceCtx {
        device_id: roofline.info.id,
        info: &roofline.info,
        metadata: Some(&metadata),
        current_state: &DeviceState::Known,
    };
    let surface = pixel_device_control_surface(&ctx);
    assert_eq!(
        surface.values["universes"],
        ControlValue::Text("1-2, 4".to_owned())
    );
    assert_eq!(surface.values["pixel_count"], ControlValue::Int(450));
    assert_eq!(
        surface.values["protocol"],
        ControlValue::Text("E1.31 (sACN)".to_owned())
    );

    let target = ControlApplyTarget::Device { device: &ctx };
    let error = validate_pixel_changes(
        &target,
        &[ControlChange {
            field_id: "pixel_count".to_owned(),
            value: ControlValue::Int(1),
        }],
    )
    .expect_err("device fields are read-only");
    assert!(error.to_string().contains("read-only"));
}
