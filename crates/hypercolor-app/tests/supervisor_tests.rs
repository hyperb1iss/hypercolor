use std::path::Path;

use hypercolor_app::supervisor::{
    DEFAULT_DAEMON_BIND, SupervisorState, bind_from_daemon_url, build_daemon_command,
    daemon_executable_name, health_url, sibling_daemon_path, sibling_ui_dir, ui_dir_candidates,
};
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
fn ui_dir_candidates_include_tauri_and_tarball_layouts() {
    let app_path = if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\Hypercolor\bin\hypercolor-app.exe")
    } else {
        Path::new("/opt/hypercolor/bin/hypercolor-app")
    };

    let candidates = ui_dir_candidates(app_path);
    let candidate_strings = candidates
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>();

    assert!(
        candidate_strings
            .iter()
            .any(|path| path.ends_with("bin\\ui")
                || path.ends_with("bin/ui")
                || path.ends_with("Hypercolor\\ui"))
    );
    assert!(
        candidate_strings
            .iter()
            .any(|path| path.replace('\\', "/").ends_with("share/hypercolor/ui"))
    );
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
