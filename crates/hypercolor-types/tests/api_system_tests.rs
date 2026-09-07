use hypercolor_types::api::system::{
    AudioDeviceInfo, AudioDevicesResponse, InputSourceStatus, MacosCapabilityOwner,
    MacosDaemonOwnershipStatus, ServerInfo, SystemResource, SystemStatus,
};

#[test]
fn system_resource_round_trips_one_typed_wire_contract() {
    let resource = SystemResource {
        identity: ServerInfo {
            instance_id: "daemon-1".to_owned(),
            instance_name: "studio".to_owned(),
            version: "0.3.2".to_owned(),
            server_session_id: Some("session-1".to_owned()),
            device_count: 3,
            auth_required: true,
        },
        status: Some(SystemStatus {
            macos_daemon_ownership: Some(MacosDaemonOwnershipStatus {
                active_owner: MacosCapabilityOwner::AppSidecar,
                owner_epoch: 17,
                ..MacosDaemonOwnershipStatus::default()
            }),
            ..SystemStatus::default()
        }),
    };

    let wire = serde_json::to_value(&resource).expect("system resource should serialize");
    assert_eq!(wire["identity"]["instance_name"], "studio");
    assert!(wire["identity"].get("identity").is_none());
    assert_eq!(
        wire["status"]["macos_daemon_ownership"]["active_owner"],
        "app_sidecar"
    );

    let decoded: SystemResource =
        serde_json::from_value(wire).expect("system resource should deserialize");
    assert_eq!(decoded, resource);
}

#[test]
fn input_source_status_requires_the_canonical_snapshot() {
    let error = serde_json::from_value::<InputSourceStatus>(serde_json::json!({
        "source_id": "host-input",
        "kind": "interaction",
        "state": "live"
    }))
    .expect_err("partial operational snapshots must not preserve old wire shapes");

    assert!(error.to_string().contains("missing field"));
}

#[test]
fn audio_device_inventory_round_trips() {
    let response = AudioDevicesResponse {
        devices: vec![AudioDeviceInfo {
            id: "default".to_owned(),
            name: "System Default".to_owned(),
            description: "Follow the operating system default".to_owned(),
        }],
        current: "default".to_owned(),
    };

    let wire = serde_json::to_string(&response).expect("audio inventory should serialize");
    let decoded: AudioDevicesResponse =
        serde_json::from_str(&wire).expect("audio inventory should deserialize");
    assert_eq!(decoded, response);
}
