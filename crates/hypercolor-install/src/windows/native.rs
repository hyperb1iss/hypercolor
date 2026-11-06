//! The Win32 side of the install platform: the service control manager,
//! the firewall and the process table.
//!
//! Each method is one call or one bounded wait, and it decides nothing.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess};
use hypercolor_windows_install_native::{
    FirewallRule, ProcessId, RunningProcess, firewall_rule, process, process_with_pid,
    processes_named, set_firewall_rule_program, terminate_processes, wait_for_any_exit,
    wait_for_every_exit,
};
use hypercolor_windows_pawnio::{BROKER_SERVICE_NAME, PawnIoError, ping_broker};
use sha2::{Digest as _, Sha256};
use windows_service::service::{Service, ServiceAccess, ServiceInfo, ServiceState, ServiceStatus};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::executor::WindowsInstallExecutor;
use super::model::{
    WINDOWS_APP_MEMBER, WINDOWS_DAEMON_MEMBER, WindowsBrokerObservation, WindowsFileIdentity,
    WindowsFirewallObservation, WindowsFirewallRule, WindowsProcessId, WindowsProcessImage,
    WindowsRunningProcess, WindowsRuntimeObservation, WindowsUnitBinding, error,
};
use super::payload::bind_windows_unit;
use super::store::InstallStore;
use crate::local_http::{MAX_HTTP_RESPONSE_BYTES, http_get};
use crate::{InstallPlatformError, LocalHttpResponse, UnitId, UnitRecord};

/// `ERROR_SERVICE_DOES_NOT_EXIST`.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
/// `ERROR_SERVICE_ALREADY_RUNNING`.
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;
/// `ERROR_SERVICE_CANNOT_ACCEPT_CTRL`: a stop or start is already underway.
const ERROR_SERVICE_CANNOT_ACCEPT_CTRL: i32 = 1061;
/// `ERROR_SERVICE_NOT_ACTIVE`.
const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
/// How long the broker gets to start or stop. Its own wait hints are 15 and
/// 10 seconds.
const BROKER_DEADLINE: Duration = Duration::from_secs(30);
/// Pause between looks at a service that is changing state; the service
/// control manager offers no wait that does not need an alertable thread.
const SERVICE_STATE_PAUSE: Duration = Duration::from_millis(200);
/// How often a started app's daemon is asked whether it is ready, while the
/// wait doubles as a wait on the app's own exit.
const READINESS_PAUSE: Duration = Duration::from_millis(250);
const HASH_BUFFER_BYTES: usize = 1024 * 1024;

fn io_error(context: &str, source: impl std::fmt::Display) -> InstallPlatformError {
    error(format!("{context}: {source}"))
}

fn service_error(context: &str, source: &windows_service::Error) -> InstallPlatformError {
    io_error(context, source)
}

fn winapi_code(source: &windows_service::Error) -> Option<i32> {
    match source {
        windows_service::Error::Winapi(error) => error.raw_os_error(),
        _ => None,
    }
}

const fn process_id(id: ProcessId) -> WindowsProcessId {
    WindowsProcessId {
        pid: id.pid,
        created: id.created,
    }
}

const fn native_id(id: WindowsProcessId) -> ProcessId {
    ProcessId {
        pid: id.pid,
        created: id.created,
    }
}

fn running_process(process: RunningProcess) -> WindowsRunningProcess {
    WindowsRunningProcess {
        id: process_id(process.id),
        image: process.image,
    }
}

fn native_ids(processes: &[WindowsProcessId]) -> Vec<ProcessId> {
    processes.iter().copied().map(native_id).collect()
}

/// The machine a Windows installation runs on, under the service control
/// manager's and the firewall's real rules.
#[derive(Debug, Clone)]
pub struct WindowsNativeExecutor {
    store: InstallStore,
    broker_service: String,
    daemon_api: SocketAddr,
}

impl WindowsNativeExecutor {
    /// The executor of the installation `store` holds, whose daemon answers
    /// at `daemon_api`.
    #[must_use]
    pub fn new(store: InstallStore, daemon_api: SocketAddr) -> Self {
        Self {
            store,
            broker_service: BROKER_SERVICE_NAME.to_owned(),
            daemon_api,
        }
    }

    /// Drive `service` as the broker, for harnesses that register a test
    /// service.
    #[must_use]
    pub fn with_broker_service(mut self, service: impl Into<String>) -> Self {
        self.broker_service = service.into();
        self
    }

    fn open_broker(&self, access: ServiceAccess) -> Result<Option<Service>, InstallPlatformError> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
                .map_err(|source| service_error("open the service control manager", &source))?;
        match manager.open_service(&self.broker_service, access) {
            Ok(service) => Ok(Some(service)),
            Err(source) if winapi_code(&source) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(None),
            Err(source) => Err(service_error("open the SMBus broker service", &source)),
        }
    }

    fn require_broker(&self, access: ServiceAccess) -> Result<Service, InstallPlatformError> {
        self.open_broker(access)?
            .ok_or_else(|| error("the SMBus broker is not registered"))
    }
}

fn broker_status(service: &Service) -> Result<ServiceStatus, InstallPlatformError> {
    service
        .query_status()
        .map_err(|source| service_error("query the SMBus broker", &source))
}

fn require_time_left(
    status: &ServiceStatus,
    wanted: &str,
    deadline: Instant,
) -> Result<(), InstallPlatformError> {
    if Instant::now() < deadline {
        std::thread::sleep(SERVICE_STATE_PAUSE);
        return Ok(());
    }
    Err(error(format!(
        "the SMBus broker is {:?}, not {wanted}, after {} s",
        status.current_state,
        BROKER_DEADLINE.as_secs()
    )))
}

/// Stop the broker from whatever state it is in. A stop only lands on a
/// running service, so one that is still starting is asked again once it
/// runs.
fn stop_and_wait(service: &Service) -> Result<(), InstallPlatformError> {
    let deadline = Instant::now() + BROKER_DEADLINE;
    loop {
        let status = broker_status(service)?;
        match status.current_state {
            ServiceState::Stopped => return Ok(()),
            ServiceState::Running | ServiceState::Paused => match service.stop() {
                Ok(_) => {}
                Err(source)
                    if matches!(
                        winapi_code(&source),
                        Some(ERROR_SERVICE_NOT_ACTIVE | ERROR_SERVICE_CANNOT_ACCEPT_CTRL)
                    ) => {}
                Err(source) => return Err(service_error("stop the SMBus broker", &source)),
            },
            _ => {}
        }
        require_time_left(&status, "stopped", deadline)?;
    }
}

/// Start the broker unless it runs and wait until it does. A broker seen
/// stopped after this asked it to start has failed, and is not asked again.
fn start_and_wait(service: &Service) -> Result<u32, InstallPlatformError> {
    let deadline = Instant::now() + BROKER_DEADLINE;
    let mut asked = false;
    loop {
        let status = broker_status(service)?;
        match status.current_state {
            ServiceState::Running => {
                return status
                    .process_id
                    .ok_or_else(|| error("the running SMBus broker has no process"));
            }
            ServiceState::Stopped if asked => {
                return Err(error(format!(
                    "the SMBus broker stopped as it started: {:?}",
                    status.exit_code
                )));
            }
            ServiceState::Stopped => {
                match service.start(&[] as &[&OsStr]) {
                    Ok(()) => {}
                    Err(source) if winapi_code(&source) == Some(ERROR_SERVICE_ALREADY_RUNNING) => {}
                    Err(source) => return Err(service_error("start the SMBus broker", &source)),
                }
                asked = true;
            }
            _ => {}
        }
        require_time_left(&status, "running", deadline)?;
    }
}

/// Wait until the broker's pipe answers. Any answer proves it is served,
/// even one that refuses the request.
fn wait_for_pipe() -> Result<(), InstallPlatformError> {
    let deadline = Instant::now() + BROKER_DEADLINE;
    loop {
        match ping_broker() {
            Err(PawnIoError::BrokerUnavailable { detail }) => {
                if Instant::now() >= deadline {
                    return Err(io_error("the SMBus broker does not answer", detail));
                }
                std::thread::sleep(SERVICE_STATE_PAUSE);
            }
            Ok(_) | Err(_) => return Ok(()),
        }
    }
}

fn hash_file(file: std::fs::File) -> io::Result<String> {
    let mut reader = file;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

impl WindowsInstallExecutor for WindowsNativeExecutor {
    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        self.store
            .read_active_unit()
            .map_err(|source| error(source.to_string()))
    }

    fn bind_unit(&mut self, unit: &UnitRecord) -> Result<WindowsUnitBinding, InstallPlatformError> {
        bind_windows_unit(unit).map_err(|source| error(source.to_string()))
    }

    fn observe_broker(&mut self) -> Result<Option<WindowsBrokerObservation>, InstallPlatformError> {
        let Some(service) =
            self.open_broker(ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS)?
        else {
            return Ok(None);
        };
        let config = service
            .query_config()
            .map_err(|source| service_error("read the SMBus broker registration", &source))?;
        let status = service
            .query_status()
            .map_err(|source| service_error("query the SMBus broker", &source))?;
        Ok(Some(WindowsBrokerObservation {
            image: config.executable_path.to_string_lossy().into_owned(),
            running: status.current_state != ServiceState::Stopped,
        }))
    }

    fn switch_broker(&mut self, program: &str) -> Result<(), InstallPlatformError> {
        let service = self.require_broker(
            ServiceAccess::QUERY_CONFIG
                | ServiceAccess::QUERY_STATUS
                | ServiceAccess::CHANGE_CONFIG
                | ServiceAccess::STOP,
        )?;
        stop_and_wait(&service)?;
        let config = service
            .query_config()
            .map_err(|source| service_error("read the SMBus broker registration", &source))?;
        // No dependencies and no account leave those parts of the
        // registration as they are.
        service
            .change_config(&ServiceInfo {
                name: OsString::from(&self.broker_service),
                display_name: config.display_name,
                service_type: config.service_type,
                start_type: config.start_type,
                error_control: config.error_control,
                executable_path: PathBuf::from(program),
                launch_arguments: Vec::new(),
                dependencies: Vec::new(),
                account_name: None,
                account_password: None,
            })
            .map_err(|source| service_error("switch the SMBus broker image", &source))
    }

    fn start_broker(&mut self) -> Result<WindowsProcessId, InstallPlatformError> {
        let service = self.require_broker(ServiceAccess::QUERY_STATUS | ServiceAccess::START)?;
        let pid = start_and_wait(&service)?;
        // The broker reports running before it opens its pipe.
        wait_for_pipe()?;
        process_with_pid(pid)
            .map_err(|source| io_error("name the SMBus broker's process", source))?
            .map(|running| process_id(running.id))
            .ok_or_else(|| error("the SMBus broker exited after it answered"))
    }

    fn observe_firewall(
        &mut self,
    ) -> Result<Option<WindowsFirewallObservation>, InstallPlatformError> {
        let program = |rule: WindowsFirewallRule| {
            firewall_rule(rule.name()).map_err(|source| io_error("read a firewall rule", source))
        };
        match (
            program(WindowsFirewallRule::Daemon)?,
            program(WindowsFirewallRule::App)?,
        ) {
            (FirewallRule::Unavailable, _) | (_, FirewallRule::Unavailable) => Ok(None),
            (daemon, app) => {
                let named = |rule| match rule {
                    FirewallRule::Program(program) => Some(program),
                    FirewallRule::Absent | FirewallRule::Unavailable => None,
                };
                Ok(Some(WindowsFirewallObservation {
                    daemon: named(daemon),
                    app: named(app),
                }))
            }
        }
    }

    fn set_firewall_rule(
        &mut self,
        rule: WindowsFirewallRule,
        program: Option<&str>,
    ) -> Result<(), InstallPlatformError> {
        set_firewall_rule_program(rule.name(), program)
            .map_err(|source| io_error("change a firewall rule", source))
    }

    fn observe_runtime(&mut self) -> Result<WindowsRuntimeObservation, InstallPlatformError> {
        let mut observation = WindowsRuntimeObservation::default();
        for process in processes_named(&[WINDOWS_APP_MEMBER, WINDOWS_DAEMON_MEMBER])
            .map_err(|source| io_error("list the running processes", source))?
        {
            let name = process
                .image
                .rsplit('\\')
                .next()
                .unwrap_or(process.image.as_str());
            if name.eq_ignore_ascii_case(WINDOWS_APP_MEMBER) {
                observation.apps.push(running_process(process));
            } else {
                observation.daemons.push(running_process(process));
            }
        }
        Ok(observation)
    }

    fn process_image(
        &mut self,
        id: WindowsProcessId,
    ) -> Result<Option<WindowsProcessImage>, InstallPlatformError> {
        let Some(running) =
            process(native_id(id)).map_err(|source| io_error("name a process", source))?
        else {
            return Ok(None);
        };
        let path = Path::new(&running.image);
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(error(format!("{} is not a file path", running.image)));
        };
        let directory = WindowsDirectory::open(parent, WindowsDirectoryAccess::Read)
            .map_err(|source| io_error("open a process image's directory", source))?;
        let metadata = directory
            .entry_metadata(name)
            .map_err(|source| io_error("inspect a process image", source))?
            .ok_or_else(|| error(format!("{} no longer exists", running.image)))?;
        let file = directory
            .open_file(name)
            .map_err(|source| io_error("open a process image", source))?;
        let sha256 = hash_file(file).map_err(|source| io_error("hash a process image", source))?;
        Ok(Some(WindowsProcessImage {
            path: running.image,
            identity: WindowsFileIdentity {
                volume_serial: metadata.volume_serial(),
                file_id: format!("{:032x}", metadata.file_id()),
                sha256,
            },
        }))
    }

    fn wait_for_every_exit(
        &mut self,
        processes: &[WindowsProcessId],
        timeout: Duration,
    ) -> Result<bool, InstallPlatformError> {
        wait_for_every_exit(&native_ids(processes), timeout)
            .map_err(|source| io_error("wait on process handles", source))
    }

    fn wait_for_any_exit(
        &mut self,
        processes: &[WindowsProcessId],
        timeout: Duration,
    ) -> Result<Option<WindowsProcessId>, InstallPlatformError> {
        wait_for_any_exit(&native_ids(processes), timeout)
            .map(|exited| exited.map(process_id))
            .map_err(|source| io_error("wait on process handles", source))
    }

    fn terminate(&mut self, processes: &[WindowsProcessId]) -> Result<(), InstallPlatformError> {
        terminate_processes(&native_ids(processes))
            .map_err(|source| io_error("end a process", source))
    }

    fn wait_for_daemon(
        &mut self,
        app: WindowsProcessId,
        timeout: Duration,
    ) -> Result<(), InstallPlatformError> {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .http_get("/health", MAX_HTTP_RESPONSE_BYTES)
                .is_ok_and(|response| response.status == 200)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(error(format!(
                    "the daemon was not ready after {} s",
                    timeout.as_secs()
                )));
            }
            if wait_for_any_exit(&[native_id(app)], READINESS_PAUSE)
                .map_err(|source| io_error("wait on the app's handle", source))?
                .is_some()
            {
                return Err(error("the app exited before its daemon was ready"));
            }
        }
    }

    fn http_get(
        &mut self,
        path: &str,
        max_bytes: usize,
    ) -> Result<LocalHttpResponse, InstallPlatformError> {
        http_get(self.daemon_api, path, max_bytes)
    }
}
