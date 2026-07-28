use hypercolor_ui::api::{InputSourceIssueStatus, InputSourceStatus, InputStatus, SystemStatus};
use hypercolor_ui::input_access::{
    InputAccessRemedy, InputPipelineState, input_access_remedy, input_pipeline_state,
    input_status_epoch, input_status_remediation, primary_input_source_issue,
};
use hypercolor_ui::ws::messages::extract_input_source_status_event_hint;

fn input(enabled: bool, opened: usize, denied: usize) -> InputStatus {
    InputStatus {
        enabled,
        host_capture_registered: true,
        host_capturing: enabled && opened > 0,
        devices_opened: opened,
        devices_denied: denied,
        degraded: None,
        backends: vec!["evdev".to_owned(), "browser".to_owned()],
        ..InputStatus::default()
    }
}

#[test]
fn interactive_with_consent_off_wants_enable() {
    assert_eq!(
        input_access_remedy(true, &input(false, 0, 0)),
        Some(InputAccessRemedy::EnableConsent)
    );
}

#[test]
fn consent_gate_wins_over_denied_devices() {
    assert_eq!(
        input_access_remedy(true, &input(false, 0, 2)),
        Some(InputAccessRemedy::EnableConsent)
    );
}

#[test]
fn interactive_enabled_all_devices_denied_wants_rules() {
    assert_eq!(
        input_access_remedy(true, &input(true, 0, 2)),
        Some(InputAccessRemedy::InstallRules)
    );
    assert_eq!(
        input_access_remedy(true, &input(true, 0, 1)),
        Some(InputAccessRemedy::InstallRules)
    );
}

#[test]
fn interactive_enabled_devices_opening_shows_nothing() {
    assert_eq!(input_access_remedy(true, &input(true, 3, 0)), None);
    // Partial denials with at least one open node still capture — silent.
    assert_eq!(input_access_remedy(true, &input(true, 1, 2)), None);
}

#[test]
fn interactive_enabled_idle_pipeline_shows_nothing() {
    // Nothing opened, nothing denied: no actionable remediation, and
    // browser-preview injection works regardless.
    assert_eq!(input_access_remedy(true, &input(true, 0, 0)), None);
}

#[test]
fn non_interactive_never_banners() {
    for status in [
        input(false, 0, 0),
        input(false, 0, 2),
        input(true, 0, 2),
        input(true, 3, 0),
        input(true, 0, 0),
    ] {
        assert_eq!(input_access_remedy(false, &status), None);
    }
}

#[test]
fn input_status_deserializes_frozen_contract() {
    let status: InputStatus = serde_json::from_value(serde_json::json!({
        "enabled": false,
        "host_capture_registered": true,
        "host_capturing": false,
        "devices_opened": 0,
        "devices_denied": 2,
        "backends": ["evdev", "browser"]
    }))
    .expect("frozen input payload should deserialize");

    assert!(!status.enabled);
    assert!(status.host_capture_registered);
    assert_eq!(status.devices_opened, 0);
    assert_eq!(status.devices_denied, 2);
    assert_eq!(status.backends, vec!["evdev", "browser"]);
}

#[test]
fn input_status_deserializes_source_health_snapshot() {
    let status: InputStatus = serde_json::from_value(serde_json::json!({
        "enabled": true,
        "source_graph_generation": 12,
        "sources": [{
            "source_id": "host-interaction",
            "kind": "interaction",
            "backend": "raw_input",
            "configured": true,
            "consented": true,
            "demanded": true,
            "state": "failed",
            "freshness": "not_applicable",
            "source_graph_generation": 12,
            "session_generation": 3,
            "resource_count": 0,
            "denied_resource_count": 0,
            "lifecycle_issue": {
                "code": "worker_exited",
                "message": "worker exited unexpectedly",
                "retryable": true
            },
            "retired": false
        }]
    }))
    .expect("current input status payload should deserialize");

    assert_eq!(status.source_graph_generation, 12);
    assert_eq!(status.sources[0].session_generation, 3);
    assert_eq!(
        status.sources[0]
            .lifecycle_issue
            .as_ref()
            .map(|issue| issue.code.as_str()),
        Some("worker_exited")
    );
}

#[test]
fn input_source_status_event_decodes_as_a_refetch_hint() {
    let hint = extract_input_source_status_event_hint(&serde_json::json!({
        "source_id": "host-interaction",
        "kind": "interaction",
        "backend": "raw_input",
        "state": "failed",
        "freshness": "not_applicable",
        "source_graph_generation": 12,
        "session_generation": 3,
        "lifecycle_issue_code": "worker_exited"
    }))
    .expect("status event should decode");

    assert_eq!(hint.source_id, "host-interaction");
    assert_eq!(hint.state, "failed");
    assert_eq!(hint.session_generation, 3);
    assert_eq!(hint.lifecycle_issue_code.as_deref(), Some("worker_exited"));
}

#[test]
fn system_status_tolerates_missing_input_object() {
    let status: SystemStatus = serde_json::from_value(serde_json::json!({
        "running": true,
        "version": "0.1.0",
        "uptime_seconds": 5,
        "device_count": 1,
        "effect_count": 10,
        "active_effect": "Keystrike",
        "active_scene": null,
        "global_brightness": 100
    }))
    .expect("status without input should still parse");

    assert_eq!(status.input, InputStatus::default());
    assert!(!status.input.enabled);
}

#[test]
fn udev_command_is_the_documented_remediation() {
    assert_eq!(
        hypercolor_ui::components::input_access_banner::UDEV_INSTALL_COMMAND,
        "sudo just udev-install"
    );
}

#[test]
fn no_interactive_session_asks_for_a_user_session_not_udev_rules() {
    // A Windows daemon running as a service has zero denied nodes and zero
    // opened ones, which the counter heuristic alone reads as healthy-but-idle.
    // Offering `sudo just udev-install` to a Windows user would be worse than
    // saying nothing.
    let mut status = input(true, 0, 0);
    status.degraded = Some("no_interactive_session".to_owned());
    assert_eq!(
        input_access_remedy(true, &status),
        Some(InputAccessRemedy::RunInUserSession)
    );
}

#[test]
fn consent_still_outranks_a_session_failure() {
    // Until the user has opted in, a backend that could not have captured
    // anything anyway is not worth a banner about.
    let mut status = input(false, 0, 0);
    status.degraded = Some("no_interactive_session".to_owned());
    assert_eq!(
        input_access_remedy(true, &status),
        Some(InputAccessRemedy::EnableConsent)
    );
}

#[test]
fn a_non_interactive_effect_never_banners_about_the_session() {
    let mut status = input(true, 0, 0);
    status.degraded = Some("no_interactive_session".to_owned());
    assert_eq!(input_access_remedy(false, &status), None);
}

#[test]
fn an_unrecognized_degradation_code_does_not_invent_a_remedy() {
    let mut status = input(true, 2, 0);
    status.degraded = Some("something_new".to_owned());
    assert_eq!(input_access_remedy(true, &status), None);
}

#[test]
fn pipeline_state_distinguishes_consent_idle_live_and_unavailable() {
    assert_eq!(
        input_pipeline_state(&input(false, 0, 0)),
        InputPipelineState::ConsentOff
    );

    let ready = input(true, 0, 0);
    assert_eq!(input_pipeline_state(&ready), InputPipelineState::Ready);

    let live = input(true, 1, 0);
    assert_eq!(input_pipeline_state(&live), InputPipelineState::Live);

    let mut unavailable = input(true, 0, 0);
    unavailable.host_capture_registered = false;
    assert_eq!(
        input_pipeline_state(&unavailable),
        InputPipelineState::Unavailable
    );
}

#[test]
fn worker_failure_and_stale_demand_are_degraded_until_recovery() {
    let mut status = input(true, 1, 0);
    status.sources = vec![InputSourceStatus {
        source_id: "host-interaction".to_owned(),
        configured: true,
        consented: true,
        demanded: true,
        state: "failed".to_owned(),
        freshness: "not_applicable".to_owned(),
        lifecycle_issue: Some(InputSourceIssueStatus {
            code: "worker_exited".to_owned(),
            message: "worker exited unexpectedly".to_owned(),
            remediation: Some("Restart the input backend.".to_owned()),
            retryable: true,
        }),
        ..InputSourceStatus::default()
    }];
    assert_eq!(input_pipeline_state(&status), InputPipelineState::Degraded);
    assert_eq!(
        input_status_remediation(&status).as_deref(),
        Some("Restart the input backend.")
    );

    status.sources[0].state = "live".to_owned();
    status.sources[0].freshness = "stale".to_owned();
    status.sources[0].lifecycle_issue = None;
    assert_eq!(input_pipeline_state(&status), InputPipelineState::Degraded);

    status.sources[0].freshness = "fresh".to_owned();
    assert_eq!(input_pipeline_state(&status), InputPipelineState::Live);
}

#[test]
fn platform_remediation_stays_specific() {
    let linux = input(true, 0, 2);
    assert!(input_status_remediation(&linux).is_some_and(|message| message.contains("udev")));

    let mut windows = input(true, 0, 0);
    windows.degraded = Some("no_interactive_session".to_owned());
    let remediation = input_status_remediation(&windows).expect("Windows remedy");
    assert!(remediation.contains("Windows session"));
    assert!(!remediation.contains("udev"));
}

#[test]
fn disabled_source_failures_do_not_degrade_the_active_pipeline() {
    let mut status = input(true, 0, 0);
    status.sources = vec![InputSourceStatus {
        source_id: "disabled-source".to_owned(),
        state: "failed".to_owned(),
        issue: Some(InputSourceIssueStatus {
            code: "worker_exited".to_owned(),
            message: "disabled worker exited".to_owned(),
            remediation: Some("Restart the disabled worker.".to_owned()),
            retryable: true,
        }),
        ..InputSourceStatus::default()
    }];

    assert_eq!(input_pipeline_state(&status), InputPipelineState::Ready);
    assert_eq!(input_status_remediation(&status), None);
}

#[test]
fn permanently_live_browser_source_does_not_claim_host_capture() {
    let mut status = input(true, 0, 0);
    status.sources = vec![InputSourceStatus {
        source_id: "browser".to_owned(),
        kind: "interaction".to_owned(),
        backend: "browser".to_owned(),
        configured: true,
        consented: true,
        demanded: true,
        state: "live".to_owned(),
        freshness: "not_applicable".to_owned(),
        ..InputSourceStatus::default()
    }];

    assert_eq!(input_pipeline_state(&status), InputPipelineState::Ready);

    status.host_capture_registered = false;
    assert_eq!(
        input_pipeline_state(&status),
        InputPipelineState::Unavailable
    );
}

#[test]
fn terminal_worker_failure_takes_precedence_over_stale_freshness() {
    let lifecycle = InputSourceIssueStatus {
        code: "capture_worker_exited".to_owned(),
        message: "worker exited".to_owned(),
        ..InputSourceIssueStatus::default()
    };
    let stale = InputSourceIssueStatus {
        code: "stale_data".to_owned(),
        message: "data is stale".to_owned(),
        ..InputSourceIssueStatus::default()
    };
    let mut source = InputSourceStatus {
        state: "failed".to_owned(),
        issue: Some(stale.clone()),
        lifecycle_issue: Some(lifecycle.clone()),
        freshness_issue: Some(stale.clone()),
        ..InputSourceStatus::default()
    };

    assert_eq!(primary_input_source_issue(&source), Some(&lifecycle));

    source.state = "live".to_owned();
    assert_eq!(primary_input_source_issue(&source), Some(&stale));
}

#[test]
fn reconnect_and_route_changes_advance_the_status_epoch() {
    let mut config = hypercolor_types::config::HypercolorConfig::default();
    let initial = input_status_epoch(1, None, Some(&config)).expect("initial epoch");

    let reconnect = input_status_epoch(2, None, Some(&config)).expect("reconnect epoch");
    assert_ne!(initial, reconnect);

    config.input.preview_route = hypercolor_types::config::InteractionRoutePolicy::Merge;
    let rerouted = input_status_epoch(2, None, Some(&config)).expect("rerouted epoch");
    assert_ne!(reconnect, rerouted);
}
