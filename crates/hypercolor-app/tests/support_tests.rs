use std::{
    fs,
    path::{Path, PathBuf},
};

use hypercolor_app::support::{
    OPENRGB_CONFLICT_NAME, OPENRGB_PROCESS_IMAGE, PawnIoHelperOptions, ServiceSupportStatus,
    build_pawnio_helper_command, daemon_launcher_status_from_query,
    detect_pawnio_support_from_resource_dir, detect_pawnio_support_with_exclusions,
    parse_sc_query_state, parse_tasklist_pids, process_conflict,
};
use hypercolor_types::service::ServiceIdentity;

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

    assert_eq!(
        missing,
        vec![
            "SmbusPIIX4.bin",
            "SmbusNCT6793.bin",
            "IntelMSR.bin",
            "AMDFamily17.bin",
        ]
    );
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
fn daemon_launcher_status_recommends_reuse_only_when_running_on_windows() {
    let running = daemon_launcher_status_from_query(true, service_status(true, Some("RUNNING")));
    assert_eq!(running.identity, Some(ServiceIdentity::windows_scm()));
    assert_eq!(running.state.as_deref(), Some("RUNNING"));
    assert!(running.online);
    assert!(running.reuse_recommended);

    let stopped = daemon_launcher_status_from_query(true, service_status(true, Some("STOPPED")));
    assert_eq!(stopped.identity, Some(ServiceIdentity::windows_scm()));
    assert!(!stopped.online);
    assert!(!stopped.reuse_recommended);

    let non_windows =
        daemon_launcher_status_from_query(false, service_status(true, Some("RUNNING")));
    assert_eq!(non_windows.identity, None);
    assert!(!non_windows.online);
    assert!(!non_windows.reuse_recommended);
}

#[test]
fn daemon_launcher_status_handles_missing_service() {
    let status = daemon_launcher_status_from_query(true, service_status(false, None));

    assert_eq!(status.identity, None);
    assert!(!status.online);
    assert!(!status.reuse_recommended);
    assert_eq!(
        serde_json::to_value(&status).expect("status serializes"),
        serde_json::json!({
            "identity": null,
            "online": false,
            "reuseRecommended": false,
            "state": null
        })
    );
}

fn create_bundled_payload(resource_dir: &Path) {
    let tools_dir = resource_dir.join("tools");
    touch(&tools_dir.join("install-windows-hardware-support.ps1"));
    touch(&tools_dir.join("hypercolor-smbus-service.exe"));
    touch(&tools_dir.join("pawnio").join("PawnIO_setup.exe"));

    for module in [
        "SmbusI801.bin",
        "SmbusPIIX4.bin",
        "SmbusNCT6793.bin",
        "IntelMSR.bin",
        "AMDFamily17.bin",
    ] {
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

#[test]
fn tasklist_csv_yields_matching_pids_case_insensitively() {
    let output = concat!(
        "\"OpenRGB.exe\",\"4242\",\"Console\",\"1\",\"58,116 K\"\r\n",
        "\"openrgb.exe\",\"4343\",\"Console\",\"1\",\"12,000 K\"\r\n",
        "\"SignalRgb.exe\",\"9000\",\"Console\",\"1\",\"400,000 K\"\r\n",
    );

    assert_eq!(
        parse_tasklist_pids(output, OPENRGB_PROCESS_IMAGE),
        vec![4242, 4343]
    );
}

#[test]
fn tasklist_no_tasks_message_yields_nothing() {
    let output = "INFO: No tasks are running which match the specified criteria.\r\n";

    assert!(parse_tasklist_pids(output, OPENRGB_PROCESS_IMAGE).is_empty());
    assert!(parse_tasklist_pids("", OPENRGB_PROCESS_IMAGE).is_empty());
}

#[test]
fn managed_openrgb_pid_is_not_reported_as_a_conflict() {
    let managed = [4242];

    assert_eq!(
        process_conflict(
            OPENRGB_CONFLICT_NAME,
            OPENRGB_PROCESS_IMAGE,
            &[4242],
            &managed
        ),
        None
    );
    assert_eq!(
        process_conflict(OPENRGB_CONFLICT_NAME, OPENRGB_PROCESS_IMAGE, &[], &[]),
        None
    );
}

#[test]
fn a_foreign_openrgb_beside_the_managed_one_is_still_a_conflict() {
    let conflict = process_conflict(
        OPENRGB_CONFLICT_NAME,
        OPENRGB_PROCESS_IMAGE,
        &[4242, 5151],
        &[4242],
    )
    .expect("foreign pid should surface");

    assert_eq!(conflict.name, "OpenRGB");
    assert_eq!(conflict.identifier, "OpenRGB.exe");
    assert!(conflict.running);
}

#[test]
fn detect_with_exclusions_matches_the_plain_detector_off_windows() {
    let resource_dir = temp_resource_dir("exclusions-parity");
    create_bundled_payload(&resource_dir);

    let plain = detect_pawnio_support_from_resource_dir(Some(&resource_dir));
    let excluded = detect_pawnio_support_with_exclusions(Some(&resource_dir), &[4242]);

    assert_eq!(plain.conflicting_rgb_tools, excluded.conflicting_rgb_tools);
    assert_eq!(plain.install_available, excluded.install_available);
    cleanup_temp_resource_dir(&resource_dir);
}
