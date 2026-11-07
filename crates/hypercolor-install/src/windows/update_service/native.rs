//! The update service's registration, through the service control manager.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess};
use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
    ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::{
    FAILURE_RESET_PERIOD, FALLBACK_DELAY, PROBE_FLAG, RESTART_DELAY, UpdateServiceConfig,
    fallback_command,
};
use crate::windows::machine_layout::{BootstrapSlot, WindowsInstallLayout};
use crate::windows::store::replace_durably;
use crate::windows::update_pipe::UPDATE_SERVICE_NAME;

const SERVICE_DISPLAY_NAME: &str = "Hypercolor Update";
/// `ERROR_SERVICE_DOES_NOT_EXIST`.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
/// `ERROR_SERVICE_ALREADY_RUNNING`.
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;
/// `ERROR_SERVICE_EXISTS`.
const ERROR_SERVICE_EXISTS: i32 = 1073;
/// `ERROR_SERVICE_NOT_ACTIVE`.
const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
/// How long a run gets to end when an installer removes the service. Its
/// own stop ends the activator without waiting for it.
const STOP_DEADLINE: Duration = Duration::from_secs(30);
const STOP_PAUSE: Duration = Duration::from_millis(200);
/// `CREATE_NO_WINDOW`: a service has no console to borrow.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SERVICE_RIGHTS: ServiceAccess = ServiceAccess::QUERY_CONFIG
    .union(ServiceAccess::CHANGE_CONFIG)
    .union(ServiceAccess::QUERY_STATUS)
    .union(ServiceAccess::START);

/// The registered `HypercolorUpdate` service of one installation.
pub struct ScmUpdateService {
    service: Service,
    layout: WindowsInstallLayout,
}

impl ScmUpdateService {
    /// Open the registered service.
    ///
    /// # Errors
    ///
    /// Returns the service manager's error, which is access denied for
    /// anyone but SYSTEM and Administrators.
    pub fn open(layout: &WindowsInstallLayout) -> io::Result<Self> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(service_error)?;
        let service = manager
            .open_service(UPDATE_SERVICE_NAME, SERVICE_RIGHTS)
            .map_err(service_error)?;
        Ok(Self {
            service,
            layout: layout.clone(),
        })
    }

    /// Register the service on `image`, with `fallback` retained: delayed
    /// auto-start, which means `recover` at each boot, and failure actions
    /// that cover a non-zero exit as well as a crash.
    ///
    /// The recorded user cannot start the service until its security
    /// descriptor says so, which `update_service_sddl` spells and the
    /// `--register-service` bootstrap command applies through [`Self::service`].
    ///
    /// One call spans several changes, so a service that already exists is
    /// set up again the same way: a repeated run finishes a registration
    /// that was cut short.
    ///
    /// # Errors
    ///
    /// Returns the service manager's error for any step.
    pub fn register(
        layout: &WindowsInstallLayout,
        image: BootstrapSlot,
        fallback: BootstrapSlot,
    ) -> io::Result<Self> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(service_error)?;
        let info = ServiceInfo {
            name: OsString::from(UPDATE_SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: layout.slot_image(image),
            launch_arguments: Vec::new(),
            dependencies: Vec::new(),
            account_name: None,
            account_password: None,
        };
        let rights = SERVICE_RIGHTS | ServiceAccess::WRITE_DAC;
        let service = match manager.create_service(&info, rights) {
            Ok(service) => service,
            Err(windows_service::Error::Winapi(error))
                if error.raw_os_error() == Some(ERROR_SERVICE_EXISTS) =>
            {
                manager
                    .open_service(UPDATE_SERVICE_NAME, rights)
                    .map_err(service_error)?
            }
            Err(error) => return Err(service_error(error)),
        };
        service
            .set_delayed_auto_start(true)
            .map_err(service_error)?;
        service
            .set_failure_actions_on_non_crash_failures(true)
            .map_err(service_error)?;
        let mut registered = Self {
            service,
            layout: layout.clone(),
        };
        registered.set_image(image)?;
        registered.set_fallback(fallback)?;
        Ok(registered)
    }

    /// Stop the registered service and delete it. A service that is not
    /// registered is already gone.
    ///
    /// # Errors
    ///
    /// Returns the service manager's error, or a timeout when it does not
    /// stop.
    pub fn remove() -> io::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(service_error)?;
        let rights = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
        let service = match manager.open_service(UPDATE_SERVICE_NAME, rights) {
            Ok(service) => service,
            Err(windows_service::Error::Winapi(error))
                if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
            {
                return Ok(());
            }
            Err(error) => return Err(service_error(error)),
        };
        match service.stop() {
            Ok(_) => {}
            Err(windows_service::Error::Winapi(error))
                if error.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE) => {}
            Err(error) => return Err(service_error(error)),
        }
        let deadline = Instant::now() + STOP_DEADLINE;
        while service.query_status().map_err(service_error)?.current_state != ServiceState::Stopped
        {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{UPDATE_SERVICE_NAME} did not stop"),
                ));
            }
            std::thread::sleep(STOP_PAUSE);
        }
        service.delete().map_err(service_error)
    }

    /// The service handle, for a host that sets what this safe surface does
    /// not reach.
    #[must_use]
    pub fn service(&self) -> &Service {
        &self.service
    }

    /// The slot `ImagePath` names, or `None` when it names neither.
    ///
    /// # Errors
    ///
    /// Returns the service manager's error.
    pub fn image_slot(&self) -> io::Result<Option<BootstrapSlot>> {
        let config = self.service.query_config().map_err(service_error)?;
        Ok(BootstrapSlot::from_image_path(
            config.executable_path.as_os_str(),
        ))
    }
}

impl UpdateServiceConfig for ScmUpdateService {
    fn set_image(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        let config = self.service.query_config().map_err(service_error)?;
        self.service
            .change_config(&ServiceInfo {
                name: OsString::from(UPDATE_SERVICE_NAME),
                display_name: config.display_name,
                service_type: config.service_type,
                start_type: config.start_type,
                error_control: config.error_control,
                executable_path: self.layout.slot_image(slot),
                launch_arguments: Vec::new(),
                dependencies: config.dependencies,
                account_name: None,
                account_password: None,
            })
            .map_err(service_error)
    }

    fn set_fallback(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        self.service
            .update_failure_actions(ServiceFailureActions {
                reset_period: ServiceFailureResetPeriod::After(FAILURE_RESET_PERIOD),
                reboot_msg: None,
                command: Some(fallback_command(&self.layout.slot_image(slot))),
                actions: Some(vec![
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: RESTART_DELAY,
                    },
                    ServiceAction {
                        action_type: ServiceActionType::RunCommand,
                        delay: FALLBACK_DELAY,
                    },
                ]),
            })
            .map_err(service_error)
    }

    fn write_slot(&mut self, slot: BootstrapSlot, source: &Path) -> io::Result<()> {
        let bytes = fs::read(source)?;
        let directory = WindowsDirectory::open(
            &self.layout.bootstrap_directory(),
            WindowsDirectoryAccess::Write,
        )?;
        replace_durably(&directory, slot.file_name(), &bytes)
    }

    fn probe_slot(&mut self, slot: BootstrapSlot) -> io::Result<()> {
        run_slot(&self.layout, slot, PROBE_FLAG)
    }

    fn start(&mut self) -> io::Result<()> {
        match self.service.start::<&OsStr>(&[]) {
            Ok(()) => Ok(()),
            Err(windows_service::Error::Winapi(error))
                if error.raw_os_error() == Some(ERROR_SERVICE_ALREADY_RUNNING) =>
            {
                Ok(())
            }
            Err(error) => Err(service_error(error)),
        }
    }
}

/// Run the bootstrap in `slot` with `flag` and require a clean exit. That
/// proves the image loads, which the service manager never would.
///
/// # Errors
///
/// Returns an error when it does not start or does not exit zero.
pub fn run_slot(layout: &WindowsInstallLayout, slot: BootstrapSlot, flag: &str) -> io::Result<()> {
    let status = Command::new(layout.slot_image(slot))
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the bootstrap in {} exited with {status} for {flag}",
            slot.file_name()
        )))
    }
}

fn service_error(error: windows_service::Error) -> io::Error {
    match error {
        windows_service::Error::Winapi(error) => error,
        other => io::Error::other(other.to_string()),
    }
}
