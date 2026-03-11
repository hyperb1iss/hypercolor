//! Tests for shared server discovery types.

use std::net::{IpAddr, Ipv4Addr};

use hypercolor_types::server::{DiscoveredServer, ServerIdentity};

#[test]
fn discovered_server_json_roundtrip() {
    let original = DiscoveredServer {
        identity: ServerIdentity {
            instance_id: "01912345-6789-7abc-def0-123456789abc".to_owned(),
            instance_name: "desk-pc".to_owned(),
            version: "0.1.0".to_owned(),
        },
        host: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
        port: 9420,
        device_count: Some(3),
        auth_required: true,
    };

    let json = serde_json::to_value(&original).expect("serialize discovered server");
    let restored: DiscoveredServer =
        serde_json::from_value(json).expect("deserialize discovered server");

    assert_eq!(restored, original);
}

#[test]
fn discovered_server_defaults_missing_device_count() {
    let json = serde_json::json!({
        "identity": {
            "instance_id": "01912345-6789-7abc-def0-123456789abc",
            "instance_name": "desk-pc",
            "version": "0.1.0"
        },
        "host": "192.168.1.42",
        "port": 9420,
        "auth_required": false
    });

    let restored: DiscoveredServer =
        serde_json::from_value(json).expect("deserialize discovered server");

    assert_eq!(restored.device_count, None);
    assert!(!restored.auth_required);
}
