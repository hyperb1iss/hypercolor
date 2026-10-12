//! The `smbus_broker` diagnose check: is the privileged broker that reaches
//! `SMBus` controllers and CPU sensor registers for the daemon registered,
//! running, and answering, and if not, what brings it back.
//!
//! The daemon opens no hardware bus itself on platforms that need a
//! broker, so a broker that is installed but stopped darkens motherboard
//! and RAM lighting and CPU temperature with no error anywhere else. This
//! check names that state, the broker's own account of its last exit, the
//! remedy, and where the broker's log lives. The platform reading comes in
//! through `crate::session::hardware_broker_report`, so this module stays
//! free of platform branches; a host with no broker reports no check.

use std::time::Duration;

use hypercolor_types::api::diagnose::DiagnoseCheck;
use hypercolor_types::host_software::{HardwareBrokerReport, HardwareBrokerState};
use tracing::warn;

/// How long a diagnostics report waits on the service manager and the
/// broker's channel before it reports the broker as unknown.
pub const BROKER_CHECK_BUDGET: Duration = Duration::from_secs(3);

const CHECK_CATEGORY: &str = "devices";
const CHECK_NAME: &str = "smbus_broker";
const WHAT_DEPENDS_ON_IT: &str = "motherboard and RAM lighting over SMBus and CPU temperature";

/// Read the host's broker within [`BROKER_CHECK_BUDGET`] and turn it into a
/// check. `None` when the host has no broker to check.
pub async fn smbus_broker_check() -> Option<DiagnoseCheck> {
    let probe = tokio::task::spawn_blocking(crate::session::hardware_broker_report);
    match tokio::time::timeout(BROKER_CHECK_BUDGET, probe).await {
        Ok(Ok(report)) => report.as_ref().map(hardware_broker_check),
        Ok(Err(error)) => {
            warn!(%error, "SMBus broker probe did not finish");
            Some(unknown_check("the broker probe did not finish"))
        }
        Err(_) => Some(unknown_check(&format!(
            "the service manager did not answer within {} seconds",
            BROKER_CHECK_BUDGET.as_secs()
        ))),
    }
}

/// Turn one broker reading into a diagnose check.
///
/// A registered broker that is not running fails the check: that is the
/// state where hardware silently goes dark. A missing broker, one that is
/// starting or stopping, and one that runs but does not answer are
/// warnings.
#[must_use]
pub fn hardware_broker_check(report: &HardwareBrokerReport) -> DiagnoseCheck {
    let name = &report.service_name;
    let log = report
        .log_path
        .as_deref()
        .map(|path| format!(" Broker log: {path}"))
        .unwrap_or_default();
    let (status, detail) = match &report.state {
        HardwareBrokerState::NotNeeded => (
            "pass",
            format!("{name} is not needed: this daemon opens SMBus and CPU sensors directly"),
        ),
        HardwareBrokerState::Running { answering: true } => {
            ("pass", format!("{name} is running and answering"))
        }
        HardwareBrokerState::Running { answering: false } => (
            "warning",
            format!(
                "{name} is running but did not answer on its pipe, so {WHAT_DEPENDS_ON_IT} may \
                 fail. {}{log}",
                report.remedy
            ),
        ),
        HardwareBrokerState::NotInstalled => (
            "warning",
            format!(
                "{name} is not installed, so {WHAT_DEPENDS_ON_IT} are unavailable. {}",
                report.remedy
            ),
        ),
        HardwareBrokerState::Transitioning { state } => (
            "warning",
            format!("{name} is {state}; check again in a few seconds.{log}"),
        ),
        HardwareBrokerState::Stopped {
            exit,
            starts_at_boot,
        } => {
            let boot = match starts_at_boot {
                Some(false) => " It is not set to start with Windows.",
                _ => "",
            };
            (
                "fail",
                format!(
                    "{name} is installed but not running: {exit}. Until it runs, \
                     {WHAT_DEPENDS_ON_IT} are unavailable.{boot} {}{log}",
                    report.remedy
                ),
            )
        }
        HardwareBrokerState::Unknown { detail } => (
            "warning",
            format!("could not read {name} from the service manager: {detail}"),
        ),
    };
    DiagnoseCheck {
        category: CHECK_CATEGORY.to_owned(),
        name: CHECK_NAME.to_owned(),
        status: status.to_owned(),
        detail,
    }
}

fn unknown_check(detail: &str) -> DiagnoseCheck {
    DiagnoseCheck {
        category: CHECK_CATEGORY.to_owned(),
        name: CHECK_NAME.to_owned(),
        status: "warning".to_owned(),
        detail: format!("could not read the SMBus broker: {detail}"),
    }
}
