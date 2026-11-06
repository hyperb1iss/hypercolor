//! The native executor against a real unit, real processes and real file
//! identities.

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess, WindowsSid};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::model::same_windows_path;
use super::store::InstallStore;
use super::{
    WINDOWS_APP_MEMBER, WINDOWS_BROKER_MEMBER, WINDOWS_DAEMON_MEMBER, WindowsInstallExecutor,
    WindowsNativeExecutor, WindowsOwnershipPolicy, WindowsProcessId, stage_release_payload,
};
use crate::model::{UnitId, UnitRecord};

const SHORT: Duration = Duration::from_millis(100);
const LONG: Duration = Duration::from_secs(30);
const NO_SUCH_SERVICE: &str = "HypercolorNoSuchTestService";

fn command_shell() -> Vec<u8> {
    let root = std::env::var_os("SystemRoot").expect("SystemRoot");
    fs::read(Path::new(&root).join("System32").join("cmd.exe")).expect("read cmd.exe")
}

/// A stored unit whose programs are real images: `cmd.exe` under each name,
/// so the test can run what the store holds.
struct Fixture {
    _guard: tempfile::TempDir,
    base: PathBuf,
    store: InstallStore,
    unit: UnitRecord,
    shell: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let guard = tempfile::tempdir().expect("temporary roots");
        let base = fs::canonicalize(guard.path()).expect("canonical roots");
        let base = PathBuf::from(
            base.to_str()
                .expect("UTF-8 temporary path")
                .trim_start_matches(r"\\?\"),
        );
        let release = base.join("Program Files").join("Hypercolor");
        let state = base.join("ProgramData").join("Hypercolor").join("update");
        fs::create_dir_all(&release).expect("release root");
        fs::create_dir_all(&state).expect("state root");
        let store = InstallStore::with_roots(&release, &state, 64 * 1024)
            .expect("store")
            .with_ownership_policy(WindowsOwnershipPolicy::trusting([
                WindowsSid::current_user().expect("current user"),
            ]));

        let shell = command_shell();
        let source = base.join("extracted");
        for directory in ["tools", "ui", "effects/bundled"] {
            fs::create_dir_all(source.join(directory)).expect("source directory");
        }
        let cli: &[u8] = b"cli";
        let ui: &[u8] = b"<html></html>";
        let effect: &[u8] = b"effect";
        let files: [(&str, &str, &[u8]); 6] = [
            (WINDOWS_APP_MEMBER, "program", &shell),
            (WINDOWS_DAEMON_MEMBER, "program", &shell),
            ("hypercolor.exe", "program", cli),
            (WINDOWS_BROKER_MEMBER, "program", &shell),
            ("ui/index.html", "data", ui),
            ("effects/bundled/one.html", "data", effect),
        ];
        let mut members = ["effects", "effects/bundled", "tools", "ui"]
            .iter()
            .map(|path| json!({"path": path, "type": "directory"}))
            .collect::<Vec<_>>();
        for (path, class, bytes) in files {
            fs::write(source.join(path), bytes).expect("source file");
            members.push(json!({
                "path": path,
                "type": "file",
                "class": class,
                "size": bytes.len(),
                "sha256": hex::encode(Sha256::digest(bytes)),
            }));
        }
        let manifest = serde_json::to_vec(&json!({
            "name": "hypercolor",
            "version": "1.5.0",
            "platform": "windows-x86_64",
            "rust_target": "x86_64-pc-windows-msvc",
            "programs": ["hypercolor-app.exe", "hypercolor-daemon.exe", "hypercolor.exe"],
            "members": members,
            "vendor_signed": [],
        }))
        .expect("manifest");
        fs::write(source.join("manifest.json"), &manifest).expect("write manifest");
        let id = UnitId::new(hex::encode(Sha256::digest(&manifest))).expect("digest");

        let lock = store.acquire_lock().expect("lock");
        let directory =
            WindowsDirectory::open(&source, WindowsDirectoryAccess::Read).expect("open source");
        let unit = stage_release_payload(&store, &lock, &directory, &id).expect("stage");
        store.set_active(Some(&id), &lock).expect("activate");
        drop(lock);
        Self {
            _guard: guard,
            base,
            store,
            unit,
            shell,
        }
    }

    fn executor(&self, daemon_api: SocketAddr) -> WindowsNativeExecutor {
        WindowsNativeExecutor::new(self.store.clone(), daemon_api)
            .with_broker_service(NO_SUCH_SERVICE)
    }

    fn program(&self, member: &str) -> PathBuf {
        self.store
            .unit_path(self.unit.id())
            .join(member.replace('/', r"\"))
    }
}

fn nowhere() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 1))
}

/// Run `program` until it is ended: `cmd` waiting on a pipe the test holds.
fn run(program: &Path) -> Child {
    Command::new(program)
        .args(["/d", "/q", "/k"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("run the program")
}

fn listed(executor: &mut WindowsNativeExecutor, child: &Child) -> WindowsProcessId {
    let runtime = executor.observe_runtime().expect("observe the runtime");
    runtime
        .apps
        .iter()
        .chain(&runtime.daemons)
        .find(|process| process.id.pid == child.id())
        .expect("the child is listed")
        .id
}

#[test]
fn a_unit_binds_to_its_manifest_and_its_files_on_the_volume() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());

    let binding = executor.bind_unit(&fixture.unit).expect("bind");

    assert_eq!(&binding.unit, fixture.unit.id());
    assert_eq!(binding.version, "1.5.0");
    binding.validate().expect("a well-formed binding");
    let digest = hex::encode(Sha256::digest(&fixture.shell));
    assert_eq!(binding.app.sha256, digest);
    assert_eq!(binding.daemon.sha256, digest);
    assert_eq!(binding.broker.sha256, digest);
    assert_ne!(binding.app.file_id, binding.daemon.file_id);
    assert_ne!(binding.daemon.file_id, binding.broker.file_id);
}

#[test]
fn a_running_process_proves_which_file_it_runs() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());
    let binding = executor.bind_unit(&fixture.unit).expect("bind");
    let daemon = fixture.program(WINDOWS_DAEMON_MEMBER);

    let mut child = run(&daemon);
    let id = listed(&mut executor, &child);
    let image = executor
        .process_image(id)
        .expect("read the image")
        .expect("it runs");

    assert!(
        same_windows_path(&image.path, daemon.to_str().expect("UTF-8 path")),
        "{}",
        image.path
    );
    assert_eq!(image.identity, binding.daemon);

    // The same bytes in another file hash alike and are still another file.
    let elsewhere = fixture.base.join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("directory");
    let copy = elsewhere.join(WINDOWS_DAEMON_MEMBER);
    fs::write(&copy, &fixture.shell).expect("copy");
    let mut other = run(&copy);
    let other_id = listed(&mut executor, &other);
    let other_image = executor
        .process_image(other_id)
        .expect("read the image")
        .expect("it runs");
    assert_eq!(other_image.identity.sha256, binding.daemon.sha256);
    assert_ne!(other_image.identity.file_id, binding.daemon.file_id);

    executor.terminate(&[id, other_id]).expect("end them");
    assert!(
        executor
            .wait_for_every_exit(&[id, other_id], LONG)
            .expect("wait")
    );
    child.wait().expect("reap");
    other.wait().expect("reap");
    assert_eq!(executor.process_image(id).expect("ask after exit"), None);
}

#[test]
fn process_handles_report_an_exit_as_it_happens() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());
    let mut app = run(&fixture.program(WINDOWS_APP_MEMBER));
    let mut daemon = run(&fixture.program(WINDOWS_DAEMON_MEMBER));
    let (app_id, daemon_id) = (listed(&mut executor, &app), listed(&mut executor, &daemon));

    let both = [app_id, daemon_id];
    assert!(!executor.wait_for_every_exit(&both, SHORT).expect("timed"));
    assert_eq!(
        executor.wait_for_any_exit(&both, SHORT).expect("timed"),
        None
    );

    executor.terminate(&[daemon_id]).expect("end the daemon");
    assert_eq!(
        executor.wait_for_any_exit(&both, LONG).expect("wait"),
        Some(daemon_id)
    );

    executor.terminate(&both).expect("end the rest");
    assert!(executor.wait_for_every_exit(&both, LONG).expect("wait"));
    app.wait().expect("reap");
    daemon.wait().expect("reap");
}

/// Serve one healthy `/health` on a loopback port.
fn answer_health() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 1024];
        let read = stream.read(&mut request).expect("request");
        assert!(request[..read].starts_with(b"GET /health "));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .expect("response");
    });
    address
}

#[test]
fn readiness_is_the_local_api_answering_while_the_app_lives() {
    let fixture = Fixture::new();
    let mut app = run(&fixture.program(WINDOWS_APP_MEMBER));

    let mut ready = fixture.executor(answer_health());
    let app_id = listed(&mut ready, &app);
    ready.wait_for_daemon(app_id, LONG).expect("ready");

    let mut silent = fixture.executor(nowhere());
    let refusal = silent
        .wait_for_daemon(app_id, SHORT)
        .expect_err("nothing answers");
    assert!(refusal.to_string().contains("not ready"), "{refusal}");

    silent.terminate(&[app_id]).expect("end the app");
    app.wait().expect("reap");
    let refusal = silent
        .wait_for_daemon(app_id, LONG)
        .expect_err("the app is gone");
    assert!(refusal.to_string().contains("exited"), "{refusal}");
}

#[test]
fn the_active_pointer_reads_without_the_transaction_lock() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());
    assert_eq!(
        executor.active_unit().expect("read the pointer").as_ref(),
        Some(fixture.unit.id())
    );
    let held = fixture.store.acquire_lock().expect("lock");
    assert_eq!(
        executor
            .active_unit()
            .expect("read beside the lock")
            .as_ref(),
        Some(fixture.unit.id())
    );
    fixture.store.set_active(None, &held).expect("deactivate");
    assert_eq!(executor.active_unit().expect("read the pointer"), None);
}

#[test]
fn a_broker_that_is_not_registered_reads_as_absent() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());
    assert_eq!(executor.observe_broker().expect("ask"), None);
    let refusal = executor
        .switch_broker(r"C:\a.exe")
        .expect_err("nothing to switch");
    assert!(refusal.to_string().contains("not registered"), "{refusal}");
    assert!(executor.start_broker().is_err());
}

#[test]
fn the_firewall_answers_without_administrator_rights() {
    let fixture = Fixture::new();
    let mut executor = fixture.executor(nowhere());
    executor.observe_firewall().expect("ask the firewall");
}
