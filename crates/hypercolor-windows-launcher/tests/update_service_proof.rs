//! Proof that the real bootstrap and launcher behave under the real service
//! manager, at the real paths.
//!
//! This installs into `C:\Program Files\Hypercolor` and registers the real
//! `HypercolorUpdate`, so it runs only on a disposable machine, as an
//! administrator, when `HYPERCOLOR_PROVE_UPDATE_SERVICE` asks for it, and
//! refuses to run over an installation. The real bootstrap and launcher
//! binaries run from the slots and the launcher path; only the units hold
//! fixtures (`examples/update_proof_fixture.rs`).

#![cfg(windows)]

use std::cell::Cell;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hypercolor_install::windows::{
    BootstrapSlot, FALLBACK_DELAY, REGISTER_SERVICE_FLAG, ScmUpdateService, UPDATE_SERVICE_NAME,
    UpdatePipeHost, UpdatePipeName, UpdateServiceEnd, WindowsInstallLayout, WindowsUpdatePipeHost,
    connect_update_pipe, fallback_command, follow_update_service, install_bootstrap,
    restore_bootstrap,
};
use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess, WindowsSid};
use windows_service::service::{
    Service, ServiceAccess, ServiceExitCode, ServiceState, ServiceStatus,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const RELEASE_ROOT: &str = r"C:\Program Files\Hypercolor";
const PROGRAM_DATA: &str = r"C:\ProgramData";
const PROOF_DIRECTORY: &str = r"C:\ProgramData\HypercolorUpdateProof";
const PRIOR: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CANDIDATE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// What a machine-wide installer lays down: the owner stated, because an
/// elevated administrator otherwise owns what it creates on client Windows.
const INSTALLER_DIRECTORY_SDDL: &str =
    "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
const INSTALLER_FILE_SDDL: &str = "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;BU)";
const STEP_DEADLINE: Duration = Duration::from_secs(30);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn layout() -> WindowsInstallLayout {
    let (layout, _) = WindowsInstallLayout::of_bootstrap(
        &Path::new(RELEASE_ROOT).join(r"update\slot-a.exe"),
        Path::new(PROGRAM_DATA),
    )
    .expect("a slot's layout");
    layout
}

fn write_installed(directory: &WindowsDirectory, name: &str, bytes: &[u8]) {
    use std::io::Write as _;
    let name = OsStr::new(name);
    match directory.remove_file(name) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clear {name:?}: {error}"),
    }
    let mut file = directory
        .create_file_with_security(name, INSTALLER_FILE_SDDL)
        .expect("create an installed file");
    file.write_all(bytes).expect("write an installed file");
}

fn create_installed_directory(parent: &WindowsDirectory, name: &str) -> WindowsDirectory {
    parent
        .create_child_directory_with_security(OsStr::new(name), INSTALLER_DIRECTORY_SDDL)
        .expect("create an installed directory")
}

fn built(program: &str) -> Vec<u8> {
    fs::read(program).unwrap_or_else(|error| panic!("read {program}: {error}"))
}

/// The fixture example, which cargo builds beside this test.
fn fixture() -> Vec<u8> {
    let test = std::env::current_exe().expect("this test's path");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("the target profile directory");
    built(
        profile
            .join("examples")
            .join("update_proof_fixture.exe")
            .to_str()
            .expect("a UTF-8 path"),
    )
}

struct Proof {
    state: WindowsDirectory,
    bootstrap_directory: WindowsDirectory,
    layout: WindowsInstallLayout,
    log: PathBuf,
    behavior: PathBuf,
}

impl Proof {
    fn install() -> Self {
        assert!(
            !Path::new(RELEASE_ROOT).exists(),
            "{RELEASE_ROOT} exists; this proof only runs on a clean machine"
        );
        let layout = layout();
        let proof_directory = Path::new(PROOF_DIRECTORY);
        fs::create_dir_all(proof_directory).expect("the proof directory");

        // Administrators may add to Program Files but not hold what a
        // directory handle for writing asks of it, so this one is made by
        // path. It inherits what any installer's directory there inherits.
        fs::create_dir(RELEASE_ROOT).expect("the release root");
        let owner = Command::new("takeown")
            .args(["/A", "/F", RELEASE_ROOT])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .expect("run takeown");
        assert!(owner.success(), "give Administrators the release root");
        let release =
            WindowsDirectory::open(Path::new(RELEASE_ROOT), WindowsDirectoryAccess::Write)
                .expect("open the release root");
        let bootstrap_directory = create_installed_directory(&release, "update");
        let units = create_installed_directory(&release, "units");
        let bootstrap = built(env!("CARGO_BIN_EXE_hypercolor-update-bootstrap"));
        for slot in [BootstrapSlot::A, BootstrapSlot::B] {
            write_installed(&bootstrap_directory, slot.file_name(), &bootstrap);
        }
        write_installed(
            &release,
            "Hypercolor.exe",
            &built(env!("CARGO_BIN_EXE_hypercolor-launcher")),
        );
        let fixture = fixture();
        for unit in [PRIOR, CANDIDATE] {
            let directory = create_installed_directory(&units, unit);
            write_installed(&directory, "hypercolor.exe", &fixture);
            write_installed(&directory, "hypercolor-app.exe", &fixture);
        }

        let program_data =
            WindowsDirectory::open(Path::new(PROGRAM_DATA), WindowsDirectoryAccess::Write)
                .expect("open ProgramData");
        let data = create_installed_directory(&program_data, "Hypercolor");
        let state = create_installed_directory(&data, "update");
        let user = WindowsSid::current_user().expect("this user");
        let installation = json_record(&[
            ("installation_id", "7c9e6679-7425-40de-944b-e07fc1f90ae7"),
            ("user_sid", &user.to_string_form()),
            ("runtime_mode", "app_supervised"),
        ]);
        write_installed(&state, "installation.json", installation.as_bytes());

        let proof = Self {
            state,
            bootstrap_directory,
            layout,
            log: proof_directory.join("log.txt"),
            behavior: proof_directory.join("behavior.txt"),
        };
        fs::write(&proof.log, "").expect("empty the log");
        proof
    }

    /// Point the installation at `active`, inside a journal of `disposition`
    /// whose prior is [`PRIOR`] and candidate [`CANDIDATE`].
    fn records(&self, disposition: &str, active: &str) {
        let journal = format!(
            r#"{{"disposition":"{disposition}","prior_active_unit":"{PRIOR}","candidate_unit":"{CANDIDATE}"}}"#
        );
        write_installed(&self.state, "install-journal.json", journal.as_bytes());
        let pointer = format!(r#"{{"schema_version":1,"unit":"{active}"}}"#);
        write_installed(&self.state, "active.json", pointer.as_bytes());
    }

    fn behave(&self, behavior: &str) {
        fs::write(&self.behavior, behavior).expect("set the behavior");
    }

    fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .expect("read the log")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Wait until the log holds `count` lines, and return the last.
    fn await_log(&self, count: usize, what: &str) -> String {
        let deadline = Instant::now() + STEP_DEADLINE + FALLBACK_DELAY;
        loop {
            let lines = self.log_lines();
            if lines.len() >= count {
                return lines[count - 1].clone();
            }
            assert!(
                Instant::now() < deadline,
                "{what}: the log holds {lines:?}, service {:?}",
                service().query_status().map(|status| status.current_state)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Register the service on slot A the way an installer does, with
    /// slot B retained.
    fn register(&self) {
        unregister();
        let slot = self.layout.slot_image(BootstrapSlot::A);
        let deadline = Instant::now() + STEP_DEADLINE;
        loop {
            let status = Command::new(&slot)
                .arg(REGISTER_SERVICE_FLAG)
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .expect("run the slot");
            if status.success() {
                return;
            }
            // A deleted service lingers until its last handle closes.
            assert!(Instant::now() < deadline, "register: {status}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Replace a slot's bootstrap with one that fails the way `role` says.
    fn break_slot(&self, slot: BootstrapSlot, role: &str) {
        let path = Path::new(PROOF_DIRECTORY).join("broken-slot.txt");
        fs::write(path, role).expect("set the broken slot's role");
        write_installed(&self.bootstrap_directory, slot.file_name(), &fixture());
    }

    fn restore_slot(&self, slot: BootstrapSlot) {
        write_installed(
            &self.bootstrap_directory,
            slot.file_name(),
            &built(env!("CARGO_BIN_EXE_hypercolor-update-bootstrap")),
        );
    }
}

impl Drop for Proof {
    fn drop(&mut self) {
        unregister();
    }
}

fn json_record(fields: &[(&str, &str)]) -> String {
    let mut text = String::from(r#"{"schema_version":1"#);
    for (key, value) in fields {
        text.push_str(&format!(r#","{key}":"{value}""#));
    }
    text.push('}');
    text
}

fn service() -> Service {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .expect("the service manager")
        .open_service(
            UPDATE_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS
                | ServiceAccess::QUERY_CONFIG
                | ServiceAccess::START
                | ServiceAccess::STOP
                | ServiceAccess::DELETE,
        )
        .expect("the registered service")
}

fn unregister() {
    let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
    else {
        return;
    };
    let Ok(service) = manager.open_service(
        UPDATE_SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
    ) else {
        return;
    };
    let _ = service.stop();
    let _ = service.delete();
}

fn await_status(what: &str, accept: impl Fn(&ServiceStatus) -> bool) -> ServiceStatus {
    let deadline = Instant::now() + STEP_DEADLINE + FALLBACK_DELAY;
    loop {
        let status = service().query_status().expect("the service's status");
        if accept(&status) {
            return status;
        }
        assert!(Instant::now() < deadline, "{what}: {status:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn running_pid() -> u32 {
    await_status("the service runs", |status| {
        status.current_state == ServiceState::Running
    })
    .process_id
    .expect("a running service has a process")
}

fn kill(pid: u32) {
    let status = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .expect("run taskkill");
    assert!(status.success(), "kill {pid}: {status}");
}

fn activator_pid(line: &str) -> u32 {
    line.rsplit_once("pid=")
        .and_then(|(_, pid)| pid.parse().ok())
        .unwrap_or_else(|| panic!("no pid in {line:?}"))
}

fn is_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

fn await_gone(pid: u32, what: &str) {
    let deadline = Instant::now() + STEP_DEADLINE;
    while is_alive(pid) {
        assert!(Instant::now() < deadline, "{what}: {pid} still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn image_path() -> String {
    service()
        .query_config()
        .expect("the service's config")
        .executable_path
        .to_string_lossy()
        .into_owned()
}

fn run_launcher(arguments: &[&str]) {
    let launcher = Path::new(RELEASE_ROOT).join("Hypercolor.exe");
    let mut child = Command::new(launcher)
        .args(arguments)
        .spawn()
        .expect("start the launcher");
    let deadline = Instant::now() + STEP_DEADLINE;
    loop {
        if let Some(status) = child.try_wait().expect("wait for the launcher") {
            assert!(status.success(), "the launcher exited with {status}");
            return;
        }
        assert!(Instant::now() < deadline, "the launcher never exited");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// While a transaction is open, the launcher runs the prior even though the
/// pointer names the candidate. Once the journal is terminal, it follows the
/// pointer. Arguments pass through untouched.
fn launcher_reads_the_journal_before_the_pointer(proof: &Proof) {
    let seen = proof.log_lines().len();

    proof.records("forward", CANDIDATE);
    run_launcher(&["--minimized", "hypercolor://open"]);
    assert_eq!(
        proof.await_log(seen + 1, "the app ran"),
        format!("app {PRIOR} --minimized hypercolor://open")
    );

    proof.records("rollback", CANDIDATE);
    run_launcher(&[]);
    assert_eq!(
        proof.await_log(seen + 2, "the app ran").trim_end(),
        format!("app {PRIOR}")
    );

    proof.records("committed", CANDIDATE);
    run_launcher(&["--minimized"]);
    assert_eq!(
        proof.await_log(seen + 3, "the app ran"),
        format!("app {CANDIDATE} --minimized")
    );
}

/// `activate` runs from the prior, and a client that checks the pipe finds
/// the registered process behind it even though a child serves it.
fn the_registered_process_owns_the_pipe_its_activator_serves(proof: &Proof) {
    proof.register();
    proof.records("forward", CANDIDATE);
    proof.behave("activate serve\n");
    let seen = proof.log_lines().len();

    service().start(&["activate"]).expect("start activate");
    let line = proof.await_log(seen + 1, "the activator ran");
    assert!(
        line.starts_with(&format!("activator {PRIOR} activate ")),
        "{line}"
    );
    let service_pid = running_pid();
    assert_ne!(service_pid, activator_pid(&line));

    let record = fs::read(proof.layout.update_pipe_record())
        .expect("the recorded user reads the pipe record");
    let name = UpdatePipeName::decode_record(&record).expect("a pipe name");
    let mut pipe = connect_update_pipe(&WindowsUpdatePipeHost::new(&proof.layout), &name)
        .expect("the client verifies the registered process");
    assert_eq!(pipe.server_process_id(), service_pid);
    let mut greeting = [0_u8; 5];
    pipe.read_exact(&mut greeting).expect("read the activator");
    assert_eq!(&greeting, b"ready");
    drop(pipe);

    let stopped = await_status("the service stops", |status| {
        status.current_state == ServiceState::Stopped
    });
    assert_eq!(stopped.exit_code, ServiceExitCode::NO_ERROR);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        proof.log_lines().len(),
        seen + 1,
        "a clean run queues no recovery"
    );
}

/// The app's own host, telling the proof each time it waits for a run and
/// which service process it found behind each pipe, and holding its second
/// wait until the proof lets it go.
struct Watched {
    host: WindowsUpdatePipeHost,
    waits: Cell<usize>,
    waiting: mpsc::Sender<usize>,
    release: mpsc::Receiver<()>,
    checked: mpsc::Sender<Option<u32>>,
}

impl UpdatePipeHost for Watched {
    type Pipe = File;

    fn connect(&self, name: &UpdatePipeName) -> io::Result<File> {
        self.host.connect(name)
    }

    fn server_process_id(&self, pipe: &File) -> io::Result<u32> {
        self.host.server_process_id(pipe)
    }

    fn service_process_id(&self) -> io::Result<Option<u32>> {
        let service = self.host.service_process_id()?;
        let _ = self.checked.send(service);
        Ok(service)
    }

    fn read_record(&self) -> io::Result<Vec<u8>> {
        self.host.read_record()
    }

    fn wait_for_run(&self) -> io::Result<bool> {
        let wait = self.waits.get() + 1;
        self.waits.set(wait);
        let _ = self.waiting.send(wait);
        if wait == 2 {
            let _ = self.release.recv();
        }
        self.host.wait_for_run()
    }
}

/// The app follows the service from before it starts: it reaches each
/// run's pipe as the run starts, outlasts a run that asks it nothing, and
/// hears the run that asks it to quit, even when that run started and was
/// running before the app was ready to wait for it.
fn the_app_follows_the_service_until_a_run_asks_it_to_quit(proof: &Proof) {
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("prepare connect\nactivate quit\n");
    let seen = proof.log_lines().len();

    let (waiting, waits) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (checked, checks) = mpsc::channel();
    let host = Watched {
        host: WindowsUpdatePipeHost::new(&proof.layout),
        waits: Cell::new(0),
        waiting,
        release: released,
        checked,
    };
    let (done, followed) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lost = Vec::new();
        let end = follow_update_service(&host, |error| lost.push(error.to_string()));
        let _ = done.send((end.map_err(|error| error.to_string()), lost));
    });
    assert_eq!(waits.recv_timeout(STEP_DEADLINE), Ok(1), "the app waits");

    service().start(&["prepare"]).expect("start prepare");
    let prepare = proof.await_log(seen + 1, "the prepare run started");
    assert!(
        prepare.starts_with(&format!("activator {CANDIDATE} prepare ")),
        "{prepare}"
    );
    assert_eq!(
        proof.await_log(seen + 2, "the app reached the prepare run"),
        "client prepare"
    );
    let found = checks
        .recv_timeout(STEP_DEADLINE)
        .expect("the app checked the prepare run's pipe");
    assert_eq!(
        found,
        Some(running_pid()),
        "the app found the running service"
    );

    service().stop().expect("end the prepare run");
    await_status("the prepare run ends", |status| {
        status.current_state == ServiceState::Stopped
    });
    assert_eq!(
        waits.recv_timeout(STEP_DEADLINE),
        Ok(2),
        "the app left the prepare run"
    );

    // The app is held before it waits again while the next run starts and
    // runs: the schedule in which waiting for the service to stop first
    // would wait through the very run that needs the app gone.
    service().start(&["activate"]).expect("start activate");
    let activate_service = running_pid();
    assert!(
        followed.try_recv().is_err(),
        "the app has not answered a run it was held from"
    );
    release.send(()).expect("let the app wait again");

    let (end, lost) = followed
        .recv_timeout(STEP_DEADLINE)
        .expect("the app heard the activate run in time");
    assert_eq!(end, Ok(UpdateServiceEnd::Quit));
    assert!(lost.is_empty(), "no run was refused or lost: {lost:?}");
    assert_eq!(
        checks.try_iter().last(),
        Some(Some(activate_service)),
        "the app found the activate run's service behind its pipe"
    );
    let activate = proof.await_log(seen + 3, "the activate run started");
    assert!(
        activate.starts_with(&format!("activator {CANDIDATE} activate ")),
        "{activate}"
    );
    assert_eq!(
        proof.await_log(seen + 4, "the activate run asked"),
        "heard activate"
    );
    let stopped = await_status("the activate run ends", |status| {
        status.current_state == ServiceState::Stopped
    });
    assert_eq!(stopped.exit_code, ServiceExitCode::NO_ERROR);
}

/// A killed service process takes its activator with it, the service
/// manager restarts it with no argument, and a second kill brings in the
/// retained slot, which switches the service to itself and starts it.
fn killed_service_restarts_then_falls_back(proof: &Proof) {
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("recover hold\nactivate hold\n");
    let seen = proof.log_lines().len();

    service().start(&["activate"]).expect("start activate");
    let first = proof.await_log(seen + 1, "the activator ran");
    assert!(
        first.starts_with(&format!("activator {CANDIDATE} activate ")),
        "{first}"
    );
    let first_service = running_pid();
    kill(first_service);
    await_gone(
        activator_pid(&first),
        "a killed service leaves no activator",
    );

    let second = proof.await_log(seen + 2, "the failure action restarted the service");
    assert!(
        second.starts_with(&format!("activator {CANDIDATE} recover ")),
        "{second}"
    );
    let second_service = await_status("a new process runs", |status| {
        status.current_state == ServiceState::Running
            && status.process_id.is_some_and(|pid| pid != first_service)
    })
    .process_id
    .expect("a running service has a process");
    assert!(BootstrapSlot::from_image_path(OsStr::new(&image_path())) == Some(BootstrapSlot::A));

    kill(second_service);
    let third = proof.await_log(seen + 3, "the retained slot restarted the service");
    assert!(
        third.starts_with(&format!("activator {CANDIDATE} recover ")),
        "{third}"
    );
    assert_eq!(
        BootstrapSlot::from_image_path(OsStr::new(&image_path())),
        Some(BootstrapSlot::B),
        "ImagePath is the retained slot"
    );
    let actions = service_failure_command();
    assert_eq!(
        actions,
        fallback_command(&proof.layout.slot_image(BootstrapSlot::B))
            .to_string_lossy()
            .into_owned(),
        "the failure action is the retained slot"
    );

    service().stop().expect("stop the service");
    let stopped = await_status("the service stops", |status| {
        status.current_state == ServiceState::Stopped
    });
    assert_eq!(stopped.exit_code, ServiceExitCode::NO_ERROR);
    await_gone(
        activator_pid(&third),
        "a stopped service leaves no activator",
    );
}

fn service_failure_command() -> String {
    let output = Command::new("sc.exe")
        .args(["qfailure", UPDATE_SERVICE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("run sc");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.lines()
        .find_map(|line| {
            Some(
                line.split_once("COMMAND_LINE")?
                    .1
                    .split_once(':')?
                    .1
                    .trim()
                    .to_owned(),
            )
        })
        .unwrap_or_else(|| panic!("no command in {text}"))
}

/// A killed activator ends the run as a failure, and the restart recovers.
fn killed_activator_is_recovered(proof: &Proof) {
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("prepare hold\nrecover exit 0\n");
    let seen = proof.log_lines().len();

    service().start(&["prepare"]).expect("start prepare");
    let line = proof.await_log(seen + 1, "the activator ran");
    kill(activator_pid(&line));

    let recovery = proof.await_log(seen + 2, "the failure action restarted the service");
    assert!(
        recovery.starts_with(&format!("activator {CANDIDATE} recover ")),
        "{recovery}"
    );
    let stopped = await_status("recovery finishes", |status| {
        status.current_state == ServiceState::Stopped
            && status.exit_code == ServiceExitCode::NO_ERROR
    });
    assert_eq!(stopped.exit_code, ServiceExitCode::NO_ERROR);
}

/// An activator that fails ends the run with its code, and the restart
/// recovers.
fn failed_activator_is_recovered(proof: &Proof) {
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("activate exit 73\nrecover exit 0\n");
    let seen = proof.log_lines().len();

    service().start(&["activate"]).expect("start activate");
    await_status("the failed run reports its code", |status| {
        status.exit_code == ServiceExitCode::ServiceSpecific(73)
            || proof.log_lines().len() >= seen + 2
    });
    let recovery = proof.await_log(seen + 2, "the failure action restarted the service");
    assert!(recovery.contains(" recover "), "{recovery}");
}

/// A release of the bootstrap that dies in the slot `ImagePath` names loses
/// to the retained slot.
fn broken_bootstrap_falls_back(proof: &Proof, role: &str) {
    proof.restore_slot(BootstrapSlot::A);
    proof.restore_slot(BootstrapSlot::B);
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("recover exit 0\n");
    proof.break_slot(BootstrapSlot::A, role);
    let seen = proof.log_lines().len();

    let started = service().start::<&OsStr>(&[]);
    println!("{role}: start returned {started:?}");
    let line = proof.await_log(seen + 1, role);
    assert!(line.contains(" recover "), "{line}");
    assert_eq!(
        BootstrapSlot::from_image_path(OsStr::new(&image_path())),
        Some(BootstrapSlot::B)
    );
    await_status("recovery finishes", |status| {
        status.current_state == ServiceState::Stopped
    });
}

/// The service manager runs no failure action for an image that never
/// reaches it: not for one that exits first, not for one that will not load.
fn an_image_that_never_reaches_the_service_manager_is_not_recovered(proof: &Proof) {
    proof.restore_slot(BootstrapSlot::A);
    proof.restore_slot(BootstrapSlot::B);
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("recover exit 0\n");
    let seen = proof.log_lines().len();

    proof.break_slot(BootstrapSlot::A, "exit-before-start");
    let early_exit = service().start::<&OsStr>(&[]);
    assert!(early_exit.is_err(), "{early_exit:?}");
    std::thread::sleep(FALLBACK_DELAY + Duration::from_secs(5));
    assert_not_recovered(proof, seen);

    write_installed(
        &proof.bootstrap_directory,
        BootstrapSlot::A.file_name(),
        b"not a program",
    );
    let unloadable = service().start::<&OsStr>(&[]);
    assert!(unloadable.is_err(), "{unloadable:?}");
    std::thread::sleep(FALLBACK_DELAY + Duration::from_secs(5));
    assert_not_recovered(proof, seen);
}

fn assert_not_recovered(proof: &Proof, seen: usize) {
    assert_eq!(
        BootstrapSlot::from_image_path(OsStr::new(&image_path())),
        Some(BootstrapSlot::A),
        "no fallback ran"
    );
    assert_eq!(
        service()
            .query_status()
            .expect("the service's status")
            .current_state,
        ServiceState::Stopped
    );
    assert_eq!(proof.log_lines().len(), seen, "no recovery ran");
}

/// So a new bootstrap has to prove it loads before `ImagePath` may name it,
/// and one that does takes over with the proven slot retained.
fn a_bootstrap_loads_before_the_service_may_name_it(proof: &Proof) {
    proof.restore_slot(BootstrapSlot::A);
    proof.restore_slot(BootstrapSlot::B);
    proof.register();
    proof.records("committed", CANDIDATE);
    proof.behave("recover exit 0\n");
    let mut registered = ScmUpdateService::open(&proof.layout).expect("open the service");
    let bytes = Path::new(PROOF_DIRECTORY).join("not-a-program.exe");
    fs::write(&bytes, b"not a program").expect("write the unloadable bootstrap");

    install_bootstrap(&mut registered, BootstrapSlot::A, &bytes)
        .expect_err("a bootstrap that will not load is refused");
    assert_eq!(
        registered.image_slot().expect("the image"),
        Some(BootstrapSlot::A)
    );
    assert_eq!(
        service_failure_command(),
        fallback_command(&proof.layout.slot_image(BootstrapSlot::A)).to_string_lossy()
    );

    let bootstrap = Path::new(env!("CARGO_BIN_EXE_hypercolor-update-bootstrap"));
    install_bootstrap(&mut registered, BootstrapSlot::A, bootstrap).expect("install");
    assert_eq!(
        registered.image_slot().expect("the image"),
        Some(BootstrapSlot::B)
    );
    assert_eq!(
        service_failure_command(),
        fallback_command(&proof.layout.slot_image(BootstrapSlot::A)).to_string_lossy(),
        "the proven bootstrap is retained"
    );

    let seen = proof.log_lines().len();
    service()
        .start::<&OsStr>(&[])
        .expect("the new bootstrap starts");
    let line = proof.await_log(seen + 1, "the new bootstrap ran the activator");
    assert!(line.contains(" recover "), "{line}");

    await_status("recovery finishes", |status| {
        status.current_state == ServiceState::Stopped
    });
    restore_bootstrap(&mut registered, BootstrapSlot::A).expect("restore");
    assert_eq!(
        registered.image_slot().expect("the image"),
        Some(BootstrapSlot::A)
    );
}

/// Registration leaves the recorded user able to start and watch the
/// service, and nobody else who is not an administrator.
fn the_recorded_user_may_start_the_service(proof: &Proof) {
    proof.register();
    let output = Command::new("sc.exe")
        .args(["sdshow", UPDATE_SERVICE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("run sc");
    let descriptor = String::from_utf8_lossy(&output.stdout).into_owned();
    let user = WindowsSid::current_user()
        .expect("this user")
        .to_string_form();
    // SDDL prints the built-in Administrator, which runners sign in as, by
    // its alias.
    let principal = if user.ends_with("-500") { "LA" } else { &user };
    let dacl = descriptor
        .trim()
        .strip_prefix("D:")
        .and_then(|rest| rest.split("S:").next())
        .unwrap_or_else(|| panic!("no DACL in {descriptor}"));
    assert_eq!(
        dacl,
        format!(
            "(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;LCRP;;;{principal})"
        ),
        "SYSTEM, Administrators and the recorded user, and nobody else"
    );
}

#[test]
#[ignore = "installs into Program Files and registers HypercolorUpdate; see the module docs"]
fn the_service_manager_recovers_the_update_service() {
    assert!(
        std::env::var_os("HYPERCOLOR_PROVE_UPDATE_SERVICE").is_some(),
        "set HYPERCOLOR_PROVE_UPDATE_SERVICE on a disposable machine"
    );
    let proof = Proof::install();

    launcher_reads_the_journal_before_the_pointer(&proof);
    the_registered_process_owns_the_pipe_its_activator_serves(&proof);
    the_app_follows_the_service_until_a_run_asks_it_to_quit(&proof);
    killed_activator_is_recovered(&proof);
    failed_activator_is_recovered(&proof);
    killed_service_restarts_then_falls_back(&proof);
    broken_bootstrap_falls_back(&proof, "crash-after-start");
    an_image_that_never_reaches_the_service_manager_is_not_recovered(&proof);
    a_bootstrap_loads_before_the_service_may_name_it(&proof);
    the_recorded_user_may_start_the_service(&proof);
}
