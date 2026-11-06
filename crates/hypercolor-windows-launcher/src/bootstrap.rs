//! The recovery bootstrap: the process the service manager registers.
//!
//! One run selects the release, creates the update pipe, starts the
//! activator from that release and waits for it. The activator does the
//! work; this process stays behind it because three things only reach the
//! registered process. A client trusts the pipe only if this process
//! created it, session notifications arrive here, and the service manager
//! watches this process for the failure that starts a recovery.

use std::error::Error;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::windows::io::{AsHandle as _, AsRawHandle as _, OwnedHandle};
use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use hypercolor_install::windows::{
    BootstrapCommand, BootstrapSlot, EXIT_NO_ACTIVATOR, INHERITED_PIPE_VARIABLE, ScmUpdateService,
    UPDATE_SERVICE_NAME, UpdatePipeName, UpdateServiceExit, UpdateServiceHost, UpdateServiceMode,
    WINDOWS_CLI_PROGRAM, WindowsInstallLayout, WindowsReleaseRecords, recover_fallback,
    run_update_service, select_release, session_logon_notice, update_service_sddl,
};
use hypercolor_install::{InstallStore, MAX_MANAGED_INSTALL_JOURNAL_BYTES};
use hypercolor_platform_fs::{WindowsSid, windows_create_pipe_server};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType, SessionChangeReason,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::{define_windows_service, service_dispatcher};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::job::{enclose_this_process_in_kill_on_close_job, terminate};
use crate::service_security::set_service_dacl;

/// Covers selecting a release and creating the pipe, which is local file
/// work and well under the service manager's default.
const START_WAIT_HINT: Duration = Duration::from_secs(10);
/// Covers ending the activator, which needs no cooperation from it.
const STOP_WAIT_HINT: Duration = Duration::from_secs(5);
const USAGE: &str = "usage: slot-a.exe [--recover-fallback | --register-service | --probe]";

define_windows_service!(ffi_service_main, service_main);

/// The process entry point of both slots.
#[must_use]
pub fn run() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let Some(command) = BootstrapCommand::parse(&arguments) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result: Result<(), Box<dyn Error>> = match command {
        // StartServiceCtrlDispatcher only succeeds when the service manager
        // started this process.
        BootstrapCommand::Service => {
            service_dispatcher::start(UPDATE_SERVICE_NAME, ffi_service_main).map_err(Into::into)
        }
        BootstrapCommand::RecoverFallback => fall_back().map_err(Into::into),
        BootstrapCommand::RegisterService => register(),
        BootstrapCommand::Probe => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{UPDATE_SERVICE_NAME}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn fall_back() -> io::Result<()> {
    let (layout, slot) = own_layout()?;
    let mut service = ScmUpdateService::open(&layout)?;
    recover_fallback(&mut service, slot)
}

/// Register the service on this slot, with the other retained, for the user
/// the installer recorded.
fn register() -> Result<(), Box<dyn Error>> {
    let (layout, slot) = own_layout()?;
    let records = release_records(&layout)?;
    let installation = records.installation()?;
    let service = ScmUpdateService::register(&layout, slot, slot.other())?;
    set_service_dacl(
        service.service(),
        &update_service_sddl(installation.user_sid()),
    )?;
    Ok(())
}

fn release_records(layout: &WindowsInstallLayout) -> Result<WindowsReleaseRecords, Box<dyn Error>> {
    let store = InstallStore::with_roots(
        layout.release_root(),
        layout.state_root(),
        MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    )?;
    Ok(store.release_records()?)
}

fn own_layout() -> io::Result<(WindowsInstallLayout, BootstrapSlot)> {
    let image = std::env::current_exe()?;
    std::env::var_os("ProgramData")
        .and_then(|data| WindowsInstallLayout::of_bootstrap(&image, data.as_ref()))
        .ok_or_else(|| io::Error::other(format!("{} is not a bootstrap slot", image.display())))
}

/// What the control handler reaches from outside the run.
#[derive(Default)]
struct RunControl {
    status: OnceLock<ServiceStatusHandle>,
    activator: OnceLock<OwnedHandle>,
    stopping: AtomicBool,
}

fn service_main(arguments: Vec<OsString>) {
    let control = Arc::new(RunControl::default());
    let (logons, sign_ins) = mpsc::channel();
    let handler_control = Arc::clone(&control);
    let handler = move |event| match event {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop => {
            handler_control.stopping.store(true, Ordering::SeqCst);
            if let Some(handle) = handler_control.status.get() {
                report(
                    *handle,
                    ServiceState::StopPending,
                    ServiceExitCode::NO_ERROR,
                );
            }
            // The run reports the stop once the activator is gone. The job
            // takes its helpers down when this process ends, and the journal
            // holds whatever was in flight for the next recovery.
            if let Some(process) = handler_control.activator.get() {
                terminate(process);
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(change) => {
            if change.reason == SessionChangeReason::SessionLogon {
                // A dead relay means the activator is gone already.
                let _ = logons.send(change.notification.session_id);
            }
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    // Without a handle the service manager cannot be told anything, and
    // ending here reads as the crash it is.
    let Ok(handle) = service_control_handler::register(UPDATE_SERVICE_NAME, handler) else {
        return;
    };
    let _ = control.status.set(handle);
    report(
        handle,
        ServiceState::StartPending,
        ServiceExitCode::NO_ERROR,
    );

    // The first element is the service's name.
    let start_arguments = arguments.get(1..).unwrap_or_default();
    let exit = if enclose_this_process_in_kill_on_close_job().is_ok() {
        let mut host = BootstrapHost {
            control: Arc::clone(&control),
            sign_ins: Some(sign_ins),
            records: None,
        };
        run_update_service(&mut host, start_arguments)
    } else {
        UpdateServiceExit::Failed(EXIT_NO_ACTIVATOR)
    };
    // A stopped service is not a failed one, whatever the stop did to the
    // activator, so no recovery follows it.
    let exit_code = match exit {
        UpdateServiceExit::Failed(code) if !control.stopping.load(Ordering::SeqCst) => {
            ServiceExitCode::ServiceSpecific(code)
        }
        _ => ServiceExitCode::NO_ERROR,
    };
    report(handle, ServiceState::Stopped, exit_code);
}

fn report(handle: ServiceStatusHandle, state: ServiceState, exit_code: ServiceExitCode) {
    let (controls_accepted, wait_hint) = match state {
        ServiceState::StartPending => (ServiceControlAccept::empty(), START_WAIT_HINT),
        ServiceState::StopPending => (ServiceControlAccept::empty(), STOP_WAIT_HINT),
        ServiceState::Stopped => (ServiceControlAccept::empty(), Duration::ZERO),
        _ => (
            ServiceControlAccept::STOP | ServiceControlAccept::SESSION_CHANGE,
            Duration::ZERO,
        ),
    };
    // A status the service manager refuses cannot be reported any other
    // way; if this was the last one, the process ending says the rest.
    let _ = handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted,
        exit_code,
        checkpoint: 0,
        wait_hint,
        process_id: None,
    });
}

struct BootstrapHost {
    control: Arc<RunControl>,
    sign_ins: Option<mpsc::Receiver<u32>>,
    records: Option<WindowsReleaseRecords>,
}

impl UpdateServiceHost for BootstrapHost {
    type Pipe = File;
    type Activator = PathBuf;
    type Error = Box<dyn Error>;

    fn select_activator(&mut self) -> Result<PathBuf, Self::Error> {
        let (layout, _) = own_layout()?;
        let mut records = release_records(&layout)?;
        let unit = select_release(&mut records)?;
        let program = records.unit_program(&unit, WINDOWS_CLI_PROGRAM)?;
        self.records = Some(records);
        Ok(program)
    }

    fn open_pipe(&mut self) -> io::Result<File> {
        let records = self
            .records
            .as_ref()
            .ok_or_else(|| io::Error::other("the pipe needs the installation's records"))?;
        let installation = records.installation().map_err(io::Error::other)?;
        let user = WindowsSid::parse(installation.user_sid())?;
        let name = UpdatePipeName::from_random(rand::random());
        // This process creates the pipe, whoever serves it afterwards: a
        // client compares the pipe's creator with the process the service
        // manager registered for this service.
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GRGW;;;{})", user.to_string_form());
        let pipe = windows_create_pipe_server(name.as_str(), &sddl)?;
        records
            .publish_update_pipe(&name, &user)
            .map_err(io::Error::other)?;
        Ok(pipe)
    }

    fn run_activator(
        &mut self,
        activator: PathBuf,
        mode: UpdateServiceMode,
        pipe: File,
    ) -> io::Result<i32> {
        let mut child = Command::new(activator)
            .args(mode.activator_arguments())
            .env(
                INHERITED_PIPE_VARIABLE,
                (pipe.as_raw_handle() as usize).to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        if let (Some(mut input), Some(sign_ins)) = (child.stdin.take(), self.sign_ins.take()) {
            // A thread, so an activator that stopped reading cannot hold up
            // the service manager's control handler.
            std::thread::spawn(move || {
                for session in sign_ins {
                    if input
                        .write_all(session_logon_notice(session).as_bytes())
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
        let _ = self
            .control
            .activator
            .set(child.as_handle().try_clone_to_owned()?);
        if let Some(handle) = self.control.status.get() {
            report(*handle, ServiceState::Running, ServiceExitCode::NO_ERROR);
        }
        // The pipe stays open here until the activator ends, so the instance
        // this process created outlives every client's check of it.
        let status = child.wait()?;
        drop(pipe);
        // Windows always gives a code; without one the run cannot be called
        // clean.
        Ok(status.code().unwrap_or(1))
    }
}
