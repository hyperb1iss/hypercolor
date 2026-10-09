use hypercolor_types::api::system::{
    AudioDeviceInfo, AudioDevicesResponse, DaemonStartupPhase, DaemonStartupProgress,
    HEALTH_STATUS_STARTING, HealthChecks, HealthResponse, InputSourceStatus, MacosCapabilityOwner,
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

#[test]
fn ready_health_omits_the_startup_report() {
    let health = HealthResponse {
        status: "healthy".to_owned(),
        version: "0.6.2".to_owned(),
        uptime_seconds: 4,
        checks: HealthChecks {
            render_loop: "ok".to_owned(),
            device_backends: "ok".to_owned(),
            event_bus: "idle".to_owned(),
        },
        startup: None,
    };

    let wire = serde_json::to_value(&health).expect("health should serialize");
    assert!(wire.get("startup").is_none());

    // A body from a daemon that predates the startup report still decodes.
    let decoded: HealthResponse = serde_json::from_value(wire).expect("health should decode");
    assert_eq!(decoded, health);
}

#[test]
fn starting_health_carries_the_phase_and_sequence() {
    let wire = serde_json::json!({
        "status": HEALTH_STATUS_STARTING,
        "version": "0.6.2",
        "uptime_seconds": 3,
        "checks": {
            "render_loop": "starting",
            "device_backends": "starting",
            "event_bus": "starting",
        },
        "startup": {
            "phase": "starting_render_thread",
            "sequence": 7,
            "detail": "SparkleFlinger GPU compose pipeline",
        },
    });

    let decoded: HealthResponse = serde_json::from_value(wire.clone()).expect("health decodes");
    assert_eq!(
        decoded.startup,
        Some(DaemonStartupProgress {
            phase: DaemonStartupPhase::StartingRenderThread,
            sequence: 7,
            detail: Some("SparkleFlinger GPU compose pipeline".to_owned()),
        })
    );
    assert_eq!(serde_json::to_value(&decoded).expect("re-encodes"), wire);
}

#[test]
fn a_startup_report_without_a_running_step_omits_the_detail() {
    let progress = DaemonStartupProgress {
        phase: DaemonStartupPhase::LoadingStores,
        sequence: 3,
        detail: None,
    };
    let wire = serde_json::to_value(&progress).expect("progress encodes");
    assert!(wire.get("detail").is_none());
    assert_eq!(
        serde_json::from_value::<DaemonStartupProgress>(wire).expect("progress decodes"),
        progress
    );
}

#[test]
fn startup_phase_wire_names_match_display() {
    for phase in [
        DaemonStartupPhase::Initializing,
        DaemonStartupPhase::ProbingGpu,
        DaemonStartupPhase::ScanningEffects,
        DaemonStartupPhase::LoadingStores,
        DaemonStartupPhase::RegisteringBackends,
        DaemonStartupPhase::StartingInputs,
        DaemonStartupPhase::StartingRenderThread,
        DaemonStartupPhase::StartingServices,
        DaemonStartupPhase::PreparingApi,
    ] {
        assert_eq!(
            serde_json::to_value(phase).expect("phase serializes"),
            serde_json::Value::String(phase.to_string())
        );
    }
}

#[test]
fn an_unknown_startup_phase_from_a_newer_daemon_still_decodes() {
    let progress: DaemonStartupProgress =
        serde_json::from_value(serde_json::json!({ "phase": "warming_caches", "sequence": 2 }))
            .expect("unknown phases decode");
    assert_eq!(progress.phase, DaemonStartupPhase::Unknown);
    assert_eq!(progress.sequence, 2);
}
