use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CaptureMonitor, CapturePickerResponse, ProtectedSourceGrantOwner,
};

#[test]
fn capture_action_contracts_serialize_stable_owner_names() {
    let authorization = CaptureAuthorizationResponse {
        authorized: true,
        grant_owner: ProtectedSourceGrantOwner::new("app_sidecar"),
    };
    let picker = CapturePickerResponse {
        picking: true,
        grant_owner: ProtectedSourceGrantOwner::new("platform_backend"),
    };

    assert_eq!(
        serde_json::to_value(authorization).expect("authorization should serialize"),
        serde_json::json!({"authorized": true, "grant_owner": "app_sidecar"})
    );
    assert_eq!(
        serde_json::to_value(picker).expect("picker should serialize"),
        serde_json::json!({"picking": true, "grant_owner": "platform_backend"})
    );
}

#[test]
fn capture_action_contract_accepts_future_owner_names() {
    let owner: ProtectedSourceGrantOwner =
        serde_json::from_str("\"future_ui_host\"").expect("future owner should deserialize");

    assert_eq!(owner.as_str(), "future_ui_host");
    assert_eq!(
        serde_json::to_string(&owner).expect("future owner should serialize"),
        "\"future_ui_host\""
    );
}

#[test]
fn capture_monitor_contract_round_trips() {
    let monitor = CaptureMonitor {
        index: 1,
        id: "display:7a3f".to_owned(),
        name: "Studio Display".to_owned(),
        width: 5_120,
        height: 2_880,
        primary: true,
        value: "display:7a3f".to_owned(),
    };
    let value = serde_json::to_value(&monitor).expect("monitor should serialize");
    let decoded: CaptureMonitor =
        serde_json::from_value(value).expect("monitor should deserialize");

    assert_eq!(decoded, monitor);
}
