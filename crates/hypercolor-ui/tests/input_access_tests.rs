use hypercolor_ui::api::{InputStatus, SystemStatus};
use hypercolor_ui::input_access::{InputAccessRemedy, input_access_remedy};

fn input(enabled: bool, opened: usize, denied: usize) -> InputStatus {
    InputStatus {
        enabled,
        host_capture_registered: true,
        host_capturing: enabled && opened > 0,
        devices_opened: opened,
        devices_denied: denied,
        backends: vec!["evdev".to_owned(), "browser".to_owned()],
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
