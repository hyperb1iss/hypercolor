use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::service::{
    DaemonRunMode, HOMEBREW_UNIT, LAUNCHD_DIRECT_UNIT, MAX_SERVICE_UNIT_BYTES,
    ProtectedControlCredential, SYSTEMD_UNIT, ServiceConflict, ServiceIdentity,
    ServiceIdentityParseError, ServiceManager, ServiceRecoveryRequired, ServiceStatus,
    StopAuthority, WINDOWS_SCM_UNIT,
};

fn every_identity() -> Vec<ServiceIdentity> {
    vec![
        ServiceIdentity::APP_SIDECAR,
        ServiceIdentity::launchd_direct(),
        ServiceIdentity::homebrew(),
        ServiceIdentity::systemd_user(),
        ServiceIdentity::systemd_system(),
        ServiceIdentity::windows_scm(),
        ServiceIdentity::STANDALONE,
    ]
}

#[test]
fn protected_control_credentials_are_canonical_and_redacted() {
    let credential = ProtectedControlCredential::from_bytes([0xab; 32]);
    let expected = format!("hc_pc_{}", "ab".repeat(32));

    assert_eq!(credential.expose_secret(), expected);
    assert_eq!(
        format!("{credential:?}"),
        "ProtectedControlCredential([REDACTED])"
    );
    assert_eq!(
        ProtectedControlCredential::parse(&expected).expect("credential parses"),
        credential
    );
    assert!(ProtectedControlCredential::parse("hc_pc_AB").is_err());
    assert!(ProtectedControlCredential::parse("hc_ak_invalid").is_err());
}

#[test]
fn generated_protected_control_credentials_are_fresh_and_round_trip() {
    let first = ProtectedControlCredential::generate();
    let second = ProtectedControlCredential::generate();

    assert_ne!(first, second);
    let json = serde_json::to_string(&first).expect("credential serializes");
    let decoded: ProtectedControlCredential =
        serde_json::from_str(&json).expect("credential deserializes");
    assert_eq!(decoded, first);
}

#[test]
fn declarations_round_trip_for_every_identity() {
    for identity in every_identity() {
        let declaration = identity.declaration();
        let parsed = ServiceIdentity::parse_declaration(&declaration)
            .unwrap_or_else(|error| panic!("{declaration}: {error}"));
        assert_eq!(parsed, identity, "{declaration}");
        assert_eq!(identity.to_string(), declaration);
    }
}

#[test]
fn declarations_use_the_documented_wire_forms() {
    assert_eq!(
        ServiceIdentity::APP_SIDECAR.declaration(),
        "supervised_child"
    );
    assert_eq!(ServiceIdentity::STANDALONE.declaration(), "standalone");
    assert_eq!(
        ServiceIdentity::launchd_direct().declaration(),
        format!("user_service:launchd:{LAUNCHD_DIRECT_UNIT}")
    );
    assert_eq!(
        ServiceIdentity::homebrew().declaration(),
        format!("user_service:homebrew:{HOMEBREW_UNIT}")
    );
    assert_eq!(
        ServiceIdentity::systemd_user().declaration(),
        format!("user_service:systemd:{SYSTEMD_UNIT}")
    );
    assert_eq!(
        ServiceIdentity::systemd_system().declaration(),
        format!("system_service:systemd:{SYSTEMD_UNIT}")
    );
    assert_eq!(
        ServiceIdentity::windows_scm().declaration(),
        format!("system_service:windows_scm:{WINDOWS_SCM_UNIT}")
    );
}

#[test]
fn declaration_parser_accepts_manager_without_unit() {
    let parsed = ServiceIdentity::parse_declaration("user_service:systemd")
        .expect("manager without unit parses");
    assert_eq!(parsed.run_mode, DaemonRunMode::UserService);
    assert_eq!(parsed.manager, Some(ServiceManager::Systemd));
    assert_eq!(parsed.unit, None);
}

#[test]
fn declaration_parser_rejects_malformed_input() {
    assert_eq!(
        ServiceIdentity::parse_declaration(""),
        Err(ServiceIdentityParseError::Empty)
    );
    assert_eq!(
        ServiceIdentity::parse_declaration("   "),
        Err(ServiceIdentityParseError::Empty)
    );
    assert!(matches!(
        ServiceIdentity::parse_declaration("launchd"),
        Err(ServiceIdentityParseError::UnknownRunMode(_))
    ));
    assert!(matches!(
        ServiceIdentity::parse_declaration("user_service:upstart:x"),
        Err(ServiceIdentityParseError::UnknownManager(_))
    ));
    assert_eq!(
        ServiceIdentity::parse_declaration("supervised_child:launchd"),
        Err(ServiceIdentityParseError::ManagerOnUnmanagedMode(
            DaemonRunMode::SupervisedChild
        ))
    );
    assert_eq!(
        ServiceIdentity::parse_declaration("standalone:systemd:x"),
        Err(ServiceIdentityParseError::ManagerOnUnmanagedMode(
            DaemonRunMode::Standalone
        ))
    );
    assert_eq!(
        ServiceIdentity::parse_declaration("user_service::unit"),
        Err(ServiceIdentityParseError::UnitWithoutManager)
    );
    assert_eq!(
        ServiceIdentity::parse_declaration("user_service:systemd:a\nb"),
        Err(ServiceIdentityParseError::UnitNotPrintable)
    );
    let oversized = "x".repeat(MAX_SERVICE_UNIT_BYTES + 1);
    assert_eq!(
        ServiceIdentity::parse_declaration(&format!("user_service:systemd:{oversized}")),
        Err(ServiceIdentityParseError::UnitTooLong(
            MAX_SERVICE_UNIT_BYTES + 1
        ))
    );
}

#[test]
fn identity_serializes_snake_case_and_omits_absent_fields() {
    let json = serde_json::to_value(ServiceIdentity::APP_SIDECAR).expect("serialize");
    assert_eq!(json, serde_json::json!({ "run_mode": "supervised_child" }));

    let json = serde_json::to_value(ServiceIdentity::windows_scm()).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "run_mode": "system_service",
            "manager": "windows_scm",
            "unit": "Hypercolor",
        })
    );
    let decoded: ServiceIdentity = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, ServiceIdentity::windows_scm());
}

#[test]
fn status_round_trips_with_conflict_and_recovery() {
    let status = ServiceStatus {
        identity: ServiceIdentity::launchd_direct(),
        owner_epoch: 42,
        conflict: Some(ServiceConflict {
            active: ServiceIdentity::launchd_direct(),
            contender: ServiceIdentity::homebrew(),
            observed_at_ms: 1_777,
        }),
        recovery_required: Some(ServiceRecoveryRequired {
            requested: ServiceIdentity::APP_SIDECAR,
            prior: ServiceIdentity::launchd_direct(),
            phase: "rollback_start_requested".into(),
        }),
    };
    let json = serde_json::to_string(&status).expect("serialize");
    let decoded: ServiceStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, status);

    let minimal: ServiceStatus =
        serde_json::from_str(r#"{"identity":{"run_mode":"standalone"}}"#).expect("minimal");
    assert_eq!(minimal, ServiceStatus::new(ServiceIdentity::STANDALONE, 0));
}

#[test]
fn service_identity_event_uses_the_snake_case_tag() {
    let event = HypercolorEvent::ServiceIdentityChanged {
        identity: ServiceIdentity::homebrew(),
        owner_epoch: 7,
        conflict: None,
        recovery_required: None,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "ServiceIdentityChanged");
    assert_eq!(json["data"]["identity"]["manager"], "homebrew");
    assert_eq!(json["data"]["owner_epoch"], 7);
    let decoded: HypercolorEvent = serde_json::from_value(json).expect("deserialize");
    assert!(matches!(
        decoded,
        HypercolorEvent::ServiceIdentityChanged { owner_epoch: 7, .. }
    ));
}

#[test]
fn stop_authority_follows_run_mode() {
    assert_eq!(
        StopAuthority::for_identity(&ServiceIdentity::APP_SIDECAR),
        StopAuthority::SupervisedChild
    );
    assert_eq!(
        StopAuthority::for_identity(&ServiceIdentity::STANDALONE),
        StopAuthority::UserDirected
    );
    assert_eq!(
        StopAuthority::for_identity(&ServiceIdentity::homebrew()),
        StopAuthority::ServiceManager(ServiceIdentity::homebrew())
    );
    let unmanaged_service = ServiceIdentity {
        run_mode: DaemonRunMode::UserService,
        manager: None,
        unit: None,
    };
    assert_eq!(
        StopAuthority::for_identity(&unmanaged_service),
        StopAuthority::UserDirected
    );
}
