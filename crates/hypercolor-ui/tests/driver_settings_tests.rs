#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/driver_settings.rs"]
mod driver_settings;

use hypercolor_types::device::{
    DriverCapabilitySet, DriverModuleDescriptor, DriverModuleKind, DriverTransportKind,
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
        enabled: true,
        config_key: format!("drivers.{id}"),
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
    ]);

    assert_eq!(
        settings,
        vec![DiscoveryDriverSetting {
            id: "leaf".to_string(),
            label: "Leaf Driver Scan".to_string(),
            description: "Discover and pair Leaf Driver devices over the network".to_string(),
            key: "drivers.leaf.enabled".to_string(),
        }]
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
                    "backend_factory": true,
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
            "enabled": true,
            "config_key": "drivers.testnet"
        }]
    }"#;

    let response: DriverListResponse = serde_json::from_str(json).expect("driver list response");

    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].descriptor.id, "testnet");
    assert_eq!(response.items[0].config_key, "drivers.testnet");
}
