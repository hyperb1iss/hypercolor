use std::{
    fs,
    path::{Path, PathBuf},
};

use hypercolor_app::support::{
    PawnIoHelperOptions, ServiceSupportStatus, build_pawnio_helper_command,
    detect_pawnio_support_from_resource_dir, parse_sc_query_state,
    windows_daemon_service_status_from_query,
};

#[test]
fn detects_complete_bundled_pawnio_payload() {
    let resource_dir = temp_resource_dir("complete-payload");
    create_bundled_payload(&resource_dir);

    let status = detect_pawnio_support_from_resource_dir(Some(&resource_dir));

    assert_eq!(status.platform_supported, cfg!(target_os = "windows"));
    assert!(status.bundled_installer_available);
    let asset_root = normalized(
        status
            .bundled_asset_root
            .as_deref()
            .expect("asset root should be set"),
    );
    let helper_script = normalized(
        status
            .helper_script
            .as_deref()
            .expect("helper script should be set"),
    );
    let broker_executable = normalized(
        status
            .broker_executable
            .as_deref()
            .expect("broker executable should be set"),
    );

    assert!(asset_root.ends_with("tools/pawnio"));
    assert!(helper_script.ends_with("tools/install-windows-hardware-support.ps1"));
    assert!(broker_executable.ends_with("tools/hypercolor-smbus-service.exe"));
    assert!(status.bundled_modules.iter().all(|module| module.bundled));
    assert_eq!(status.install_available, cfg!(target_os = "windows"));

    cleanup_temp_resource_dir(&resource_dir);
}

#[test]
fn detects_missing_bundled_pawnio_modules() {
    let resource_dir = temp_resource_dir("missing-module");
    let tools_dir = resource_dir.join("tools");
    touch(&tools_dir.join("install-windows-hardware-support.ps1"));
    touch(&tools_dir.join("hypercolor-smbus-service.exe"));
    touch(&tools_dir.join("pawnio").join("PawnIO_setup.exe"));
    touch(
        &tools_dir
            .join("pawnio")
            .join("modules")
            .join("SmbusI801.bin"),
    );

    let status = detect_pawnio_support_from_resource_dir(Some(&resource_dir));
    let missing: Vec<_> = status
        .bundled_modules
        .iter()
        .filter(|module| !module.bundled)
        .map(|module| module.name.as_str())
        .collect();

    assert_eq!(missing, vec!["SmbusPIIX4.bin", "SmbusNCT6793.bin"]);
    assert!(!status.install_available);

    cleanup_temp_resource_dir(&resource_dir);
}

#[test]
fn pawnio_helper_command_uses_bundled_orchestrator() {
    let tools_dir = Path::new(r"C:\Program Files\Hypercolor\tools");
    let options = PawnIoHelperOptions {
        force_pawn_io: true,
        silent: true,
        reinstall_service: true,
        no_start_service: true,
    };

    let command = build_pawnio_helper_command(tools_dir, options);

    assert_eq!(command.program, PathBuf::from("powershell.exe"));
    assert!(command.args.iter().any(|arg| arg == "-File"));
    assert!(
        command
            .args
            .iter()
            .any(|arg| normalized(arg).ends_with("tools/install-windows-hardware-support.ps1"))
    );
    assert!(
        command
            .args
            .iter()
            .any(|arg| normalized(arg).ends_with("tools/pawnio"))
    );
    assert!(
        command
            .args
            .iter()
            .any(|arg| normalized(arg).ends_with("tools/hypercolor-smbus-service.exe"))
    );
    for switch in [
        "-ForcePawnIo",
        "-Silent",
        "-ReinstallService",
        "-NoStartService",
    ] {
        assert!(
            command.args.iter().any(|arg| arg == switch),
            "helper command should include {switch}"
        );
    }
}

#[test]
fn parses_service_state_from_sc_query_output() {
    let output = r"
SERVICE_NAME: HypercolorSmBus
        TYPE               : 10  WIN32_OWN_PROCESS
        STATE              : 4  RUNNING
";

    assert_eq!(parse_sc_query_state(output), Some("RUNNING".to_owned()));
}

#[test]
fn windows_daemon_service_status_recommends_reuse_only_when_running_on_windows() {
    let running =
        windows_daemon_service_status_from_query(true, service_status(true, Some("RUNNING")));
    assert_eq!(running.service_name, "Hypercolor");
    assert!(running.running);
    assert!(running.reuse_recommended);

    let stopped =
        windows_daemon_service_status_from_query(true, service_status(true, Some("STOPPED")));
    assert!(!stopped.running);
    assert!(!stopped.reuse_recommended);

    let non_windows =
        windows_daemon_service_status_from_query(false, service_status(true, Some("RUNNING")));
    assert!(!non_windows.running);
    assert!(!non_windows.reuse_recommended);
}

#[test]
fn windows_daemon_service_status_handles_missing_service() {
    let status = windows_daemon_service_status_from_query(true, service_status(false, None));

    assert!(!status.service.installed);
    assert!(!status.running);
    assert!(!status.reuse_recommended);
}

fn create_bundled_payload(resource_dir: &Path) {
    let tools_dir = resource_dir.join("tools");
    touch(&tools_dir.join("install-windows-hardware-support.ps1"));
    touch(&tools_dir.join("hypercolor-smbus-service.exe"));
    touch(&tools_dir.join("pawnio").join("PawnIO_setup.exe"));

    for module in ["SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin"] {
        touch(&tools_dir.join("pawnio").join("modules").join(module));
    }
}

fn temp_resource_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hypercolor-app-support-{name}-{}",
        std::process::id()
    ));
    cleanup_temp_resource_dir(&dir);
    fs::create_dir_all(&dir).expect("temp resource dir should be created");
    dir
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directory should be created");
    }
    fs::write(path, []).expect("test file should be written");
}

fn cleanup_temp_resource_dir(dir: &Path) {
    let temp = std::env::temp_dir();
    if dir.starts_with(&temp) && dir.exists() {
        fs::remove_dir_all(dir).expect("temp resource dir should be removable");
    }
}

fn service_status(installed: bool, state: Option<&str>) -> ServiceSupportStatus {
    ServiceSupportStatus {
        installed,
        state: state.map(str::to_owned),
    }
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/")
}
