#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/driver_settings.rs"]
mod driver_settings;
#[path = "../src/label_utils.rs"]
mod label_utils;

use hypercolor_types::device::{
    DRIVER_MODULE_API_SCHEMA_VERSION, DriverCapabilitySet, DriverModuleDescriptor,
    DriverModuleKind, DriverPresentation, DriverTransportKind,
};

use api::{DriverConfigResponse, DriverListResponse, DriverSummary, driver_config_url};
use driver_settings::{DiscoveryDriverSetting, discovery_driver_settings};

fn driver(
    id: &str,
    display_name: &str,
    discovery: bool,
    pairing: bool,
    transports: Vec<DriverTransportKind>,
) -> DriverSummary {
    DriverSummary {
        descriptor: DriverModuleDescriptor {
            id: id.to_string(),
            display_name: display_name.to_string(),
            vendor_name: None,
            module_kind: DriverModuleKind::Network,
            transports,
            capabilities: DriverCapabilitySet {
                discovery,
                pairing,
                ..DriverCapabilitySet::empty()
            },
            api_schema_version: DRIVER_MODULE_API_SCHEMA_VERSION,
            config_version: 1,
            default_enabled: true,
        },
        presentation: DriverPresentation {
            label: display_name.to_string(),
            short_label: None,
            accent_rgb: None,
            secondary_rgb: None,
            icon: None,
            default_device_class: None,
        },
        enabled: true,
        config_key: format!("drivers.{id}"),
        protocols: Vec::new(),
        control_surface_id: None,
        control_surface_path: None,
    }
}

#[test]
fn discovery_settings_follow_driver_descriptors() {
    let settings = discovery_driver_settings(&[
        driver(
            "leaf",
            "Leaf Driver",
            true,
            true,
            vec![DriverTransportKind::Network],
        ),
        driver(
            "catalog",
            "Catalog Only",
            false,
            false,
            vec![DriverTransportKind::Usb],
        ),
        driver(
            "bridge",
            "Bridge Driver",
            true,
            false,
            vec![DriverTransportKind::Bridge],
        ),
        driver(
            "external",
            "External Driver",
            true,
            false,
            vec![DriverTransportKind::Custom("open-link-hub".to_string())],
        ),
    ]);

    assert_eq!(
        settings,
        vec![
            DiscoveryDriverSetting {
                id: "leaf".to_string(),
                label: "Leaf Driver Scan".to_string(),
                description: "Discover and pair Leaf Driver devices over the network".to_string(),
                key: "drivers.leaf.enabled".to_string(),
            },
            DiscoveryDriverSetting {
                id: "bridge".to_string(),
                label: "Bridge Driver Scan".to_string(),
                description: "Discover Bridge Driver devices over bridge services".to_string(),
                key: "drivers.bridge.enabled".to_string(),
            },
            DiscoveryDriverSetting {
                id: "external".to_string(),
                label: "External Driver Scan".to_string(),
                description: "Discover External Driver devices over Open Link Hub".to_string(),
                key: "drivers.external.enabled".to_string(),
            }
        ]
    );
}

#[test]
fn driver_list_response_deserializes_daemon_data() {
    let json = r#"{
        "items": [{
            "descriptor": {
                "id": "testnet",
                "display_name": "Test Network",
                "module_kind": "network",
                "transports": ["network"],
                "capabilities": {
                    "config": false,
                    "discovery": true,
                    "pairing": false,
                    "output_backend": true,
                    "protocol_catalog": false,
                    "runtime_cache": false,
                    "credentials": false,
                    "presentation": false,
                    "controls": false
                },
                "api_schema_version": __SCHEMA_VERSION__,
                "config_version": 1,
                "default_enabled": true
            },
            "presentation": {
                "label": "Test Network",
                "accent_rgb": [128, 255, 234]
            },
            "enabled": true,
            "config_key": "drivers.testnet",
            "protocols": [{
                "driver_id": "testnet",
                "protocol_id": "testnet/proto",
                "display_name": "Test Protocol",
                "vendor_id": 4660,
                "product_id": 22136,
                "family_id": "testnet",
                "transport": "network",
                "route_backend_id": "network"
            }]
        }]
    }"#
    .replace(
        "__SCHEMA_VERSION__",
        &DRIVER_MODULE_API_SCHEMA_VERSION.to_string(),
    );

    let response: DriverListResponse = serde_json::from_str(&json).expect("driver list response");

    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].descriptor.id, "testnet");
    assert_eq!(response.items[0].presentation.label, "Test Network");
    assert_eq!(
        response.items[0].presentation.accent_rgb,
        Some([128, 255, 234])
    );
    assert_eq!(response.items[0].config_key, "drivers.testnet");
    assert_eq!(response.items[0].protocols[0].protocol_id, "testnet/proto");
    assert_eq!(response.items[0].protocols[0].route_backend_id, "network");
}

#[test]
fn driver_config_url_encodes_driver_id_path_segment() {
    assert_eq!(
        driver_config_url("external driver/one"),
        "/api/v1/drivers/external%20driver%2Fone/config"
    );
}

#[test]
fn driver_config_response_deserializes_flattened_entries() {
    let json = r#"{
        "driver_id": "wled",
        "config_key": "drivers.wled",
        "configurable": true,
        "current": {
            "enabled": true,
            "known_ips": ["192.168.1.50"]
        },
        "default": {
            "enabled": true,
            "known_ips": [],
            "default_protocol": "ddp"
        }
    }"#;

    let response: DriverConfigResponse =
        serde_json::from_str(json).expect("driver config response");

    assert_eq!(response.driver_id, "wled");
    assert_eq!(response.config_key, "drivers.wled");
    assert!(response.configurable);
    assert_eq!(
        response.current.settings["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );
    let default = response.default.expect("default config should deserialize");
    assert_eq!(default.settings["default_protocol"], "ddp");
}

#[test]
fn driver_config_response_handles_non_configurable_entries() {
    let json = r#"{
        "driver_id": "nollie",
        "config_key": "drivers.nollie",
        "configurable": false,
        "current": {
            "enabled": false
        }
    }"#;

    let response: DriverConfigResponse =
        serde_json::from_str(json).expect("non-configurable driver config response");

    assert_eq!(response.driver_id, "nollie");
    assert!(!response.configurable);
    assert!(!response.current.enabled);
    assert!(response.current.settings.is_empty());
    assert!(response.default.is_none());
}
