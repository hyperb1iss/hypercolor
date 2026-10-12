//! The `HypercolorSmBus` broker's health in neutral vocabulary.

use hypercolor_types::host_software::{HardwareBrokerReport, HardwareBrokerState};
use hypercolor_windows_pawnio::{
    BROKER_SERVICE_NAME, BrokerServiceState, broker_service_status, describe_broker_exit,
};

const REMEDY_START: &str = "Start it from an elevated PowerShell with \
    `Start-Service HypercolorSmBus`, or open Settings > Device Discovery > Windows SMBus \
    Support in the Hypercolor app and choose Install support.";
const REMEDY_RESTART: &str = "Restart it from an elevated PowerShell with \
    `Restart-Service HypercolorSmBus`; if it still does not answer, its log says what it is \
    stuck on.";
const REMEDY_INSTALL: &str = "Open Settings > Device Discovery > Windows SMBus Support in \
    the Hypercolor app and choose Install support, or rerun the Hypercolor installer.";

/// Report the `HypercolorSmBus` broker: registered or not, running or
/// stopped and why, whether it answers, and where its log is.
///
/// Queries the Service Control Manager and, when the broker runs, pings
/// its pipe. Both block, so async callers should run this on a blocking
/// thread.
#[must_use]
pub fn hardware_broker_report() -> Option<HardwareBrokerReport> {
    let status = match broker_service_status() {
        Ok(status) => status,
        Err(error) => {
            return Some(HardwareBrokerReport {
                service_name: BROKER_SERVICE_NAME.to_owned(),
                state: HardwareBrokerState::Unknown {
                    detail: error.to_string(),
                },
                log_path: None,
                remedy: REMEDY_START.to_owned(),
            });
        }
    };

    let (state, remedy) = if status.direct_mode {
        (HardwareBrokerState::NotNeeded, "")
    } else {
        match status.state {
            BrokerServiceState::NotInstalled => (HardwareBrokerState::NotInstalled, REMEDY_INSTALL),
            BrokerServiceState::Running => {
                let answering = status.answering.unwrap_or(true);
                (
                    HardwareBrokerState::Running { answering },
                    if answering { "" } else { REMEDY_RESTART },
                )
            }
            BrokerServiceState::StartPending => (transitioning("starting"), REMEDY_START),
            BrokerServiceState::StopPending => (transitioning("stopping"), REMEDY_START),
            BrokerServiceState::Paused => (transitioning("paused"), REMEDY_START),
            BrokerServiceState::Stopped => (
                HardwareBrokerState::Stopped {
                    exit: describe_broker_exit(
                        status.win32_exit_code,
                        status.service_specific_exit_code,
                    ),
                    starts_at_boot: status.starts_at_boot,
                },
                REMEDY_START,
            ),
        }
    };

    Some(HardwareBrokerReport {
        service_name: status.service_name.to_owned(),
        state,
        log_path: status.log_path.map(|path| path.display().to_string()),
        remedy: remedy.to_owned(),
    })
}

fn transitioning(state: &str) -> HardwareBrokerState {
    HardwareBrokerState::Transitioning {
        state: state.to_owned(),
    }
}
