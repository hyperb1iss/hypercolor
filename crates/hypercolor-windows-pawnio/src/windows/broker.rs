//! HypercolorSmBus broker service and its client transport.
//!
//! ## Failure model
//!
//! The broker is a thin SMBus-over-PawnIO relay running as
//! `HypercolorSmBus` under SCM. Failure handling is layered so the user
//! never has to think about service health:
//!
//! - **Crashes** are handled by SCM recovery actions configured at
//!   install time in `install-windows-smbus-service.ps1`
//!   (`restart/5000`, `restart/15000`, 60s cooldown, reset after 24h).
//!   Three crash-restart cycles per failure window with no user action.
//! - **Brief unavailability** during SCM restart is absorbed by
//!   [`open_pipe_client`]'s retry loop (`CLIENT_CONNECT_ATTEMPTS` x
//!   `CLIENT_CONNECT_RETRY_DELAY`). Per-frame callers that miss a
//!   render naturally retry on the next frame.
//! - **In-flight broken-pipe errors** propagate as
//!   `PawnIoError::BrokerUnavailable` and translate to `TransportError`
//!   at the HAL boundary. Upstream consumers decide whether to retry or
//!   degrade.
//! - **Every start and stop is on record.** As a service the broker logs
//!   to `logs\smbus-broker.log` beside its executable: the PawnIO runtime
//!   and modules it found, each state it reports, why it stopped (Stop,
//!   system shutdown, or a failure with its exit code), any panic, and
//!   the first request failure of each kind. Failures after the control
//!   handler registers report `SERVICE_STOPPED` with a service-specific
//!   exit code instead of leaving the service in `START_PENDING`.
//!
//! A broker that SCM recovery cannot bring back is still a bug to fix in
//! the broker, but it must never be silent: `hypercolor diagnose` reads
//! [`super::broker_service_status`] and names a registered broker that is
//! not running, with the remedy and the log path.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsString, c_void};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::watch;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use super::service_log::{broker_log_path, open_broker_log};
use super::{
    CpuVendor, IOCTL_READ_MSR, IOCTL_READ_SMN, PAWNIO_MODULE_AMD_FAMILY_17,
    PAWNIO_MODULE_INTEL_MSR, PawnIoError, PawnIoHandle, PawnIoResult, PawnIoRuntime,
    SmBusBatchOperation, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBus,
    WindowsSmBusBusInfo, check_pawnio_status, detect_cpu_vendor, direct_enumerate_smbus_buses,
    direct_open_smbus_bus, module_name_from_wire, resolve_module_path,
};
use crate::PawnIoErrorKind;

pub(super) const SERVICE_NAME: &str = crate::BROKER_SERVICE_NAME;
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
const SERVICE_START_WAIT_HINT: Duration = Duration::from_secs(15);
const SERVICE_STOP_WAIT_HINT: Duration = Duration::from_secs(10);
/// How long a stopping broker waits for in-flight requests before its
/// runtime is torn down regardless. Kept under the stop wait hint.
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const PIPE_NAME: &str = r"\\.\pipe\hypercolor-smbus-v1";
/// Pause between attempts to create the next pipe instance.
const PIPE_CREATE_RETRY_DELAY: Duration = Duration::from_millis(250);
/// Consecutive pipe creation failures (ten seconds of retries) after which
/// the request loop fails and the broker stops with exit code 1. A busy
/// pipe, or a name another process holds, never counts.
const PIPE_CREATE_ATTEMPTS: u32 = 40;
/// How long a connected client has to send its request before the broker
/// drops it and frees the pipe instance.
const CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// `ERROR_PIPE_BUSY`: every instance of the pipe is in use.
const ERROR_PIPE_BUSY: i32 = 231;
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)";
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_BATCH_DELAY_MS: u64 = 1_000;
const CLIENT_CONNECT_ATTEMPTS: usize = 20;
const CLIENT_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(10);
const DIRECT_ENV: &str = "HYPERCOLOR_PAWNIO_DIRECT";

define_windows_service!(ffi_service_main, service_main);

pub(super) fn direct_mode_enabled() -> bool {
    std::env::var_os(DIRECT_ENV).is_some_and(|value| value != "0")
}

pub(super) fn enumerate_smbus_buses() -> PawnIoResult<Vec<WindowsSmBusBusInfo>> {
    let response = send_request(BrokerRequest::EnumerateBuses, "enumerate_buses")?;
    let BrokerResponse::Buses { buses } = response else {
        return Err(unexpected_response("enumerate_buses"));
    };

    buses.into_iter().map(TryInto::try_into).collect()
}

pub(super) fn open_smbus_bus(path: &str) -> PawnIoResult<WindowsSmBusBus> {
    let response = send_request(
        BrokerRequest::OpenBus {
            path: path.to_owned(),
        },
        "open_bus",
    )?;
    let BrokerResponse::Bus { bus } = response else {
        return Err(unexpected_response("open_bus"));
    };

    Ok(WindowsSmBusBus::brokered(bus.try_into()?))
}

pub(super) fn smbus_xfer(
    path: &str,
    address: u8,
    direction: SmBusDirection,
    command: u8,
    transaction: &mut SmBusTransaction,
) -> PawnIoResult<()> {
    let response = send_request(
        BrokerRequest::SmBusXfer {
            path: path.to_owned(),
            address,
            direction: direction.into(),
            command,
            transaction: BrokerSmBusTransaction::from(&*transaction),
        },
        "smbus_xfer",
    )?;
    let BrokerResponse::Transaction {
        transaction: returned,
    } = response
    else {
        return Err(unexpected_response("smbus_xfer"));
    };

    *transaction = returned.try_into()?;
    Ok(())
}

/// Read an MSR via the broker. The broker loads the right PawnIO module
/// (IntelMSR.bin for Intel, AMDFamily17.bin for AMD) lazily on first use
/// and caches the handle for the broker's lifetime.
pub(super) fn read_msr(msr: u32) -> PawnIoResult<u64> {
    let response = send_request(BrokerRequest::ReadMsr { msr }, "read_msr")?;
    let BrokerResponse::MsrValue { value } = response else {
        return Err(unexpected_response("read_msr"));
    };
    Ok(value)
}

/// Read a 32-bit SMN register via the broker (AMD only). Same lazy
/// PawnIO module loading as `read_msr`.
pub(super) fn read_smn(addr: u32) -> PawnIoResult<u32> {
    let response = send_request(BrokerRequest::ReadSmn { addr }, "read_smn")?;
    let BrokerResponse::SmnValue { value } = response else {
        return Err(unexpected_response("read_smn"));
    };
    Ok(value)
}

/// Ask the broker to answer without touching PawnIO or any bus. `Ok`
/// carries the broker's version. A [`PawnIoError::BrokerCall`] means a
/// broker from before this request answered it, which still proves the
/// pipe is served; only [`PawnIoError::BrokerUnavailable`] means nobody is
/// listening.
pub(super) fn ping() -> PawnIoResult<String> {
    let response = send_request(BrokerRequest::Ping, "ping")?;
    let BrokerResponse::Pong { version } = response else {
        return Err(unexpected_response("ping"));
    };
    Ok(version)
}

pub(super) fn smbus_xfer_batch(
    path: &str,
    address: u8,
    operations: &mut [SmBusBatchOperation],
) -> PawnIoResult<()> {
    let broker_operations = operations
        .iter()
        .map(BrokerSmBusBatchOperation::from)
        .collect::<Vec<_>>();
    let response = send_request(
        BrokerRequest::SmBusXferBatch {
            path: path.to_owned(),
            address,
            operations: broker_operations,
        },
        "smbus_xfer_batch",
    )?;
    let BrokerResponse::Batch {
        operations: returned,
    } = response
    else {
        return Err(unexpected_response("smbus_xfer_batch"));
    };
    if returned.len() != operations.len() {
        return Err(PawnIoError::broker_call(
            "smbus_xfer_batch",
            format!(
                "broker returned {} batch operations for {} requests",
                returned.len(),
                operations.len()
            ),
        ));
    }

    for (target, returned) in operations.iter_mut().zip(returned) {
        if let (
            SmBusBatchOperation::Transfer { transaction, .. },
            BrokerSmBusBatchOperation::Transfer {
                transaction: returned,
                ..
            },
        ) = (target, returned)
        {
            *transaction = returned.try_into()?;
        }
    }

    Ok(())
}

pub(super) fn run_smbus_service() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    if args.any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    if std::env::args_os().any(|arg| arg == "--console") {
        init_console_tracing();
        install_panic_logger();
        return run_console();
    }

    let log_path = init_service_tracing();
    install_panic_logger();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        log = ?log_path,
        "Hypercolor SMBus broker process started"
    );

    // StartServiceCtrlDispatcher only succeeds when the Service Control
    // Manager launched this process. Anything else (a double-click, a
    // script running the exe directly) lands here, and the log says so.
    let result = service_dispatcher::start(SERVICE_NAME, ffi_service_main).context(
        "could not connect to the Service Control Manager; the broker runs as the \
         HypercolorSmBus service, or in a terminal with --console",
    );
    match &result {
        Ok(()) => info!("Hypercolor SMBus broker process exiting"),
        Err(error) => {
            error!(error = %format_args!("{error:#}"), "Hypercolor SMBus broker could not start")
        }
    }
    result
}

fn print_help() {
    println!(
        "Hypercolor SMBus broker\n\nUSAGE:\n    hypercolor-smbus-service.exe [--console]\n\nRuns the narrow PawnIO SMBus broker used by the regular Hypercolor daemon.\nAs a service it logs to logs\\smbus-broker.log beside this executable."
    );
}

fn broker_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hypercolor_windows_pawnio=info,info"))
}

fn init_console_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(broker_env_filter())
        .with_target(true)
        .try_init();
}

/// Route tracing to the broker log beside the executable. A service has no
/// console, so without this every message, failures included, is dropped.
/// Falls back to console output when the log cannot be opened, which keeps
/// a misconfigured install from also losing the broker itself.
fn init_service_tracing() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| broker_log_path(&exe))?;
    match open_broker_log(&path) {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(broker_env_filter())
                .with_target(true)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init();
            Some(path)
        }
        Err(_) => {
            init_console_tracing();
            None
        }
    }
}

/// Record a panic in the broker log before the default hook runs. Release
/// builds abort on panic, so this line is the only trace a panic leaves.
fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error!(panic = %info, "Hypercolor SMBus broker panicked");
        previous(info);
    }));
}

fn run_console() -> Result<()> {
    log_pawnio_self_check();
    let runtime = build_runtime()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(None);
    runtime.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(Some(StopReason::ConsoleInterrupt));
        }
    });

    runtime.block_on(serve_until_shutdown(shutdown_rx))
}

/// Why the broker left its serve loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    /// The Service Control Manager sent Stop (Stop-Service, sc.exe stop,
    /// the installer stopping it before replacing files).
    StopRequested,
    /// Windows is shutting down.
    SystemShutdown,
    /// Ctrl+C in `--console` mode.
    ConsoleInterrupt,
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        error!(error = %format_args!("{error:#}"), "Hypercolor SMBus broker service failed");
    }
}

fn run_service() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = watch::channel(None);

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => {
                let _ = shutdown_tx.send(Some(StopReason::StopRequested));
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Shutdown => {
                let _ = shutdown_tx.send(Some(StopReason::SystemShutdown));
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .context("failed to register Hypercolor SMBus broker control handler")?;
    report_status(
        &status_handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        ServiceExitCode::NO_ERROR,
        SERVICE_START_WAIT_HINT,
    )?;
    log_pawnio_self_check();

    // Every failure past this point, short of SetServiceStatus itself
    // failing, reports SERVICE_STOPPED with a service-specific exit code,
    // which the Service Control Manager records as Win32 code 1066 plus that
    // value. Returning without it would strand the service in START_PENDING
    // until the Service Control Manager gives up on it, with nothing
    // recorded about why.
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(error = %format_args!("{error:#}"), "Hypercolor SMBus broker could not start its runtime");
            report_stopped(
                &status_handle,
                ServiceExitCode::ServiceSpecific(crate::BROKER_EXIT_RUNTIME_FAILED),
            );
            return Err(error);
        }
    };
    report_status(
        &status_handle,
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::NO_ERROR,
        Duration::ZERO,
    )?;
    info!(pipe = PIPE_NAME, "Hypercolor SMBus broker is running");

    let run_result = runtime.block_on(serve_until_shutdown(shutdown_rx.clone()));
    let reason = *shutdown_rx.borrow();
    match (&run_result, reason) {
        (Err(error), _) => {
            error!(error = %format_args!("{error:#}"), "Hypercolor SMBus broker serve loop failed");
        }
        (Ok(()), Some(reason)) => info!(?reason, "Hypercolor SMBus broker stopping"),
        (Ok(()), None) => warn!("Hypercolor SMBus broker control handler closed; stopping"),
    }

    // Tear the runtime down before reporting SERVICE_STOPPED: once Stopped
    // is reported the process exits, and the installer treats Stopped as
    // permission to replace the files this process holds open.
    report_status(
        &status_handle,
        ServiceState::StopPending,
        ServiceControlAccept::empty(),
        ServiceExitCode::NO_ERROR,
        SERVICE_STOP_WAIT_HINT,
    )?;
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);

    let exit_code = if run_result.is_err() {
        ServiceExitCode::ServiceSpecific(crate::BROKER_EXIT_SERVE_FAILED)
    } else {
        ServiceExitCode::NO_ERROR
    };
    info!(?exit_code, "Hypercolor SMBus broker stopped");
    report_stopped(&status_handle, exit_code);

    run_result
}

/// Log where PawnIO and its modules resolve from, so the broker log answers
/// "was PawnIO there when the broker started" without a second tool. The
/// broker loads neither here: PawnIO stays lazy until the first request.
fn log_pawnio_self_check() {
    match super::resolve_pawnio_library_path() {
        Ok(path) => info!(path = %path.display(), "PawnIO runtime found"),
        Err(error) => warn!(
            %error,
            searched = ?super::pawnio_install_dirs(),
            "PawnIO runtime not found; SMBus and CPU temperature requests will fail until it is installed"
        ),
    }

    let mut found = Vec::new();
    let mut missing = Vec::new();
    let modules = super::SMBUS_MODULES
        .iter()
        .map(|spec| spec.module_name)
        .chain([PAWNIO_MODULE_INTEL_MSR, PAWNIO_MODULE_AMD_FAMILY_17]);
    for module in modules {
        match resolve_module_path(module) {
            Ok(path) => found.push(path.display().to_string()),
            Err(_) => missing.push(module),
        }
    }
    if missing.is_empty() {
        info!(modules = ?found, "PawnIO modules found");
    } else {
        warn!(
            found = ?found,
            missing = ?missing,
            searched = ?super::pawnio_module_dirs(),
            "PawnIO modules missing; requests that need them will fail"
        );
    }
}

fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hypercolor-smbus-broker")
        .enable_all()
        .build()
        .context("failed to build Hypercolor SMBus broker runtime")
}

fn report_status(
    status_handle: &ServiceStatusHandle,
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: ServiceExitCode,
    wait_hint: Duration,
) -> Result<()> {
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted,
            exit_code,
            checkpoint: 0,
            wait_hint,
            process_id: None,
        })
        .context("failed to update Hypercolor SMBus broker service status")
}

/// Report `SERVICE_STOPPED`, logging rather than propagating a failure:
/// callers are already on their way out with a more useful error.
fn report_stopped(status_handle: &ServiceStatusHandle, exit_code: ServiceExitCode) {
    if let Err(error) = report_status(
        status_handle,
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        exit_code,
        Duration::ZERO,
    ) {
        error!(error = %format_args!("{error:#}"), "failed to report the broker stopped");
    }
}

async fn serve_until_shutdown(mut shutdown: watch::Receiver<Option<StopReason>>) -> Result<()> {
    let state = Arc::new(BrokerState::default());
    info!(pipe = PIPE_NAME, "Hypercolor SMBus broker listening");
    let mut create_failures = 0_u32;
    let mut busy_instances = RepeatGate::default();
    let mut connect_failures = RepeatGate::default();

    while shutdown.borrow().is_none() {
        let server = match create_server_pipe() {
            Ok(server) => {
                create_failures = 0;
                busy_instances.reset();
                server
            }
            Err(error) if pipe_held_elsewhere(&error) => {
                // Every instance is busy, or another process holds the pipe
                // name. That is load or interference, not a broken broker: it
                // clears as clients finish (or hit CLIENT_REQUEST_TIMEOUT) or
                // the other process lets go, so it never counts toward
                // stopping the broker, which any interactive user could
                // otherwise force.
                if busy_instances.first() {
                    warn!(
                        error = %format_args!("{error:#}"),
                        "SMBus broker pipe is busy or held by another process; waiting"
                    );
                }
                tokio::time::sleep(PIPE_CREATE_RETRY_DELAY).await;
                continue;
            }
            Err(error) => {
                create_failures += 1;
                // A pipe that cannot be created for this long is not a
                // hiccup. Fail the loop so the broker stops with exit code 1,
                // where SCM recovery and diagnose see it, instead of
                // reporting Running while no client can reach it.
                if create_failures >= PIPE_CREATE_ATTEMPTS {
                    return Err(error.context(format!(
                        "could not create the SMBus broker pipe {PIPE_CREATE_ATTEMPTS} times in a row"
                    )));
                }
                if create_failures == 1 {
                    warn!(
                        error = %format_args!("{error:#}"),
                        "failed to create SMBus broker pipe; retrying"
                    );
                } else {
                    debug!(error = %format_args!("{error:#}"), "SMBus broker pipe still unavailable");
                }
                tokio::time::sleep(PIPE_CREATE_RETRY_DELAY).await;
                continue;
            }
        };
        tokio::select! {
            connect_result = server.connect() => {
                match connect_result {
                    Ok(()) => {
                        connect_failures.reset();
                        tokio::spawn(handle_connected_client(server, Arc::clone(&state)));
                    }
                    Err(error) => {
                        if connect_failures.first() {
                            warn!(%error, "SMBus broker pipe connection failed; accepting next client");
                        } else {
                            debug!(%error, "SMBus broker pipe connection failed again");
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }

    debug!("Hypercolor SMBus broker serve loop ended");
    Ok(())
}

/// Lets the first of a run of identical failures through at `warn` and
/// sends the rest to `debug`, so a failure that repeats every few
/// milliseconds cannot fill the broker log.
#[derive(Default)]
struct RepeatGate {
    seen: bool,
}

impl RepeatGate {
    /// True for the first failure since the last success.
    fn first(&mut self) -> bool {
        !std::mem::replace(&mut self.seen, true)
    }

    fn reset(&mut self) {
        self.seen = false;
    }
}

async fn handle_connected_client(mut server: NamedPipeServer, state: Arc<BrokerState>) {
    let result = async {
        // Clients write their request as soon as they connect. One that
        // connects and sends nothing would hold a pipe instance forever, and
        // enough of them would leave no instance for anyone else.
        let request = tokio::time::timeout(CLIENT_REQUEST_TIMEOUT, read_frame(&mut server))
            .await
            .map_err(|_| {
                anyhow!(
                    "client sent no request within {} seconds",
                    CLIENT_REQUEST_TIMEOUT.as_secs()
                )
            })??;
        let response = match serde_json::from_slice::<BrokerRequest>(&request) {
            Ok(request) => {
                let operation = request.operation();
                let result = state.handle_request(request);
                if let Err(error) = &result {
                    state.log_failure(operation, error);
                }
                BrokerEnvelope::ok(result)
            }
            Err(error) => BrokerEnvelope::error(PawnIoError::InvalidInput {
                detail: format!("invalid broker request: {error}"),
            }),
        };
        let response_bytes =
            serde_json::to_vec(&response).context("failed to encode SMBus broker response")?;
        write_frame(&mut server, &response_bytes).await
    }
    .await;

    if let Err(error) = result {
        trace!(error = %format_args!("{error:#}"), "SMBus broker client request failed");
    }
}

/// True when pipe creation failed because every instance is busy or because
/// another process created the name with a DACL that refuses the broker.
fn pipe_held_elsewhere(error: &anyhow::Error) -> bool {
    matches!(
        error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error),
        Some(ERROR_PIPE_BUSY | super::ERROR_ACCESS_DENIED)
    )
}

fn create_server_pipe() -> Result<NamedPipeServer> {
    let mut security = PipeSecurity::new().context("failed to build SMBus broker pipe ACL")?;
    let mut options = ServerOptions::new();
    options.reject_remote_clients(true);
    let pipe = unsafe {
        // SAFETY: `PipeSecurity` owns a valid SECURITY_ATTRIBUTES structure for
        // the duration of this CreateNamedPipeW call. Tokio copies the raw
        // pointer into the immediate OS call and does not retain it.
        options.create_with_security_attributes_raw(PIPE_NAME, security.as_mut_ptr())
    }?;
    Ok(pipe)
}

async fn read_frame(stream: &mut NamedPipeServer) -> Result<Vec<u8>> {
    let mut len_bytes = [0_u8; 4];
    stream
        .read_exact(&mut len_bytes)
        .await
        .context("failed to read SMBus broker frame length")?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "SMBus broker frame has {len} bytes, max is {MAX_FRAME_BYTES}"
        ));
    }

    let mut bytes = vec![0_u8; len];
    stream
        .read_exact(&mut bytes)
        .await
        .context("failed to read SMBus broker frame")?;
    Ok(bytes)
}

async fn write_frame(stream: &mut NamedPipeServer, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).context("SMBus broker response exceeds u32 length")?;
    stream
        .write_all(&len.to_le_bytes())
        .await
        .context("failed to write SMBus broker response length")?;
    stream
        .write_all(bytes)
        .await
        .context("failed to write SMBus broker response")
}

fn send_request(request: BrokerRequest, operation: &'static str) -> PawnIoResult<BrokerResponse> {
    let request_bytes = serde_json::to_vec(&request).map_err(|error| {
        PawnIoError::broker_call(operation, format!("failed to encode request: {error}"))
    })?;
    let response_bytes = send_request_bytes(&request_bytes)?;
    let envelope = serde_json::from_slice::<BrokerEnvelope>(&response_bytes).map_err(|error| {
        PawnIoError::broker_call(operation, format!("failed to decode response: {error}"))
    })?;

    envelope.into_result(operation)
}

fn send_request_bytes(request: &[u8]) -> PawnIoResult<Vec<u8>> {
    let mut stream = open_pipe_client()?;
    write_blocking_frame(&mut stream, request)?;
    read_blocking_frame(&mut stream)
}

fn open_pipe_client() -> PawnIoResult<std::fs::File> {
    let mut last_error = None;
    for attempt in 0..CLIENT_CONNECT_ATTEMPTS {
        match OpenOptions::new().read(true).write(true).open(PIPE_NAME) {
            Ok(file) => return Ok(file),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < CLIENT_CONNECT_ATTEMPTS {
                    thread::sleep(CLIENT_CONNECT_RETRY_DELAY);
                }
            }
        }
    }

    Err(PawnIoError::BrokerUnavailable {
        detail: last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "named pipe did not open".to_owned()),
    })
}

fn write_blocking_frame(stream: &mut std::fs::File, bytes: &[u8]) -> PawnIoResult<()> {
    let len = u32::try_from(bytes.len()).map_err(|error| {
        PawnIoError::broker_call(
            "write_frame",
            format!("request frame is too large: {error}"),
        )
    })?;
    stream
        .write_all(&len.to_le_bytes())
        .and_then(|()| stream.write_all(bytes))
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to write request to broker: {error}"),
        })
}

fn read_blocking_frame(stream: &mut std::fs::File) -> PawnIoResult<Vec<u8>> {
    let mut len_bytes = [0_u8; 4];
    stream
        .read_exact(&mut len_bytes)
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to read response length from broker: {error}"),
        })?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(PawnIoError::broker_call(
            "read_frame",
            format!("response frame has {len} bytes, max is {MAX_FRAME_BYTES}"),
        ));
    }

    let mut bytes = vec![0_u8; len];
    stream
        .read_exact(&mut bytes)
        .map_err(|error| PawnIoError::BrokerUnavailable {
            detail: format!("failed to read response from broker: {error}"),
        })?;
    Ok(bytes)
}

#[derive(Default)]
struct BrokerState {
    buses: Mutex<HashMap<String, WindowsSmBusBus>>,
    /// Lazy-loaded PawnIO module for MSR/SMN reads. Loaded on first
    /// CPU-temp request and kept for the broker's lifetime — there's
    /// only ever one CPU vendor per box, so one handle covers all
    /// callers.
    cpu_module: Mutex<Option<CpuTempModule>>,
    /// Failure classes already written to the broker log at `warn`.
    /// Clients retry every frame, so each operation and error kind is
    /// logged prominently once and at `debug` after that.
    logged_failures: Mutex<HashSet<(&'static str, PawnIoErrorKind)>>,
}

impl BrokerState {
    fn log_failure(&self, operation: &'static str, error: &PawnIoError) {
        // A transaction the bus rejected (no device at a probed address, a
        // NACK) is an answer, not a broker problem; discovery probes cause
        // plenty of them.
        if matches!(error, PawnIoError::SmBusIo { .. }) {
            debug!(operation, %error, "SMBus transaction failed");
            return;
        }
        let first = self
            .logged_failures
            .lock()
            .map(|mut logged| logged.insert((operation, error.kind())))
            .unwrap_or(false);
        if first {
            warn!(operation, kind = ?error.kind(), %error, "SMBus broker request failed");
        } else {
            debug!(operation, %error, "SMBus broker request failed");
        }
    }

    fn handle_request(&self, request: BrokerRequest) -> PawnIoResult<BrokerResponse> {
        match request {
            BrokerRequest::Ping => Ok(BrokerResponse::Pong {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }),
            BrokerRequest::EnumerateBuses => Ok(BrokerResponse::Buses {
                buses: direct_enumerate_smbus_buses()?
                    .iter()
                    .map(BrokerBusInfo::from)
                    .collect(),
            }),
            BrokerRequest::OpenBus { path } => Ok(BrokerResponse::Bus {
                bus: BrokerBusInfo::from(&self.open_or_cached_bus(&path)?),
            }),
            BrokerRequest::SmBusXfer {
                path,
                address,
                direction,
                command,
                transaction,
            } => {
                let mut transaction = transaction.try_into()?;
                self.smbus_xfer(&path, address, direction.into(), command, &mut transaction)?;
                Ok(BrokerResponse::Transaction {
                    transaction: BrokerSmBusTransaction::from(&transaction),
                })
            }
            BrokerRequest::SmBusXferBatch {
                path,
                address,
                mut operations,
            } => {
                self.smbus_xfer_batch(&path, address, &mut operations)?;
                Ok(BrokerResponse::Batch { operations })
            }
            BrokerRequest::ReadMsr { msr } => {
                let value = self.read_msr(msr)?;
                Ok(BrokerResponse::MsrValue { value })
            }
            BrokerRequest::ReadSmn { addr } => {
                let value = self.read_smn(addr)?;
                Ok(BrokerResponse::SmnValue { value })
            }
        }
    }

    fn read_msr(&self, msr: u32) -> PawnIoResult<u64> {
        let mut guard = self.cpu_module.lock().map_err(|_| lock_poisoned())?;
        let module = match guard.as_ref() {
            Some(module) => module,
            None => {
                *guard = Some(CpuTempModule::new()?);
                guard.as_ref().expect("module just inserted")
            }
        };
        let input = [u64::from(msr)];
        let mut output = [0_u64; 1];
        let mut returned = 0_usize;
        let status = module.runtime.execute(
            module.handle,
            IOCTL_READ_MSR,
            &input,
            &mut output,
            &mut returned,
        );
        check_pawnio_status("ioctl_read_msr", status)?;
        Ok(output[0])
    }

    fn read_smn(&self, addr: u32) -> PawnIoResult<u32> {
        let mut guard = self.cpu_module.lock().map_err(|_| lock_poisoned())?;
        let module = match guard.as_ref() {
            Some(module) => module,
            None => {
                *guard = Some(CpuTempModule::new()?);
                guard.as_ref().expect("module just inserted")
            }
        };
        if module.vendor != CpuVendor::Amd {
            return Err(PawnIoError::InvalidInput {
                detail: "SMN reads are only valid on AMD CPUs".to_owned(),
            });
        }
        let input = [u64::from(addr)];
        let mut output = [0_u64; 1];
        let mut returned = 0_usize;
        let status = module.runtime.execute(
            module.handle,
            IOCTL_READ_SMN,
            &input,
            &mut output,
            &mut returned,
        );
        check_pawnio_status("ioctl_read_smn", status)?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(output[0] as u32)
    }

    fn open_or_cached_bus(&self, path: &str) -> PawnIoResult<WindowsSmBusBusInfo> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            let bus = direct_open_smbus_bus(path)?;
            debug!(
                bus_path = %bus.info().path,
                name = %bus.info().name,
                "opened SMBus broker bus"
            );
            buses.insert(path.to_owned(), bus);
        }

        buses
            .get(path)
            .map(|bus| bus.info().clone())
            .ok_or_else(|| PawnIoError::InvalidInput {
                detail: format!("SMBus broker bus '{path}' was not cached after open"),
            })
    }

    fn smbus_xfer(
        &self,
        path: &str,
        address: u8,
        direction: SmBusDirection,
        command: u8,
        transaction: &mut SmBusTransaction,
    ) -> PawnIoResult<()> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            buses.insert(path.to_owned(), direct_open_smbus_bus(path)?);
        }
        let bus = buses.get(path).ok_or_else(|| PawnIoError::InvalidInput {
            detail: format!("SMBus broker bus '{path}' was not cached after open"),
        })?;

        bus.smbus_xfer(address, direction, command, transaction)
    }

    fn smbus_xfer_batch(
        &self,
        path: &str,
        address: u8,
        operations: &mut [BrokerSmBusBatchOperation],
    ) -> PawnIoResult<()> {
        let mut buses = self.buses.lock().map_err(|_| lock_poisoned())?;
        if !buses.contains_key(path) {
            buses.insert(path.to_owned(), direct_open_smbus_bus(path)?);
        }
        let bus = buses.get(path).ok_or_else(|| PawnIoError::InvalidInput {
            detail: format!("SMBus broker bus '{path}' was not cached after open"),
        })?;

        for operation in operations {
            match operation {
                BrokerSmBusBatchOperation::Transfer {
                    direction,
                    command,
                    transaction,
                } => {
                    let mut transaction_value = SmBusTransaction::try_from(std::mem::replace(
                        transaction,
                        BrokerSmBusTransaction::Quick,
                    ))?;
                    bus.smbus_xfer(
                        address,
                        (*direction).into(),
                        *command,
                        &mut transaction_value,
                    )?;
                    *transaction = BrokerSmBusTransaction::from(&transaction_value);
                }
                BrokerSmBusBatchOperation::Delay { duration_ms } => {
                    validate_batch_delay_ms(*duration_ms)?;
                    thread::sleep(Duration::from_millis(*duration_ms));
                }
            }
        }

        Ok(())
    }
}

fn validate_batch_delay_ms(duration_ms: u64) -> PawnIoResult<()> {
    if duration_ms > MAX_BATCH_DELAY_MS {
        return Err(PawnIoError::InvalidInput {
            detail: format!("SMBus batch delay exceeds maximum of {MAX_BATCH_DELAY_MS} ms"),
        });
    }

    Ok(())
}

fn lock_poisoned() -> PawnIoError {
    PawnIoError::InvalidInput {
        detail: "SMBus broker state lock poisoned".to_owned(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    /// Liveness probe that touches neither PawnIO nor any bus. Brokers
    /// older than this request answer it with an invalid-request error,
    /// which still proves the pipe is served.
    Ping,
    EnumerateBuses,
    OpenBus {
        path: String,
    },
    SmBusXfer {
        path: String,
        address: u8,
        direction: BrokerSmBusDirection,
        command: u8,
        transaction: BrokerSmBusTransaction,
    },
    SmBusXferBatch {
        path: String,
        address: u8,
        operations: Vec<BrokerSmBusBatchOperation>,
    },
    /// Read a single MSR. Broker selects the right PawnIO module
    /// (IntelMSR / AMDFamily17) for the detected CPU vendor.
    ReadMsr {
        msr: u32,
    },
    /// Read a single 32-bit SMN register. AMD-only; the broker errors
    /// on Intel hosts.
    ReadSmn {
        addr: u32,
    },
}

impl BrokerRequest {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::EnumerateBuses => "enumerate_buses",
            Self::OpenBus { .. } => "open_bus",
            Self::SmBusXfer { .. } => "smbus_xfer",
            Self::SmBusXferBatch { .. } => "smbus_xfer_batch",
            Self::ReadMsr { .. } => "read_msr",
            Self::ReadSmn { .. } => "read_smn",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerResponse {
    Pong {
        version: String,
    },
    Buses {
        buses: Vec<BrokerBusInfo>,
    },
    Bus {
        bus: BrokerBusInfo,
    },
    Transaction {
        transaction: BrokerSmBusTransaction,
    },
    Batch {
        operations: Vec<BrokerSmBusBatchOperation>,
    },
    MsrValue {
        value: u64,
    },
    SmnValue {
        value: u32,
    },
}

/// Broker-side cached PawnIO module for CPU MSR/SMN reads. Loaded once
/// (lazily on the first ReadMsr/ReadSmn request) and reused across the
/// broker's lifetime.
struct CpuTempModule {
    runtime: Arc<PawnIoRuntime>,
    handle: PawnIoHandle,
    vendor: CpuVendor,
}

impl CpuTempModule {
    fn new() -> PawnIoResult<Self> {
        let vendor = detect_cpu_vendor()?;
        let module_name = match vendor {
            CpuVendor::Intel => PAWNIO_MODULE_INTEL_MSR,
            CpuVendor::Amd => PAWNIO_MODULE_AMD_FAMILY_17,
        };
        let module_path = resolve_module_path(module_name)?;
        let runtime = PawnIoRuntime::load()?;
        let handle = runtime.open_loaded_module(&module_path)?;
        debug!(
            vendor = ?vendor,
            module = module_name,
            path = %module_path.display(),
            "broker loaded PawnIO CPU temperature module"
        );
        Ok(Self {
            runtime,
            handle,
            vendor,
        })
    }
}

impl Drop for CpuTempModule {
    fn drop(&mut self) {
        let _ = self.runtime.close(self.handle);
    }
}

// SAFETY: PawnIO handle is opaque kernel state. The broker serializes
// access through `BrokerState::cpu_module` (Mutex), and the loaded
// library stays alive through `runtime` for the handle's lifetime.
unsafe impl Send for CpuTempModule {}
// SAFETY: Same justification as Send — concurrent access is gated by
// the broker's `cpu_module` mutex.
unsafe impl Sync for CpuTempModule {}

#[derive(Debug, Serialize, Deserialize)]
struct BrokerEnvelope {
    ok: bool,
    data: Option<BrokerResponse>,
    error: Option<BrokerError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BrokerError {
    kind: PawnIoErrorKind,
    hresult: Option<u32>,
    detail: String,
}

impl BrokerEnvelope {
    fn ok(result: PawnIoResult<BrokerResponse>) -> Self {
        match result {
            Ok(response) => Self {
                ok: true,
                data: Some(response),
                error: None,
            },
            Err(error) => Self::error(error),
        }
    }

    fn error(error: PawnIoError) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(BrokerError {
                kind: error.kind(),
                hresult: error.hresult(),
                detail: error.to_string(),
            }),
        }
    }

    fn into_result(self, operation: &'static str) -> PawnIoResult<BrokerResponse> {
        if self.ok {
            return self.data.ok_or_else(|| {
                PawnIoError::broker_call(
                    operation,
                    "success response did not include data".to_owned(),
                )
            });
        }

        let error = self.error.unwrap_or_else(|| BrokerError {
            kind: PawnIoErrorKind::Io,
            hresult: None,
            detail: "broker returned failure without an error".to_owned(),
        });
        Err(PawnIoError::BrokerCall {
            operation,
            kind: error.kind,
            hresult: error.hresult,
            detail: error.detail,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BrokerBusInfo {
    path: String,
    module_name: String,
    port: Option<u8>,
    name: String,
    pci_vendor: u16,
    pci_device: u16,
    pci_subsystem_vendor: u16,
    pci_subsystem_device: u16,
    module_path: String,
}

impl From<&WindowsSmBusBusInfo> for BrokerBusInfo {
    fn from(info: &WindowsSmBusBusInfo) -> Self {
        Self {
            path: info.path.clone(),
            module_name: info.module_name.to_owned(),
            port: info.port,
            name: info.name.clone(),
            pci_vendor: info.pci_vendor,
            pci_device: info.pci_device,
            pci_subsystem_vendor: info.pci_subsystem_vendor,
            pci_subsystem_device: info.pci_subsystem_device,
            module_path: info.module_path.display().to_string(),
        }
    }
}

impl TryFrom<BrokerBusInfo> for WindowsSmBusBusInfo {
    type Error = PawnIoError;

    fn try_from(info: BrokerBusInfo) -> PawnIoResult<Self> {
        Ok(Self {
            path: info.path,
            module_name: module_name_from_wire(&info.module_name)?,
            port: info.port,
            name: info.name,
            pci_vendor: info.pci_vendor,
            pci_device: info.pci_device,
            pci_subsystem_vendor: info.pci_subsystem_vendor,
            pci_subsystem_device: info.pci_subsystem_device,
            module_path: PathBuf::from(info.module_path),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BrokerSmBusDirection {
    Read,
    Write,
}

impl From<SmBusDirection> for BrokerSmBusDirection {
    fn from(direction: SmBusDirection) -> Self {
        match direction {
            SmBusDirection::Read => Self::Read,
            SmBusDirection::Write => Self::Write,
        }
    }
}

impl From<BrokerSmBusDirection> for SmBusDirection {
    fn from(direction: BrokerSmBusDirection) -> Self {
        match direction {
            BrokerSmBusDirection::Read => Self::Read,
            BrokerSmBusDirection::Write => Self::Write,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerSmBusBatchOperation {
    Transfer {
        direction: BrokerSmBusDirection,
        command: u8,
        transaction: BrokerSmBusTransaction,
    },
    Delay {
        duration_ms: u64,
    },
}

impl From<&SmBusBatchOperation> for BrokerSmBusBatchOperation {
    fn from(operation: &SmBusBatchOperation) -> Self {
        match operation {
            SmBusBatchOperation::Transfer {
                direction,
                command,
                transaction,
            } => Self::Transfer {
                direction: (*direction).into(),
                command: *command,
                transaction: BrokerSmBusTransaction::from(transaction),
            },
            SmBusBatchOperation::Delay { duration } => Self::Delay {
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerSmBusTransaction {
    Quick,
    Byte { value: u8 },
    ByteData { value: u8 },
    WordData { value: u16 },
    BlockData { data: Vec<u8> },
}

impl From<&SmBusTransaction> for BrokerSmBusTransaction {
    fn from(transaction: &SmBusTransaction) -> Self {
        match transaction {
            SmBusTransaction::Quick => Self::Quick,
            SmBusTransaction::Byte { value } => Self::Byte { value: *value },
            SmBusTransaction::ByteData { value } => Self::ByteData { value: *value },
            SmBusTransaction::WordData { value } => Self::WordData { value: *value },
            SmBusTransaction::BlockData { data } => Self::BlockData {
                data: data.as_slice().to_vec(),
            },
        }
    }
}

impl TryFrom<BrokerSmBusTransaction> for SmBusTransaction {
    type Error = PawnIoError;

    fn try_from(transaction: BrokerSmBusTransaction) -> PawnIoResult<Self> {
        match transaction {
            BrokerSmBusTransaction::Quick => Ok(Self::Quick),
            BrokerSmBusTransaction::Byte { value } => Ok(Self::Byte { value }),
            BrokerSmBusTransaction::ByteData { value } => Ok(Self::ByteData { value }),
            BrokerSmBusTransaction::WordData { value } => Ok(Self::WordData { value }),
            BrokerSmBusTransaction::BlockData { data } => Ok(Self::BlockData {
                data: SmBusBlockData::new(&data)?,
            }),
        }
    }
}

fn unexpected_response(operation: &'static str) -> PawnIoError {
    PawnIoError::broker_call(
        operation,
        "broker returned the wrong response type".to_owned(),
    )
}

struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attrs: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    fn new() -> Result<Self> {
        let sddl = wide_null(PIPE_SDDL);
        let mut descriptor = std::ptr::null_mut();
        let converted = unsafe {
            // SAFETY: `sddl` is NUL-terminated UTF-16, and `descriptor` is a
            // valid out pointer owned by the returned `PipeSecurity`.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(anyhow!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW failed"
            ));
        }

        Ok(Self {
            descriptor,
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.cast::<c_void>(),
                bInheritHandle: 0,
            },
        })
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        (&mut self.attrs as *mut SECURITY_ATTRIBUTES).cast::<c_void>()
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            let _ = unsafe {
                // SAFETY: The descriptor was allocated by
                // ConvertStringSecurityDescriptorToSecurityDescriptorW and is
                // released once when this owner is dropped.
                LocalFree(self.descriptor.cast::<c_void>())
            };
        }
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        BrokerBusInfo, BrokerEnvelope, BrokerError, BrokerSmBusBatchOperation,
        BrokerSmBusDirection, BrokerSmBusTransaction, MAX_BATCH_DELAY_MS, PawnIoError,
        SmBusBatchOperation, SmBusBlockData, SmBusDirection, SmBusTransaction, WindowsSmBusBusInfo,
    };
    use crate::PawnIoErrorKind;

    #[test]
    fn transaction_dto_round_trips_block_data() {
        let transaction = SmBusTransaction::BlockData {
            data: SmBusBlockData::new(&[1, 2, 3]).expect("valid block data"),
        };
        let dto = BrokerSmBusTransaction::from(&transaction);

        let decoded = SmBusTransaction::try_from(dto).expect("dto should decode");

        assert_eq!(decoded, transaction);
    }

    #[test]
    fn direction_dto_round_trips() {
        let dto = BrokerSmBusDirection::from(SmBusDirection::Read);
        assert!(matches!(SmBusDirection::from(dto), SmBusDirection::Read));
    }

    #[test]
    fn batch_operation_dto_preserves_transfers_and_delays() {
        let transfer = SmBusBatchOperation::Transfer {
            direction: SmBusDirection::Write,
            command: 0x03,
            transaction: SmBusTransaction::BlockData {
                data: SmBusBlockData::new(&[1, 2, 3]).expect("valid block data"),
            },
        };
        let delay = SmBusBatchOperation::Delay {
            duration: Duration::from_millis(7),
        };

        let transfer_dto = BrokerSmBusBatchOperation::from(&transfer);
        let delay_dto = BrokerSmBusBatchOperation::from(&delay);

        assert!(matches!(
            transfer_dto,
            BrokerSmBusBatchOperation::Transfer {
                direction: BrokerSmBusDirection::Write,
                command: 0x03,
                transaction: BrokerSmBusTransaction::BlockData { .. }
            }
        ));
        assert!(matches!(
            delay_dto,
            BrokerSmBusBatchOperation::Delay { duration_ms: 7 }
        ));
    }

    #[test]
    fn bus_info_dto_preserves_known_module_name() {
        let info = WindowsSmBusBusInfo {
            path: "pawnio:i801".to_owned(),
            module_name: "SmbusI801.bin",
            port: None,
            name: "I801".to_owned(),
            pci_vendor: 1,
            pci_device: 2,
            pci_subsystem_vendor: 3,
            pci_subsystem_device: 4,
            module_path: PathBuf::from(r"C:\PawnIO\SmbusI801.bin"),
        };

        let decoded =
            WindowsSmBusBusInfo::try_from(BrokerBusInfo::from(&info)).expect("dto should decode");

        assert_eq!(decoded, info);
    }

    #[test]
    fn batch_delay_rejects_values_above_maximum() {
        let error = super::validate_batch_delay_ms(MAX_BATCH_DELAY_MS + 1)
            .expect_err("delay above max should be rejected");
        assert!(matches!(error, PawnIoError::InvalidInput { .. }));
    }

    #[test]
    fn broker_round_trip_preserves_hresult_and_kind() {
        let envelope = BrokerEnvelope::ok(Err(PawnIoError::SmBusIo {
            operation: "test",
            bus_path: "pawnio:test".to_owned(),
            address: 0x40,
            hresult: 0x8007_0005,
            detail: "harmless display text".to_owned(),
        }));
        let encoded = serde_json::to_vec(&envelope).expect("envelope should encode");
        let decoded =
            serde_json::from_slice::<BrokerEnvelope>(&encoded).expect("envelope should decode");
        let error = decoded
            .into_result("test")
            .expect_err("broker result should fail");

        assert_eq!(error.kind(), PawnIoErrorKind::PermissionDenied);
        assert_eq!(error.hresult(), Some(0x8007_0005));
    }

    #[test]
    fn broker_classification_ignores_adversarial_detail() {
        let envelope = BrokerEnvelope {
            ok: false,
            data: None,
            error: Some(BrokerError {
                kind: PawnIoErrorKind::Io,
                hresult: None,
                detail: "access denied and module not found".to_owned(),
            }),
        };
        let error = envelope
            .into_result("test")
            .expect_err("broker result should fail");

        assert_eq!(error.kind(), PawnIoErrorKind::Io);
    }
}
