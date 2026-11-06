#![cfg(unix)]

use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use hypercolor_macos_owner::{
    MACOS_APP_CONTROL_SOCKET_NAME, MACOS_APP_RELAUNCH_FILE_NAME, MACOS_UPDATE_PENDING_FILE_NAME,
    MacosAppControl, MacosAppControlServer, MacosAppHost, MacosAppStartupHold,
    MacosOwnerExecutionError, MacosRunningApp, macos_app_bundle_of, macos_app_startup_hold,
    macos_update_state_root, wait_for_macos_update_end,
};

const TIMEOUT: Duration = Duration::from_secs(5);
const TRANSLOCATED: &str = "/private/var/folders/ab/cdef/T/AppTranslocation/0F3A/d/Hypercolor.app";

#[derive(Default)]
struct FakeHost {
    outlives: bool,
    refuses: bool,
    /// The process dies at this point of its run.
    crash: Option<Crash>,
    opened: RefCell<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crash {
    WaitingForTheExit,
    AfterOpening,
}

impl MacosAppHost for FakeHost {
    fn wait_for_exit(
        &mut self,
        _pid: u32,
        _timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        assert!(
            self.crash != Some(Crash::WaitingForTheExit),
            "the updater died"
        );
        Ok(!self.outlives)
    }

    fn open_bundle(&mut self, bundle: &Path) -> Result<(), MacosOwnerExecutionError> {
        if self.refuses {
            return Err(MacosOwnerExecutionError::new("LaunchServices refused"));
        }
        self.opened.borrow_mut().push(bundle.to_path_buf());
        assert!(self.crash != Some(Crash::AfterOpening), "the updater died");
        Ok(())
    }
}

fn running_app(bundle: &str) -> MacosRunningApp {
    MacosRunningApp {
        pid: 4242,
        bundle: PathBuf::from(bundle),
    }
}

/// A running app serving `root` on its own thread until it is told to quit.
fn serve(root: &Path, app: MacosRunningApp) -> thread::JoinHandle<std::io::Result<()>> {
    let server = MacosAppControlServer::bind(root).expect("bind the control socket");
    thread::spawn(move || server.serve_until_quit(&app))
}

#[test]
fn the_update_state_root_lives_in_the_accounts_application_support() {
    assert_eq!(
        macos_update_state_root(Path::new("/Users/player")),
        Path::new("/Users/player/Library/Application Support/Hypercolor/update")
    );
}

#[test]
fn a_bundle_is_read_only_from_an_executable_inside_one() {
    assert_eq!(
        macos_app_bundle_of(Path::new(
            "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-app"
        )),
        Some(PathBuf::from("/Applications/Hypercolor.app"))
    );
    for loose in [
        "/usr/local/bin/hypercolor-app",
        "/Applications/Hypercolor.app/hypercolor-app",
        "/Applications/Hypercolor/Contents/MacOS/hypercolor-app",
    ] {
        assert_eq!(macos_app_bundle_of(Path::new(loose)), None, "{loose}");
    }
}

#[test]
fn startup_holds_for_a_translocated_app_and_an_open_update() {
    let root = tempfile::tempdir().expect("root");
    let installed = Path::new("/Applications/Hypercolor.app/Contents/MacOS/hypercolor-app");
    let translocated = PathBuf::from(TRANSLOCATED).join("Contents/MacOS/hypercolor-app");
    assert_eq!(macos_app_startup_hold(installed, root.path()), None);
    assert_eq!(
        macos_app_startup_hold(&translocated, root.path()),
        Some(MacosAppStartupHold::MoveToApplications)
    );
    std::fs::write(root.path().join(MACOS_UPDATE_PENDING_FILE_NAME), b"").expect("pending");
    assert_eq!(
        macos_app_startup_hold(installed, root.path()),
        Some(MacosAppStartupHold::UpdateInProgress)
    );
    assert_eq!(
        macos_app_startup_hold(&translocated, root.path()),
        Some(MacosAppStartupHold::MoveToApplications),
        "a copy that can never proceed does not wait for the update"
    );
}

#[test]
fn the_wait_for_an_update_ends_with_its_marker() {
    let root = tempfile::tempdir().expect("root");
    let marker = root.path().join(MACOS_UPDATE_PENDING_FILE_NAME);
    wait_for_macos_update_end(root.path()).expect("no update is open");

    std::fs::write(&marker, b"").expect("pending");
    let waiting = {
        let root = root.path().to_path_buf();
        thread::spawn(move || wait_for_macos_update_end(&root))
    };
    thread::sleep(Duration::from_millis(100));
    assert!(!waiting.is_finished(), "the update is still open");
    std::fs::remove_file(&marker).expect("terminal");
    waiting
        .join()
        .expect("waiter")
        .expect("the removal ends the wait");
}

#[test]
fn a_quit_records_the_bundle_first_and_a_relaunch_opens_exactly_it() {
    let root = tempfile::tempdir().expect("root");
    let app = running_app("/Applications/Hypercolor.app");
    let serving = serve(root.path(), app.clone());
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();

    assert_eq!(
        control
            .quit_for_update(&mut host, TIMEOUT)
            .expect("quit")
            .as_ref(),
        Some(&app)
    );
    serving.join().expect("server").expect("the app quits");
    assert!(root.path().join(MACOS_APP_RELAUNCH_FILE_NAME).is_file());
    assert!(
        !root.path().join(MACOS_APP_CONTROL_SOCKET_NAME).exists(),
        "a clean exit removes its socket"
    );
    assert!(host.opened.borrow().is_empty(), "nothing relaunched yet");

    assert_eq!(
        control.relaunch_recorded(&mut host).expect("relaunch"),
        Some(app.bundle.clone())
    );
    assert_eq!(*host.opened.borrow(), std::slice::from_ref(&app.bundle));
    assert!(!root.path().join(MACOS_APP_RELAUNCH_FILE_NAME).exists());
    assert_eq!(
        control.relaunch_recorded(&mut host).expect("repeat"),
        None,
        "a relaunch happens once"
    );
}

#[test]
fn a_crash_anywhere_after_the_record_still_brings_the_app_back() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let root = tempfile::tempdir().expect("root");
    let app = running_app("/Applications/Hypercolor.app");
    let serving = serve(root.path(), app.clone());
    let control = MacosAppControl::new(root.path());

    let mut dying = FakeHost {
        crash: Some(Crash::WaitingForTheExit),
        ..FakeHost::default()
    };
    assert!(
        catch_unwind(AssertUnwindSafe(
            || control.quit_for_update(&mut dying, TIMEOUT)
        ))
        .is_err()
    );
    serving.join().expect("server").expect("the app quits");

    let mut dying = FakeHost {
        crash: Some(Crash::AfterOpening),
        ..FakeHost::default()
    };
    assert!(catch_unwind(AssertUnwindSafe(|| control.relaunch_recorded(&mut dying))).is_err());
    assert_eq!(
        *dying.opened.borrow(),
        std::slice::from_ref(&app.bundle),
        "the first attempt opened it"
    );

    let mut host = FakeHost::default();
    assert_eq!(
        control.relaunch_recorded(&mut host).expect("relaunch"),
        Some(app.bundle.clone()),
        "the record outlives the crash, and opening a running app only raises it"
    );
    assert_eq!(control.relaunch_recorded(&mut host).expect("repeat"), None);
}

#[test]
fn no_app_means_nothing_to_quit_or_relaunch() {
    let root = tempfile::tempdir().expect("root");
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();
    assert_eq!(
        control.quit_for_update(&mut host, TIMEOUT).expect("none"),
        None
    );

    // The socket a crashed app left has no listener behind it.
    let socket = root.path().join(MACOS_APP_CONTROL_SOCKET_NAME);
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("a crashed app's socket"));
    assert!(socket.exists());
    assert_eq!(
        control.quit_for_update(&mut host, TIMEOUT).expect("none"),
        None
    );
    assert_eq!(control.relaunch_recorded(&mut host).expect("none"), None);

    // The next app claims the socket over the leftover.
    let serving = serve(root.path(), running_app("/Applications/Hypercolor.app"));
    assert!(
        control
            .quit_for_update(&mut host, TIMEOUT)
            .expect("quit")
            .is_some()
    );
    serving.join().expect("server").expect("the app quits");
}

#[test]
fn the_socket_is_private_to_the_account() {
    let root = tempfile::tempdir().expect("root");
    let server = MacosAppControlServer::bind(root.path()).expect("bind");
    let mode = std::fs::metadata(root.path().join(MACOS_APP_CONTROL_SOCKET_NAME))
        .expect("socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    drop(server);
}

#[test]
fn a_translocated_app_quits_and_is_never_recorded() {
    let root = tempfile::tempdir().expect("root");
    let serving = serve(root.path(), running_app(TRANSLOCATED));
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();
    assert!(
        control
            .quit_for_update(&mut host, TIMEOUT)
            .expect("quit")
            .is_some()
    );
    serving.join().expect("server").expect("the app quits");
    assert!(!root.path().join(MACOS_APP_RELAUNCH_FILE_NAME).exists());
    assert_eq!(control.relaunch_recorded(&mut host).expect("none"), None);
}

#[test]
fn an_app_that_outlives_its_quit_is_an_error_and_keeps_its_record() {
    let root = tempfile::tempdir().expect("root");
    let serving = serve(root.path(), running_app("/Applications/Hypercolor.app"));
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost {
        outlives: true,
        ..FakeHost::default()
    };
    let error = control
        .quit_for_update(&mut host, TIMEOUT)
        .expect_err("still running");
    assert!(error.to_string().contains("did not quit"), "{error}");
    serving.join().expect("server").expect("acknowledged");
    assert!(root.path().join(MACOS_APP_RELAUNCH_FILE_NAME).is_file());
}

#[test]
fn a_refused_relaunch_keeps_its_record_for_the_next_attempt() {
    let root = tempfile::tempdir().expect("root");
    let serving = serve(root.path(), running_app("/Applications/Hypercolor.app"));
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();
    control.quit_for_update(&mut host, TIMEOUT).expect("quit");
    serving.join().expect("server").expect("the app quits");

    host.refuses = true;
    assert!(control.relaunch_recorded(&mut host).is_err());
    assert!(root.path().join(MACOS_APP_RELAUNCH_FILE_NAME).is_file());
    host.refuses = false;
    assert!(
        control
            .relaunch_recorded(&mut host)
            .expect("relaunch")
            .is_some()
    );
}

#[test]
fn a_recorded_path_that_is_not_an_exact_app_bundle_is_never_opened() {
    let root = tempfile::tempdir().expect("root");
    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();
    for bundle in [
        TRANSLOCATED,
        "Hypercolor.app",
        "/Applications/../Hypercolor.app",
        "/Applications/Hypercolor",
    ] {
        std::fs::write(
            root.path().join(MACOS_APP_RELAUNCH_FILE_NAME),
            serde_json::json!({ "bundle": bundle }).to_string(),
        )
        .expect("record");
        assert!(control.relaunch_recorded(&mut host).is_err(), "{bundle}");
    }
    assert!(host.opened.borrow().is_empty());
}

#[test]
fn a_client_that_misbehaves_loses_only_its_connection() {
    use std::io::Write as _;

    let root = tempfile::tempdir().expect("root");
    let serving = serve(root.path(), running_app("/Applications/Hypercolor.app"));
    let socket = root.path().join(MACOS_APP_CONTROL_SOCKET_NAME);
    let mut garbled = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
    garbled.write_all(b"\"reboot\"\n").expect("write");
    drop(garbled);
    drop(std::os::unix::net::UnixStream::connect(&socket).expect("connect"));

    let control = MacosAppControl::new(root.path());
    let mut host = FakeHost::default();
    assert!(
        control
            .quit_for_update(&mut host, TIMEOUT)
            .expect("quit")
            .is_some()
    );
    serving.join().expect("server").expect("the app quits");
}
