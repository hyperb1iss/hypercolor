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
