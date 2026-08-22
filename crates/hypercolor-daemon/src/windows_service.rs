//! Windows Service Control Manager entry point.

use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::daemon::{self, DaemonExtensionInstaller, DaemonRunOptions};
use anyhow::{Context, Result, anyhow};
use hypercolor_windows_session::{ScmSessionEventAdapter, scm_session_monitor};
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;

const SERVICE_NAME: &str = "Hypercolor";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const SERVICE_START_WAIT_HINT: Duration = Duration::from_secs(30);
const SERVICE_STOP_WAIT_HINT: Duration = Duration::from_secs(20);

struct ServiceConfiguration {
    options: Mutex<Option<DaemonRunOptions>>,
    extension_installers: &'static [&'static dyn DaemonExtensionInstaller],
    session_adapter: ScmSessionEventAdapter,
}

static SERVICE_CONFIGURATION: OnceLock<ServiceConfiguration> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Run the daemon under the Windows Service Control Manager.
///
/// # Errors
///
/// Returns an error when the service dispatcher cannot attach to SCM.
pub fn run(
    mut options: DaemonRunOptions,
    extension_installers: &'static [&'static dyn DaemonExtensionInstaller],
) -> Result<()> {
    let (session_adapter, monitor) = scm_session_monitor();
    options.session_monitors = Some(vec![Box::new(monitor)]);
    SERVICE_CONFIGURATION
        .set(ServiceConfiguration {
            options: Mutex::new(Some(options)),
            extension_installers,
            session_adapter,
        })
        .map_err(|_| anyhow!("Windows service configuration was already initialized"))?;

    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("failed to start Hypercolor Windows service dispatcher")
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        eprintln!("Hypercolor Windows service failed: {error:#}");
    }
}

fn run_service() -> Result<()> {
    let configuration = SERVICE_CONFIGURATION
        .get()
        .context("Windows service configuration was not initialized")?;
    let options = configuration
        .options
        .lock()
        .map_err(|_| anyhow!("Windows service options lock was poisoned"))?
        .take()
        .context("Windows service options were already consumed")?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let session_adapter = configuration.session_adapter.clone();

    let event_handler =
        move |control_event| handle_service_control(control_event, &session_adapter, &shutdown_tx);

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .context("failed to register Hypercolor service control handler")?;
    report_status(
        &status_handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
        SERVICE_START_WAIT_HINT,
    )?;

    let runtime = daemon::build_main_runtime()?;
    report_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::PRESHUTDOWN
            | ServiceControlAccept::POWER_EVENT
            | ServiceControlAccept::SESSION_CHANGE,
        0,
        Duration::ZERO,
    )?;

    let run_result = runtime.block_on(daemon::run_with_extensions(
        options,
        shutdown_rx,
        configuration.extension_installers,
    ));
    let exit_code = u32::from(run_result.is_err());
    report_status(
        &status_handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit_code,
        SERVICE_STOP_WAIT_HINT,
    )?;

    run_result
}

fn handle_service_control(
    control_event: ServiceControl,
    session_adapter: &ScmSessionEventAdapter,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> ServiceControlHandlerResult {
    if session_adapter.publish_service_control(&control_event) {
        return ServiceControlHandlerResult::NoError;
    }
    match control_event {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
            let _ = shutdown_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_service::service::{
        PowerEventParam, SessionChangeParam, SessionChangeReason, SessionNotification,
    };

    const NO_ERROR: u32 = 0;
    const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;

    #[test]
    fn recognized_session_controls_are_acknowledged_after_monitor_shutdown() {
        let (adapter, monitor) = scm_session_monitor();
        drop(monitor);
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        let session_lock = ServiceControl::SessionChange(SessionChangeParam {
            reason: SessionChangeReason::SessionLock,
            notification: SessionNotification {
                size: size_of::<SessionNotification>() as u32,
                session_id: 1,
            },
        });

        assert_eq!(
            handle_service_control(
                ServiceControl::PowerEvent(PowerEventParam::Suspend),
                &adapter,
                &shutdown_tx,
            )
            .to_raw(),
            NO_ERROR
        );
        assert_eq!(
            handle_service_control(session_lock, &adapter, &shutdown_tx).to_raw(),
            NO_ERROR
        );
        assert_eq!(
            handle_service_control(ServiceControl::ParamChange, &adapter, &shutdown_tx).to_raw(),
            ERROR_CALL_NOT_IMPLEMENTED
        );
    }
}

fn report_status(
    status_handle: &ServiceStatusHandle,
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: u32,
    wait_hint: Duration,
) -> Result<()> {
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(exit_code),
            checkpoint: 0,
            wait_hint,
            process_id: None,
        })
        .context("failed to update Hypercolor service status")
}
