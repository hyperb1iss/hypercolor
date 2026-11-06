//! The broker steps against the real service control manager and the real
//! broker.
//!
//! These register a service, so they need administrator rights, and
//! `HYPERCOLOR_TEST_BROKER_EXE` must name a built
//! `hypercolor-smbus-service.exe`. The Windows CI lane runs them.

#![cfg(windows)]

use std::ffi::OsString;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hypercolor_install::windows::{WindowsInstallExecutor, WindowsNativeExecutor};
use hypercolor_install::{InstallStore, MAX_INSTALL_JOURNAL_BYTES};
use sha2::{Digest as _, Sha256};
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const STOP_DEADLINE: Duration = Duration::from_secs(30);

/// A registered test service, stopped and deleted however the test ends.
struct RegisteredService {
    name: String,
}

impl RegisteredService {
    fn new(program: &Path) -> Self {
        let name = format!("HypercolorInstallTest{}", std::process::id());
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .expect("open the service control manager");
        manager
            .create_service(
                &ServiceInfo {
                    name: OsString::from(&name),
                    display_name: OsString::from("Hypercolor install test"),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::OnDemand,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: program.to_path_buf(),
                    launch_arguments: Vec::new(),
                    dependencies: Vec::new(),
                    account_name: None,
                    account_password: None,
                },
                ServiceAccess::QUERY_CONFIG,
            )
            .expect("register the test service");
        Self { name }
    }
}

impl Drop for RegisteredService {
    fn drop(&mut self) {
        let Ok(manager) =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        else {
            return;
        };
        let Ok(service) = manager.open_service(
            &self.name,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
        ) else {
            return;
        };
        drop(service.stop());
        let deadline = Instant::now() + STOP_DEADLINE;
        while Instant::now() < deadline
            && service
                .query_status()
                .is_ok_and(|status| status.current_state != ServiceState::Stopped)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(service.delete());
    }
}

/// A copy of the broker in its own directory, under a path that needs
/// quoting the way `%ProgramFiles%` does.
fn broker_copy(base: &Path, unit: &str, source: &Path) -> PathBuf {
    let directory = base.join("Program Files").join(unit).join("tools");
    fs::create_dir_all(&directory).expect("broker directory");
    let program = directory.join("hypercolor-smbus-service.exe");
    fs::copy(source, &program).expect("copy the broker");
    program
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

#[test]
#[ignore = "registers a service; needs administrator rights and HYPERCOLOR_TEST_BROKER_EXE"]
fn the_broker_switches_units_through_the_service_control_manager() {
    let broker = PathBuf::from(
        std::env::var_os("HYPERCOLOR_TEST_BROKER_EXE").expect("HYPERCOLOR_TEST_BROKER_EXE"),
    );
    let guard = tempfile::tempdir().expect("temporary directory");
    let base = fs::canonicalize(guard.path()).expect("canonical base");
    let base = PathBuf::from(text(&base).trim_start_matches(r"\\?\"));
    let first = broker_copy(&base, "first", &broker);
    let second = broker_copy(&base, "second", &broker);
    let digest = hex::encode(Sha256::digest(fs::read(&broker).expect("read the broker")));
    let release = base.join("release");
    let state = base.join("state");
    fs::create_dir_all(&release).expect("release root");
    fs::create_dir_all(&state).expect("state root");

    let service = RegisteredService::new(&first);
    let store =
        InstallStore::with_roots(&release, &state, MAX_INSTALL_JOURNAL_BYTES).expect("store");
    let mut executor =
        WindowsNativeExecutor::new(store, SocketAddr::from((Ipv4Addr::LOCALHOST, 1)))
            .with_broker_service(&service.name);

    let observed = executor
        .observe_broker()
        .expect("observe")
        .expect("registered");
    assert_eq!(observed.image, format!("\"{}\"", text(&first)));
    assert!(!observed.running);

    let process = executor.start_broker().expect("start and hear the broker");
    assert_eq!(
        executor.start_broker().expect("starting it again"),
        process,
        "a broker that runs is left running"
    );
    let image = executor
        .process_image(process)
        .expect("read its image")
        .expect("it runs");
    assert_eq!(image.path.to_lowercase(), text(&first).to_lowercase());
    assert_eq!(image.identity.sha256, digest);
    assert!(
        executor
            .observe_broker()
            .expect("observe")
            .expect("registered")
            .running
    );

    executor.switch_broker(text(&second)).expect("switch");
    let observed = executor
        .observe_broker()
        .expect("observe")
        .expect("registered");
    assert_eq!(observed.image, format!("\"{}\"", text(&second)));
    assert!(!observed.running, "a switch leaves the broker stopped");
    assert_eq!(
        executor.process_image(process).expect("ask after stop"),
        None
    );

    executor
        .switch_broker(text(&second))
        .expect("repeating a switch changes nothing");

    let process = executor.start_broker().expect("start the new image");
    let image = executor
        .process_image(process)
        .expect("read its image")
        .expect("it runs");
    assert_eq!(image.path.to_lowercase(), text(&second).to_lowercase());
}
