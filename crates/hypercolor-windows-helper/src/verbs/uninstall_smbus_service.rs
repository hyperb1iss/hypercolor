//! `uninstall-smbus-service` verb.
//!
//! Stops + deletes the `HypercolorSmBus` Windows service so a clean
//! Hypercolor uninstall doesn't leave an orphaned LocalSystem service
//! registered. Called by the NSIS uninstaller (Phase 1.4) and by the
//! `uninstall-hardware-support` composite (Phase 1.4) which also
//! removes PawnIO modules.

use std::time::{Duration, Instant};

use tracing::info;
use windows_service::service::{ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::VerbError;

const SERVICE_NAME: &str = "HypercolorSmBus";

/// Wait budget for the service to stop before we hard-delete. Long-running
/// SMBus transactions can hold the broker for a few seconds; 15s matches
/// the repair verb's tolerance.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub fn run() -> Result<(), VerbError> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|err| VerbError::ServiceManager {
            detail: err.to_string(),
        })?;

    // The service may already be gone (re-running uninstall). Treat
    // "service not found" as success since the end state matches.
    let service = match manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        Ok(service) => service,
        Err(err) => {
            // windows-service 0.8 wraps ERROR_SERVICE_DOES_NOT_EXIST in
            // its own error type; the underlying io::Error has kind
            // NotFound, but rather than depend on the internal layout we
            // match by string fragment — the message includes the
            // service name verbatim.
            let message = err.to_string();
            if message.contains("does not exist") || message.contains("ERROR_SERVICE_DOES_NOT_EXIST")
            {
                info!("uninstall: service already absent");
                return Ok(());
            }
            return Err(VerbError::ServiceOpen {
                service: SERVICE_NAME.to_owned(),
                detail: message,
            });
        }
    };

    let initial = service
        .query_status()
        .map_err(|err| VerbError::ServiceQuery {
            service: SERVICE_NAME.to_owned(),
            detail: err.to_string(),
        })?;
    info!(state = ?initial.current_state, "uninstall: initial service state");

    if initial.current_state != ServiceState::Stopped {
        // Stop pending is fine — we just wait it out. Other live states
        // need an explicit stop.
        if initial.current_state != ServiceState::StopPending {
            service.stop().map_err(|err| VerbError::ServiceStop {
                service: SERVICE_NAME.to_owned(),
                detail: err.to_string(),
            })?;
        }
        wait_for_stopped(&service)?;
    }

    service.delete().map_err(|err| VerbError::ServiceDelete {
        service: SERVICE_NAME.to_owned(),
        detail: err.to_string(),
    })?;
    info!("uninstall: service deleted");
    Ok(())
}

fn wait_for_stopped(service: &windows_service::service::Service) -> Result<(), VerbError> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        let status = service
            .query_status()
            .map_err(|err| VerbError::ServiceQuery {
                service: SERVICE_NAME.to_owned(),
                detail: err.to_string(),
            })?;
        if status.current_state == ServiceState::Stopped {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(VerbError::ServiceTimeout {
                service: SERVICE_NAME.to_owned(),
                expected: format!("{:?}", ServiceState::Stopped),
                observed: format!("{:?}", status.current_state),
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
