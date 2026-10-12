//! What the Service Control Manager says about the SMBus broker, and
//! whether the broker answers on its pipe.
//!
//! The daemon runs unelevated and reaches PawnIO only through the broker,
//! so a broker that is registered but stopped silently darkens motherboard
//! and RAM lighting and CPU temperature. This is the read side diagnostics
//! use to name that state. Querying needs only `SERVICE_QUERY_STATUS` and
//! `SERVICE_QUERY_CONFIG`, which a service's default security descriptor
//! grants to interactive users.

use std::path::PathBuf;

use windows_service::service::{ServiceAccess, ServiceExitCode, ServiceStartType, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::broker::{SERVICE_NAME, direct_mode_enabled, ping};
use super::service_log::broker_log_path;
use super::{PawnIoError, PawnIoResult};

/// `ERROR_SERVICE_DOES_NOT_EXIST`.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

/// Broker state as the Service Control Manager reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerServiceState {
    /// No `HypercolorSmBus` service is registered.
    NotInstalled,
    /// `SERVICE_STOPPED`.
    Stopped,
    /// `SERVICE_START_PENDING`.
    StartPending,
    /// `SERVICE_STOP_PENDING`.
    StopPending,
    /// `SERVICE_RUNNING`.
    Running,
    /// Paused or moving between paused and running, which the broker never
    /// asks for.
    Paused,
}

/// One reading of the broker's service registration and health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerServiceStatus {
    /// Service Control Manager name of the broker.
    pub service_name: &'static str,
    /// Current state.
    pub state: BrokerServiceState,
    /// `dwWin32ExitCode`: 0 after a clean stop, 1077 when the service has
    /// not been started since boot, 1066 when the broker reported its own
    /// code in [`Self::service_specific_exit_code`].
    pub win32_exit_code: u32,
    /// `dwServiceSpecificExitCode`, meaningful when
    /// [`Self::win32_exit_code`] is 1066.
    pub service_specific_exit_code: u32,
    /// Whether Windows starts the broker at boot, when the configuration
    /// could be read.
    pub starts_at_boot: Option<bool>,
    /// The registered broker executable, when the configuration could be
    /// read.
    pub executable: Option<PathBuf>,
    /// The broker's own log, beside the registered executable.
    pub log_path: Option<PathBuf>,
    /// Whether the broker answered on its pipe just now. `None` when it was
    /// not asked because the service is not running.
    pub answering: Option<bool>,
    /// The version a running broker reported, when it supports the probe.
    pub version: Option<String>,
    /// True when `HYPERCOLOR_PAWNIO_DIRECT` makes this process reach PawnIO
    /// itself, so the broker is not needed.
    pub direct_mode: bool,
}

/// Read the broker's registration and, when it is running, probe its pipe.
///
/// # Errors
///
/// Returns [`PawnIoError::BrokerUnavailable`] when the Service Control
/// Manager cannot be opened or the service cannot be queried. A broker
/// that is simply not registered is reported as
/// [`BrokerServiceState::NotInstalled`], not as an error.
pub fn broker_service_status() -> PawnIoResult<BrokerServiceStatus> {
    let direct_mode = direct_mode_enabled();
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|error| scm_error("open the Service Control Manager", &error))?;
    let service = match manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
    ) {
        Ok(service) => service,
        Err(windows_service::Error::Winapi(error))
            if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
        {
            return Ok(BrokerServiceStatus {
                service_name: SERVICE_NAME,
                state: BrokerServiceState::NotInstalled,
                win32_exit_code: 0,
                service_specific_exit_code: 0,
                starts_at_boot: None,
                executable: None,
                log_path: None,
                answering: None,
                version: None,
                direct_mode,
            });
        }
        Err(error) => return Err(scm_error("open the HypercolorSmBus service", &error)),
    };

    let status = service
        .query_status()
        .map_err(|error| scm_error("query the HypercolorSmBus service", &error))?;
    // The configuration only adds context (start type, log location), so a
    // failure to read it leaves those fields empty instead of failing.
    let config = service.query_config().ok();
    let executable = config.as_ref().map(|config| config.executable_path.clone());
    let log_path = executable.as_deref().and_then(broker_log_path);
    let starts_at_boot = config
        .as_ref()
        .map(|config| config.start_type == ServiceStartType::AutoStart);

    let state = match status.current_state {
        ServiceState::Stopped => BrokerServiceState::Stopped,
        ServiceState::StartPending => BrokerServiceState::StartPending,
        ServiceState::StopPending => BrokerServiceState::StopPending,
        ServiceState::Running => BrokerServiceState::Running,
        ServiceState::ContinuePending | ServiceState::PausePending | ServiceState::Paused => {
            BrokerServiceState::Paused
        }
    };
    let (win32_exit_code, service_specific_exit_code) = match status.exit_code {
        ServiceExitCode::Win32(code) => (code, 0),
        ServiceExitCode::ServiceSpecific(code) => (crate::ERROR_SERVICE_SPECIFIC_ERROR, code),
    };

    let (answering, version) = if state == BrokerServiceState::Running {
        match ping() {
            Ok(version) => (Some(true), Some(version)),
            Err(PawnIoError::BrokerUnavailable { .. }) => (Some(false), None),
            // Any other error is a well-formed reply from a broker that
            // predates the ping request: it is listening.
            Err(_) => (Some(true), None),
        }
    } else {
        (None, None)
    };

    Ok(BrokerServiceStatus {
        service_name: SERVICE_NAME,
        state,
        win32_exit_code,
        service_specific_exit_code,
        starts_at_boot,
        executable,
        log_path,
        answering,
        version,
        direct_mode,
    })
}

fn scm_error(action: &str, error: &windows_service::Error) -> PawnIoError {
    // The crate's own Display for `Winapi` drops the OS message, so read
    // it from the wrapped io::Error.
    let detail = match error {
        windows_service::Error::Winapi(source) => source.to_string(),
        other => other.to_string(),
    };
    PawnIoError::BrokerUnavailable {
        detail: format!("could not {action}: {detail}"),
    }
}
