//! Device API contract tests.

use hypercolor_types::api::devices::{DiscoverResponse, DiscoveryScanResult};
use serde_json::json;

#[test]
fn discovery_scanning_response_uses_a_closed_discriminator() {
    let response = DiscoverResponse::Scanning {
        scan_id: "scan_1".to_owned(),
        targets: vec!["wled".to_owned()],
        timeout_ms: 5_000,
    };

    let value = serde_json::to_value(&response).expect("serialize scanning response");
    assert_eq!(
        value,
        json!({
            "status": "scanning",
            "scan_id": "scan_1",
            "targets": ["wled"],
            "timeout_ms": 5_000
        })
    );
    assert_eq!(
        serde_json::from_value::<DiscoverResponse>(value).expect("deserialize scanning response"),
        response
    );
}

#[test]
fn discovery_completed_response_uses_a_closed_discriminator() {
    let response = DiscoverResponse::Completed {
        scan_id: "scan_2".to_owned(),
        result: DiscoveryScanResult {
            targets: vec!["wled".to_owned()],
            timeout_ms: 100,
            new_devices: Vec::new(),
            reappeared_devices: Vec::new(),
            vanished_devices: Vec::new(),
            total_known: 0,
            duration_ms: 4,
            scanners: Vec::new(),
        },
    };

    let value = serde_json::to_value(&response).expect("serialize completed response");
    assert_eq!(value["status"], "completed");
    assert_eq!(
        serde_json::from_value::<DiscoverResponse>(value).expect("deserialize completed response"),
        response
    );
}

#[test]
fn discovery_response_rejects_unknown_status() {
    serde_json::from_value::<DiscoverResponse>(json!({
        "status": "started",
        "scan_id": "scan_3",
        "targets": [],
        "timeout_ms": 100
    }))
    .expect_err("unknown discovery status must be rejected");
}

mod spec81 {
    use hypercolor_types::api::devices::{
        BridgeDeviceSummary, CoverageActive, CoverageBridgeDevice, CoverageIdentity,
        CoverageIdentityKind, CoverageNativeDevice, DeviceCoverageListResponse, DeviceCoverageRow,
        UnclaimedDevice, UnclaimedDeviceListResponse,
    };
    use serde_json::json;

    #[test]
    fn unclaimed_device_round_trips_with_optional_fields_defaulted() {
        let device = UnclaimedDevice {
            vendor_id: 0x1532,
            product_id: 0x0226,
            manufacturer: Some("Razer".to_owned()),
            product: Some("Huntsman".to_owned()),
            serial: None,
            bus_path: Some("1-1.2".to_owned()),
            interface_classes: vec![3],
            claimable_by: Some("razer".to_owned()),
        };
        let value = serde_json::to_value(&device).expect("serialize unclaimed device");
        assert_eq!(value["vendor_id"], 0x1532);
        assert_eq!(value["claimable_by"], "razer");
        assert_eq!(
            serde_json::from_value::<UnclaimedDevice>(value).expect("deserialize"),
            device
        );

        let minimal: UnclaimedDevice =
            serde_json::from_value(json!({ "vendor_id": 1, "product_id": 2 }))
                .expect("optional fields default");
        assert_eq!(minimal.interface_classes, Vec::<u8>::new());
        assert_eq!(minimal.claimable_by, None);

        let list = UnclaimedDeviceListResponse {
            items: vec![device],
            total: 1,
            page: None,
        };
        let value = serde_json::to_value(&list).expect("serialize list");
        assert!(value.get("page").is_none(), "unpaged lists omit page");
        assert_eq!(value["total"], 1);
    }

    #[test]
    fn coverage_row_uses_snake_case_discriminators() {
        let row = DeviceCoverageRow {
            identity: CoverageIdentity {
                kind: CoverageIdentityKind::UsbPath,
                value: "1-1.2".to_owned(),
                label: "Nollie".to_owned(),
            },
            native: Some(CoverageNativeDevice {
                device_id: "n1".to_owned(),
                driver_id: "nollie".to_owned(),
                state: "active".to_owned(),
            }),
            bridge: Some(CoverageBridgeDevice {
                device_id: "b1".to_owned(),
                state: "disabled".to_owned(),
                output_enabled: false,
                disabled_reason: Some("native driver owns this device (nollie)".to_owned()),
            }),
            unclaimed: false,
            active: CoverageActive::Native,
        };
        let value = serde_json::to_value(&row).expect("serialize coverage row");
        assert_eq!(value["identity"]["kind"], "usb_path");
        assert_eq!(value["active"], "native");
        assert_eq!(
            serde_json::from_value::<DeviceCoverageRow>(value).expect("deserialize"),
            row
        );

        for (active, wire) in [
            (CoverageActive::Native, "native"),
            (CoverageActive::Bridge, "bridge"),
            (CoverageActive::None, "none"),
            (CoverageActive::Conflict, "conflict"),
        ] {
            assert_eq!(
                serde_json::to_value(active).expect("serialize"),
                json!(wire)
            );
        }
        for (kind, wire) in [
            (CoverageIdentityKind::Serial, "serial"),
            (CoverageIdentityKind::Smbus, "smbus"),
            (CoverageIdentityKind::UsbPath, "usb_path"),
            (CoverageIdentityKind::Device, "device"),
        ] {
            assert_eq!(serde_json::to_value(kind).expect("serialize"), json!(wire));
        }

        let list = DeviceCoverageListResponse {
            items: vec![row],
            total: 1,
            page: None,
        };
        let value = serde_json::to_value(&list).expect("serialize list");
        assert_eq!(value["items"][0]["bridge"]["output_enabled"], false);
    }

    #[test]
    fn bridge_summary_round_trips_and_defaults_optional_fields() {
        let summary = BridgeDeviceSummary {
            endpoint: Some("127.0.0.1:6742".to_owned()),
            controller_index: Some(3),
            identity_confidence: Some("stable".to_owned()),
            detector_class: Some("ENE DRAM".to_owned()),
            output_enabled: false,
            disabled_reason: Some("native driver owns this device (asus)".to_owned()),
            protocol_version: Some(5),
            fingerprint: Some("bridge:openrgb:127.0.0.1:6742:serial:0994FA72AB3CAE43".to_owned()),
        };
        let value = serde_json::to_value(&summary).expect("serialize bridge summary");
        assert_eq!(value["controller_index"], 3);
        assert_eq!(
            serde_json::from_value::<BridgeDeviceSummary>(value).expect("deserialize"),
            summary
        );

        let minimal: BridgeDeviceSummary =
            serde_json::from_value(json!({ "output_enabled": true })).expect("defaults");
        assert!(minimal.output_enabled);
        assert_eq!(minimal.endpoint, None);
        assert_eq!(minimal.protocol_version, None);
        assert_eq!(minimal.fingerprint, None);
    }
}
