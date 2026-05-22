//! `repair-smbus-service` verb.
//!
//! Stops + restarts the `HypercolorSmBus` Windows service. Invoked by the
//! daemon's broker watchdog (Phase 1.2) when health probes against the
//! broker pipe time out. Cheapest verb to implement end-to-end because it
//! needs no PawnIO modules and no installer interaction — just SCM
//! manipulation, which we already use elsewhere via `windows-service`.

use std::time::{Duration, Instant};

use tracing::info;
use windows_service::service::{ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::VerbError;

const SERVICE_NAME: &str = "HypercolorSmBus";

/// Maximum time we wait for the service to reach a target state after
/// issuing Stop or Start. Windows can take a few seconds to drain
/// pending I/O before the service transitions.
const STATE_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll interval while waiting for state transitions.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub fn run() -> Result<(), VerbError> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|err| VerbError::ServiceManager {
            detail: err.to_string(),
        })?;

    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START,
        )
        .map_err(|err| VerbError::ServiceOpen {
            service: SERVICE_NAME.to_owned(),
            detail: err.to_string(),
        })?;

    let initial = service
        .query_status()
        .map_err(|err| VerbError::ServiceQuery {
            service: SERVICE_NAME.to_owned(),
            detail: err.to_string(),
        })?;
    info!(state = ?initial.current_state, "repair: initial service state");

    // Stop only if it isn't already stopped or pending-stop. Trying to
    // stop a stopped service returns an error; trying to stop one that's
    // already pending-stop just wastes time.
    match initial.current_state {
        ServiceState::Stopped => {}
        ServiceState::StopPending => {
            wait_for_state(&service, ServiceState::Stopped)?;
        }
        _ => {
            service.stop().map_err(|err| VerbError::ServiceStop {
                service: SERVICE_NAME.to_owned(),
                detail: err.to_string(),
            })?;
            wait_for_state(&service, ServiceState::Stopped)?;
        }
    }

    service
        .start::<&str>(&[])
        .map_err(|err| VerbError::ServiceStart {
            service: SERVICE_NAME.to_owned(),
            detail: err.to_string(),
        })?;
    wait_for_state(&service, ServiceState::Running)?;
    info!("repair: service is running");
    Ok(())
}

fn wait_for_state(
    service: &windows_service::service::Service,
    target: ServiceState,
) -> Result<(), VerbError> {
    let deadline = Instant::now() + STATE_TIMEOUT;
    loop {
        let status = service
            .query_status()
            .map_err(|err| VerbError::ServiceQuery {
                service: SERVICE_NAME.to_owned(),
                detail: err.to_string(),
            })?;
        if status.current_state == target {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(VerbError::ServiceTimeout {
                service: SERVICE_NAME.to_owned(),
                expected: format!("{target:?}"),
                observed: format!("{:?}", status.current_state),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
