//! Windows Service Control Manager entry point.

use std::ffi::OsString;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use hypercolor_daemon::daemon::{self, DaemonRunOptions};
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

static SERVICE_OPTIONS: OnceLock<DaemonRunOptions> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Run the daemon under the Windows Service Control Manager.
///
/// # Errors
///
/// Returns an error when the service dispatcher cannot attach to SCM.
pub fn run(options: DaemonRunOptions) -> Result<()> {
    SERVICE_OPTIONS
        .set(options)
        .map_err(|_| anyhow!("Windows service options were already initialized"))?;

    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("failed to start Hypercolor Windows service dispatcher")
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        eprintln!("Hypercolor Windows service failed: {error:#}");
    }
}

fn run_service() -> Result<()> {
    let options = SERVICE_OPTIONS
        .get()
        .cloned()
        .context("Windows service options were not initialized")?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                let _ = shutdown_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

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
            | ServiceControlAccept::PRESHUTDOWN,
        0,
        Duration::ZERO,
    )?;

    let run_result = runtime.block_on(daemon::run(options, shutdown_rx));
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
