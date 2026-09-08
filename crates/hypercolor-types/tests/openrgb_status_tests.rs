//! Shared guided setup wire contract coverage.

use hypercolor_types::api::system::{
    OpenRgbEndpointStatus, OpenRgbInstallHint, OpenRgbPermissionStatus, OpenRgbStatus,
};
use hypercolor_types::config::DriverConfigEntry;

#[test]
fn setup_status_preserves_unreachable_endpoint_and_actionable_remedies() {
    let status = OpenRgbStatus {
        compiled: true,
        enabled: false,
        platform: "linux".into(),
        binary_path: None,
        binary_version: None,
        bridge_config: DriverConfigEntry::default(),
        probes: vec![OpenRgbEndpointStatus {
            endpoint: "127.0.0.1:6742".into(),
            reachable: false,
            protocol_version: None,
            controller_count: None,
            error: Some("connection refused".into()),
        }],
        install_hints: vec![OpenRgbInstallHint {
            command: "install OpenRGB".into(),
            note: "Use the host package manager".into(),
        }],
        permission_checks: vec![OpenRgbPermissionStatus {
            id: "udev_rules".into(),
            ok: false,
            detail: "rules missing".into(),
            remedy: Some("install rules".into()),
        }],
        coverage: Vec::new(),
        output_disabled_count: 0,
    };
    let value = serde_json::to_value(&status).expect("serialize status");
    assert_eq!(value["probes"][0]["reachable"], false);
    assert_eq!(value["permission_checks"][0]["remedy"], "install rules");
    assert_eq!(
        serde_json::from_value::<OpenRgbStatus>(value).expect("deserialize status"),
        status
    );
}
