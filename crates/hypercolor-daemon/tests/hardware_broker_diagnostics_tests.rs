//! The `smbus_broker` diagnose check reads one platform broker report and
//! names a registered broker that is not running, with the remedy and the
//! broker's own log.

use hypercolor_daemon::domain::hardware_broker_diagnostics::{
    hardware_broker_check, smbus_broker_check,
};
use hypercolor_types::host_software::{HardwareBrokerReport, HardwareBrokerState};

const REMEDY: &str = "Start it with Start-Service HypercolorSmBus.";
const LOG: &str = r"C:\Program Files\Hypercolor\tools\logs\smbus-broker.log";

fn report(state: HardwareBrokerState) -> HardwareBrokerReport {
    HardwareBrokerReport {
        service_name: "HypercolorSmBus".to_owned(),
        state,
        log_path: Some(LOG.to_owned()),
        remedy: REMEDY.to_owned(),
    }
}

#[test]
fn an_installed_broker_that_is_stopped_fails_with_the_remedy_and_log() {
    let check = hardware_broker_check(&report(HardwareBrokerState::Stopped {
        exit: "it last stopped cleanly (exit code 0)".to_owned(),
        starts_at_boot: Some(true),
    }));

    assert_eq!(check.category, "devices");
    assert_eq!(check.name, "smbus_broker");
    assert_eq!(check.status, "fail");
    for expected in [
        "HypercolorSmBus is installed but not running",
        "exit code 0",
        "motherboard and RAM lighting over SMBus and CPU temperature are unavailable",
        REMEDY,
        LOG,
    ] {
        assert!(
            check.detail.contains(expected),
            "missing {expected:?} in {}",
            check.detail
        );
    }
    assert!(!check.detail.contains("not set to start with Windows"));
}

#[test]
fn a_stopped_broker_that_does_not_start_at_boot_says_so() {
    let check = hardware_broker_check(&report(HardwareBrokerState::Stopped {
        exit: "it has not been started since Windows booted (exit code 1077)".to_owned(),
        starts_at_boot: Some(false),
    }));

    assert_eq!(check.status, "fail");
    assert!(check.detail.contains("not set to start with Windows"));
}

#[test]
fn a_running_broker_that_answers_passes() {
    let check = hardware_broker_check(&report(HardwareBrokerState::Running { answering: true }));

    assert_eq!(check.status, "pass");
    assert_eq!(check.detail, "HypercolorSmBus is running and answering");
}

#[test]
fn a_running_broker_that_does_not_answer_warns_with_the_log() {
    let check = hardware_broker_check(&report(HardwareBrokerState::Running { answering: false }));

    assert_eq!(check.status, "warning");
    assert!(check.detail.contains("did not answer on its pipe"));
    assert!(check.detail.contains(LOG));
}

#[test]
fn a_missing_broker_warns_with_the_install_remedy() {
    let mut missing = report(HardwareBrokerState::NotInstalled);
    missing.log_path = None;
    missing.remedy = "Install support from the app.".to_owned();

    let check = hardware_broker_check(&missing);

    assert_eq!(check.status, "warning");
    assert!(check.detail.contains("HypercolorSmBus is not installed"));
    assert!(check.detail.contains("Install support from the app."));
}

#[test]
fn starting_stopping_and_unknown_states_warn() {
    let starting = hardware_broker_check(&report(HardwareBrokerState::Transitioning {
        state: "starting".to_owned(),
    }));
    let unknown = hardware_broker_check(&report(HardwareBrokerState::Unknown {
        detail: "access is denied".to_owned(),
    }));

    assert_eq!(starting.status, "warning");
    assert!(starting.detail.contains("HypercolorSmBus is starting"));
    assert_eq!(unknown.status, "warning");
    assert!(unknown.detail.contains("access is denied"));
}

#[test]
fn a_daemon_that_opens_buses_directly_needs_no_broker() {
    let check = hardware_broker_check(&report(HardwareBrokerState::NotNeeded));

    assert_eq!(check.status, "pass");
    assert!(check.detail.contains("not needed"));
}

#[tokio::test]
async fn hosts_without_a_broker_report_no_check() {
    let check = smbus_broker_check().await;

    if cfg!(target_os = "windows") {
        let check = check.expect("Windows always reports the SMBus broker");
        assert_eq!(check.name, "smbus_broker");
    } else {
        assert!(check.is_none());
    }
}
