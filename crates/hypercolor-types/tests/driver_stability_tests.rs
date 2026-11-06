//! Spec 89 stability labels in the shared driver and device contracts.

use hypercolor_types::api::devices::UnclaimedDevice;
use hypercolor_types::device::{
    DRIVER_MODULE_API_SCHEMA_VERSION, DriverModuleDescriptor, DriverProtocolDescriptor,
    DriverStability,
};
use serde_json::json;

#[test]
fn stability_serializes_snake_case_and_defaults_to_stable() {
    assert_eq!(DriverStability::default(), DriverStability::Stable);
    assert_eq!(
        serde_json::to_value(DriverStability::Experimental).expect("serialize"),
        json!("experimental")
    );
    assert_eq!(
        serde_json::from_value::<DriverStability>(json!("stable")).expect("deserialize"),
        DriverStability::Stable
    );
    assert!(DriverStability::Experimental.is_experimental());
    assert!(!DriverStability::Stable.is_experimental());
}

fn module_json() -> serde_json::Value {
    json!({
        "id": "fixture",
        "display_name": "Fixture",
        "module_kind": "hal",
        "transports": [],
        "capabilities": {
            "config": false,
            "discovery": false,
            "pairing": false,
            "output_backend": false,
            "protocol_catalog": true,
            "runtime_cache": false,
            "credentials": false,
            "presentation": false,
            "controls": false
        },
        "api_schema_version": DRIVER_MODULE_API_SCHEMA_VERSION,
        "config_version": 1,
        "default_enabled": true
    })
}

#[test]
fn module_descriptor_without_stability_reads_as_stable() {
    let descriptor: DriverModuleDescriptor =
        serde_json::from_value(module_json()).expect("pre-Spec 89 module descriptor");

    assert_eq!(descriptor.stability, DriverStability::Stable);
    assert_eq!(
        serde_json::to_value(&descriptor).expect("serialize"),
        module_json(),
        "the stable default is omitted, so the payload round-trips unchanged"
    );
}

#[test]
fn module_descriptor_round_trips_the_experimental_label() {
    let mut value = module_json();
    value["stability"] = json!("experimental");

    let descriptor: DriverModuleDescriptor =
        serde_json::from_value(value).expect("experimental module descriptor");
    assert_eq!(descriptor.stability, DriverStability::Experimental);
    assert!(descriptor.default_enabled);

    let back = serde_json::to_value(&descriptor).expect("serialize");
    assert_eq!(back["stability"], "experimental");
}

#[test]
fn protocol_descriptor_without_stability_reads_as_stable() {
    let protocol: DriverProtocolDescriptor = serde_json::from_value(json!({
        "driver_id": "fixture",
        "protocol_id": "fixture/one",
        "display_name": "Fixture One",
        "family_id": "fixture",
        "transport": "usb",
        "route_backend_id": "usb"
    }))
    .expect("pre-Spec 89 protocol descriptor");

    assert_eq!(protocol.stability, DriverStability::Stable);
}

#[test]
fn unclaimed_device_mentions_experimental_only_when_set() {
    let quiet = UnclaimedDevice {
        vendor_id: 0x1e71,
        product_id: 0x2011,
        claimable_by: Some("fixture".to_owned()),
        ..UnclaimedDevice::default()
    };
    let value = serde_json::to_value(&quiet).expect("serialize");
    assert!(value.get("experimental").is_none());

    let labeled = UnclaimedDevice {
        experimental: true,
        ..quiet
    };
    let value = serde_json::to_value(&labeled).expect("serialize");
    assert_eq!(value["experimental"], true);
    assert_eq!(
        serde_json::from_value::<UnclaimedDevice>(value).expect("deserialize"),
        labeled
    );
}
