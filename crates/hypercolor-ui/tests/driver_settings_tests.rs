#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/driver_settings.rs"]
mod driver_settings;
#[path = "../src/label_utils.rs"]
mod label_utils;

use hypercolor_types::device::{
    DriverCapabilitySet, DriverModuleDescriptor, DriverModuleKind, DriverPresentation,
    DriverTransportKind,
};

use api::{DriverListResponse, DriverSummary};
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
            api_schema_version: 1,
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
                "api_schema_version": 1,
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
    }"#;

    let response: DriverListResponse = serde_json::from_str(json).expect("driver list response");

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
