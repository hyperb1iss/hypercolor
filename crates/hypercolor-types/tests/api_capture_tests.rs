use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CaptureMonitor, CapturePickerResponse, ProtectedSourceGrantOwner,
};

#[test]
fn capture_action_contracts_serialize_stable_owner_names() {
    let authorization = CaptureAuthorizationResponse {
        authorized: true,
        grant_owner: ProtectedSourceGrantOwner::AppSidecar,
    };
    let picker = CapturePickerResponse {
        picking: true,
        grant_owner: ProtectedSourceGrantOwner::PlatformBackend,
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
