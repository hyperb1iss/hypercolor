//! The programs `tests/update_service_proof.rs` installs in place of real
//! ones. What this binary does depends on the name it was copied to:
//!
//! - `hypercolor.exe`: an activator that appends what it was asked to the
//!   proof log, then follows `behavior.txt`.
//! - `hypercolor-app.exe`: an app that appends its arguments to the log.
//! - `slot-a.exe` or `slot-b.exe`: a broken bootstrap, following
//!   `broken-slot.txt`: `crash-after-start` reaches the service manager and
//!   dies, and `exit-before-start` never reaches it.

#[cfg(windows)]
mod fixture {
    use std::ffi::OsString;
    use std::fs::{self, OpenOptions};
    use std::io::{Read as _, Write as _};
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::Duration;

    use hypercolor_install::windows::{INHERITED_PIPE_VARIABLE, UPDATE_SERVICE_NAME};
    use hypercolor_platform_fs::{windows_adopt_inherited_pipe, windows_connect_pipe_client};
    use windows_service::service::{
        ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    const PROOF_DIRECTORY: &str = r"C:\ProgramData\HypercolorUpdateProof";

    define_windows_service!(ffi_crashing_main, crashing_main);

    fn crashing_main(_arguments: Vec<OsString>) {
        let handler = |_| ServiceControlHandlerResult::NoError;
        if let Ok(handle) = service_control_handler::register(UPDATE_SERVICE_NAME, handler) {
            let _ = handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Running,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::NO_ERROR,
                checkpoint: 0,
                wait_hint: Duration::ZERO,
                process_id: None,
            });
        }
        std::process::abort();
    }

    fn log(line: &str) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(PROOF_DIRECTORY).join("log.txt"))
            .expect("open the proof log");
        writeln!(file, "{line}").expect("write the proof log");
    }

    /// The action `behavior.txt` names for `mode`, one `<mode> <action>`
    /// line each.
    fn action(mode: &str) -> String {
        fs::read_to_string(Path::new(PROOF_DIRECTORY).join("behavior.txt"))
            .expect("read the behavior")
            .lines()
            .find_map(|line| {
                line.strip_prefix(mode)?
                    .strip_prefix(' ')
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "exit 0".to_owned())
    }

    /// Prove the inherited handle reaches the client that verified it.
    fn serve_pipe() {
        let value = std::env::var(INHERITED_PIPE_VARIABLE)
            .expect("the bootstrap names the pipe")
            .parse::<usize>()
            .expect("a handle value");
        // SAFETY: The bootstrap passed down this one handle and this process
        // takes it once.
        let mut pipe = unsafe { windows_adopt_inherited_pipe(value) }.expect("adopt the pipe");
        windows_connect_pipe_client(&pipe).expect("wait for the client");
        pipe.write_all(b"ready").expect("greet the client");
        let mut rest = Vec::new();
        let _ = pipe.read_to_end(&mut rest);
    }

    fn activator(unit: &str, mode: &str) -> ExitCode {
        log(&format!(
            "activator {unit} {mode} pid={}",
            std::process::id()
        ));
        let action = action(mode);
        match action.split_once(' ') {
            Some(("exit", code)) => ExitCode::from(code.parse::<u8>().expect("exit code")),
            None if action == "serve" => {
                serve_pipe();
                ExitCode::SUCCESS
            }
            None if action == "hold" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            _ => panic!("unknown behavior {action}"),
        }
    }

    pub fn run() -> ExitCode {
        let image = PathBuf::from(std::env::args_os().next().expect("program name"));
        let image = std::env::current_exe().unwrap_or(image);
        let name = image
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let unit = image
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let arguments = std::env::args().skip(1).collect::<Vec<_>>();
        match name.as_str() {
            "hypercolor.exe" => activator(&unit, arguments.last().expect("a mode")),
            "hypercolor-app.exe" => {
                log(&format!("app {unit} {}", arguments.join(" ")));
                ExitCode::SUCCESS
            }
            "slot-a.exe" | "slot-b.exe" => {
                let path = Path::new(PROOF_DIRECTORY).join("broken-slot.txt");
                let role = fs::read_to_string(path).expect("read the broken slot's role");
                if role.trim() == "crash-after-start" {
                    let _ = service_dispatcher::start(UPDATE_SERVICE_NAME, ffi_crashing_main);
                }
                ExitCode::FAILURE
            }
            _ => ExitCode::FAILURE,
        }
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    fixture::run()
}

#[cfg(not(windows))]
fn main() {}
