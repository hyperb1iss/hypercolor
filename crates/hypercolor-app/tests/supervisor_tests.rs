use std::path::Path;

use hypercolor_app::supervisor::{
    DEFAULT_DAEMON_BIND, SYSTEMD_USER_SERVICE, SupervisorState, SystemdUserServicePlan,
    SystemdUserServiceProbe, bind_from_daemon_url, build_daemon_command, daemon_executable_name,
    daemon_path_candidates, health_url, macos_app_resource_dir, sibling_daemon_path,
    sibling_ui_dir, startup_retry_delay, systemctl_is_active_output, systemctl_is_enabled_output,
    systemd_user_service_plan, target_triple_candidates, tauri_sidecar_daemon_name,
    ui_dir_candidates,
};
use std::time::Duration;
use url::Url;

#[test]
fn daemon_executable_name_matches_platform() {
    let name = daemon_executable_name();

    if cfg!(target_os = "windows") {
        assert_eq!(name, "hypercolor-daemon.exe");
    } else {
        assert_eq!(name, "hypercolor-daemon");
    }
}

#[test]
fn sibling_paths_resolve_from_app_executable() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };

    let daemon = sibling_daemon_path(app_path).expect("daemon path should resolve");
    assert_eq!(
        daemon.file_name().and_then(|name| name.to_str()),
        Some(daemon_executable_name())
    );

    let ui_dir = sibling_ui_dir(app_path).expect("ui path should resolve");
    assert_eq!(
        ui_dir.file_name().and_then(|name| name.to_str()),
        Some("ui")
    );
}

#[test]
fn daemon_path_candidates_include_sibling_and_resource_layouts() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\bin\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };
    let resource_dir = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor")
    } else {
        Path::new("/opt/hypercolor/resources")
    };

    let candidates = daemon_path_candidates(app_path, Some(resource_dir));

    assert!(
        candidates.contains(
            &app_path
                .parent()
                .expect("app path should have parent")
                .join(daemon_executable_name())
        )
    );
    assert!(candidates.contains(&resource_dir.join(daemon_executable_name())));
}

#[test]
fn daemon_path_candidates_include_tauri_sidecar_names() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };
    let resource_dir = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor")
    } else {
        Path::new("/opt/hypercolor/resources")
    };

    let candidates = daemon_path_candidates(app_path, Some(resource_dir));

    for target_triple in target_triple_candidates() {
        assert!(candidates.contains(&resource_dir.join(tauri_sidecar_daemon_name(target_triple))));
    }
}

#[test]
fn ui_dir_candidates_include_sibling_and_tarball_layouts() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\bin\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };

    let candidates = ui_dir_candidates(app_path, None);

    assert!(
        candidates.contains(
            &app_path
                .parent()
                .expect("app path should have parent")
                .join("ui")
        )
    );
    assert!(
        path_strings(&candidates)
            .iter()
            .any(|path| path.ends_with("share/hypercolor/ui"))
    );
}

#[test]
fn ui_dir_candidates_include_resource_dir_layouts() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };
    let resource_dir = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor")
    } else {
        Path::new("/opt/hypercolor/resources")
    };

    let candidates = ui_dir_candidates(app_path, Some(resource_dir));

    assert!(candidates.contains(&resource_dir.join("ui")));
    assert!(candidates.contains(&resource_dir.join("share").join("hypercolor").join("ui")));
}

#[test]
fn candidates_include_macos_app_resources_from_contents_macos_exe() {
    let app_path = Path::new("/Applications/Hypercolor.app/Contents/MacOS/hypercolor-app");
    let resource_dir = macos_app_resource_dir(app_path).expect("resource dir should resolve");

    assert!(normalized(&resource_dir).ends_with("Hypercolor.app/Contents/Resources"));

    let daemon_candidates = daemon_path_candidates(app_path, None);
    let ui_candidates = ui_dir_candidates(app_path, None);

    assert!(daemon_candidates.contains(&resource_dir.join(daemon_executable_name())));
    assert!(ui_candidates.contains(&resource_dir.join("ui")));
    assert!(ui_candidates.contains(&resource_dir.join("share").join("hypercolor").join("ui")));
}

#[test]
fn build_daemon_command_includes_bind_and_ui_dir() {
    let command = build_daemon_command(
        Path::new("hypercolor-daemon"),
        DEFAULT_DAEMON_BIND,
        Some(Path::new("ui")),
    );

    assert_eq!(command.program, Path::new("hypercolor-daemon"));
    assert_eq!(
        command.args,
        vec!["--bind", DEFAULT_DAEMON_BIND, "--ui-dir", "ui"]
    );
}

#[test]
fn build_daemon_command_allows_missing_ui_dir() {
    let command = build_daemon_command(Path::new("hypercolor-daemon"), DEFAULT_DAEMON_BIND, None);

    assert_eq!(command.args, vec!["--bind", DEFAULT_DAEMON_BIND]);
}

#[test]
fn bind_from_daemon_url_uses_url_host_and_port() {
    let url = Url::parse("http://127.0.0.1:9420").expect("url should parse");

    assert_eq!(
        bind_from_daemon_url(&url),
        Some(DEFAULT_DAEMON_BIND.to_owned())
    );
}

#[test]
fn bind_from_daemon_url_brackets_ipv6_hosts() {
    let url = Url::parse("http://[::1]:9420").expect("url should parse");

    assert_eq!(bind_from_daemon_url(&url), Some("[::1]:9420".to_owned()));
}

#[test]
fn health_url_targets_root_health_endpoint() {
    let url = Url::parse("http://127.0.0.1:9420/app/").expect("url should parse");

    assert_eq!(health_url(&url).as_str(), "http://127.0.0.1:9420/health");
}

#[test]
fn supervisor_state_starts_without_child_process() {
    let state = SupervisorState::default();

    assert_eq!(state.child_pid(), None);
}

#[test]
fn startup_retry_delay_caps_to_remaining_budget() {
    assert_eq!(
        startup_retry_delay(Duration::from_millis(50), Duration::from_millis(250)),
        Some(Duration::from_millis(50))
    );
    assert_eq!(
        startup_retry_delay(Duration::from_millis(500), Duration::from_millis(250)),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        startup_retry_delay(Duration::ZERO, Duration::from_millis(250)),
        None
    );
}

#[test]
fn systemd_user_service_name_matches_packaged_unit() {
    assert_eq!(SYSTEMD_USER_SERVICE, "hypercolor.service");
}

#[test]
fn systemctl_active_parser_accepts_only_active_state() {
    assert!(systemctl_is_active_output("active\n"));
    assert!(systemctl_is_active_output("\n active \n"));
    assert!(!systemctl_is_active_output("activating\n"));
    assert!(!systemctl_is_active_output("inactive\n"));
    assert!(!systemctl_is_active_output(""));
}

#[test]
fn systemctl_enabled_parser_accepts_user_managed_enabled_states() {
    assert!(systemctl_is_enabled_output("enabled\n"));
    assert!(systemctl_is_enabled_output("enabled-runtime\n"));
    assert!(systemctl_is_enabled_output("linked\n"));
    assert!(systemctl_is_enabled_output("linked-runtime\n"));
    assert!(systemctl_is_enabled_output("alias\n"));
    assert!(!systemctl_is_enabled_output("disabled\n"));
    assert!(!systemctl_is_enabled_output("static\n"));
    assert!(!systemctl_is_enabled_output("masked\n"));
}

#[test]
fn systemd_user_service_plan_prefers_systemd_when_available() {
    assert_eq!(
        systemd_user_service_plan(SystemdUserServiceProbe::Active),
        SystemdUserServicePlan::Reuse
    );
    assert_eq!(
        systemd_user_service_plan(SystemdUserServiceProbe::EnabledInactive),
        SystemdUserServicePlan::Start
    );
    assert_eq!(
        systemd_user_service_plan(SystemdUserServiceProbe::Unavailable),
        SystemdUserServicePlan::SpawnChild
    );
}

fn path_strings(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths.iter().map(|path| normalized(path)).collect()
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
