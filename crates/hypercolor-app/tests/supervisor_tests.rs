use std::path::Path;

use hypercolor_app::supervisor::{
    DEFAULT_DAEMON_BIND, DaemonCommand, HoldReason, LauncherPlan, LauncherProbe, OwnerPreference,
    SYSTEMD_USER_SERVICE, SupervisorState, SystemdUserServiceProbe, bind_from_daemon_url,
    build_daemon_command, daemon_executable_name, daemon_path_candidates, health_url,
    is_terminal_daemon_exit_code, launcher_plan, macos_app_resource_dir, restart_backoff,
    sibling_daemon_path, sibling_ui_dir, startup_retry_delay, systemctl_is_active_output,
    systemctl_is_enabled_output, target_triple_candidates, tauri_sidecar_daemon_name,
    ui_dir_candidates,
};
use hypercolor_app::support::DaemonLauncherStatus;
use hypercolor_types::service::ServiceIdentity;
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
fn restart_backoff_grows_then_saturates() {
    // Per-attempt backoff: 1, 1, 2, 5, 10, 30, 30, ...
    assert_eq!(restart_backoff(0), Duration::from_secs(1));
    assert_eq!(restart_backoff(1), Duration::from_secs(1));
    assert_eq!(restart_backoff(2), Duration::from_secs(2));
    assert_eq!(restart_backoff(3), Duration::from_secs(5));
    assert_eq!(restart_backoff(4), Duration::from_secs(10));
    assert_eq!(restart_backoff(5), Duration::from_secs(30));
    assert_eq!(restart_backoff(100), Duration::from_secs(30));
}

#[test]
fn macos_owner_conflict_is_the_only_terminal_daemon_exit_code() {
    assert_eq!(
        is_terminal_daemon_exit_code(Some(
            hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE
        )),
        cfg!(target_os = "macos")
    );
    assert!(!is_terminal_daemon_exit_code(None));
    assert!(!is_terminal_daemon_exit_code(Some(1)));
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
    let app_path = Path::new("/Applications/Hypercolor.app/Contents/MacOS/Hypercolor");
    let resource_dir = macos_app_resource_dir(app_path).expect("resource dir should resolve");

    assert!(normalized(&resource_dir).ends_with("Hypercolor.app/Contents/Resources"));

    let daemon_candidates = daemon_path_candidates(app_path, None);
    let ui_candidates = ui_dir_candidates(app_path, None);

    assert!(daemon_candidates.contains(&resource_dir.join(daemon_executable_name())));
    assert!(ui_candidates.contains(&resource_dir.join("ui")));
    assert!(ui_candidates.contains(&resource_dir.join("share").join("hypercolor").join("ui")));
}

#[test]
fn build_daemon_command_includes_bind_ui_dir_and_effects_dir() {
    let command = build_daemon_command(
        Path::new("hypercolor-daemon"),
        DEFAULT_DAEMON_BIND,
        Some(Path::new("ui")),
        Some(Path::new("effects")),
    );

    assert_eq!(command.program, Path::new("hypercolor-daemon"));
    assert_eq!(
        command.args,
        [
            "--bind",
            DEFAULT_DAEMON_BIND,
            #[cfg(target_os = "macos")]
            "--macos-owner",
            #[cfg(target_os = "macos")]
            "app-sidecar",
            "--ui-dir",
            "ui",
            "--effects-dir",
            "effects"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
    assert_eq!(
        command.environment,
        [
            (
                "HYPERCOLOR_SUPERVISED_PARENT_PID".to_owned(),
                std::process::id().to_string(),
            ),
            (
                "HYPERCOLOR_SERVICE_IDENTITY".to_owned(),
                "supervised_child".to_owned(),
            ),
            #[cfg(target_os = "macos")]
            (
                "HYPERCOLOR_MACOS_OWNER".to_owned(),
                "app-sidecar".to_owned(),
            ),
        ]
    );
}

#[test]
fn build_daemon_command_allows_missing_asset_dirs() {
    let command = build_daemon_command(
        Path::new("hypercolor-daemon"),
        DEFAULT_DAEMON_BIND,
        None,
        None,
    );

    let expected = [
        "--bind",
        DEFAULT_DAEMON_BIND,
        #[cfg(target_os = "macos")]
        "--macos-owner",
        #[cfg(target_os = "macos")]
        "app-sidecar",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    assert_eq!(command.args, expected);
    assert_eq!(
        command.environment,
        [
            (
                "HYPERCOLOR_SUPERVISED_PARENT_PID".to_owned(),
                std::process::id().to_string(),
            ),
            (
                "HYPERCOLOR_SERVICE_IDENTITY".to_owned(),
                "supervised_child".to_owned(),
            ),
            #[cfg(target_os = "macos")]
            (
                "HYPERCOLOR_MACOS_OWNER".to_owned(),
                "app-sidecar".to_owned(),
            ),
        ]
    );
}

#[test]
fn effects_dir_candidates_cover_install_layouts() {
    let current_exe = Path::new("/opt/hypercolor/bin/hypercolor-app");
    let resource_dir = Path::new("/opt/hypercolor/resources");

    let candidates =
        hypercolor_app::supervisor::effects_dir_candidates(current_exe, Some(resource_dir));

    assert!(candidates.contains(&Path::new("/opt/hypercolor/bin/effects/bundled").to_path_buf()));
    assert!(
        candidates
            .contains(&Path::new("/opt/hypercolor/share/hypercolor/effects/bundled").to_path_buf())
    );
    assert!(candidates.contains(&resource_dir.join("effects").join("bundled")));
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

fn endpoint() -> Url {
    Url::parse("http://127.0.0.1:9420/").expect("endpoint parses")
}

fn spawn_command() -> DaemonCommand {
    build_daemon_command(
        Path::new("hypercolor-daemon"),
        DEFAULT_DAEMON_BIND,
        None,
        None,
    )
}

fn plan(probe: LauncherProbe, preference: OwnerPreference) -> LauncherPlan {
    launcher_plan(&probe, &preference, &endpoint(), spawn_command())
}

#[test]
fn systemd_probe_folds_into_the_launcher_probe() {
    assert_eq!(
        LauncherProbe::from(SystemdUserServiceProbe::Active),
        LauncherProbe::online(ServiceIdentity::systemd_user())
    );
    assert_eq!(
        LauncherProbe::from(SystemdUserServiceProbe::EnabledInactive),
        LauncherProbe::startable(ServiceIdentity::systemd_user())
    );
    assert_eq!(
        LauncherProbe::from(SystemdUserServiceProbe::Unavailable),
        LauncherProbe::NOTHING
    );
}

#[test]
fn scm_status_folds_into_the_launcher_probe_with_a_stopped_start_arm() {
    let running = DaemonLauncherStatus {
        identity: Some(ServiceIdentity::windows_scm()),
        online: true,
        reuse_recommended: true,
        state: Some("RUNNING".to_owned()),
    };
    assert_eq!(
        LauncherProbe::from_launcher_status(&running),
        Some(LauncherProbe::online(ServiceIdentity::windows_scm()))
    );
    let stopped = DaemonLauncherStatus {
        identity: Some(ServiceIdentity::windows_scm()),
        online: false,
        reuse_recommended: false,
        state: Some("STOPPED".to_owned()),
    };
    assert_eq!(
        LauncherProbe::from_launcher_status(&stopped),
        Some(LauncherProbe::startable(ServiceIdentity::windows_scm()))
    );
    assert_eq!(
        LauncherProbe::from_launcher_status(&DaemonLauncherStatus::default()),
        None
    );
}

#[test]
fn flexible_plan_reuses_starts_then_spawns_on_every_platform() {
    for identity in [
        ServiceIdentity::systemd_user(),
        ServiceIdentity::systemd_system(),
        ServiceIdentity::windows_scm(),
        ServiceIdentity::launchd_direct(),
        ServiceIdentity::homebrew(),
    ] {
        assert_eq!(
            plan(
                LauncherProbe::online(identity.clone()),
                OwnerPreference::Flexible
            ),
            LauncherPlan::Reuse {
                identity: identity.clone(),
                endpoint: endpoint(),
            },
            "{identity}"
        );
        assert_eq!(
            plan(
                LauncherProbe::startable(identity.clone()),
                OwnerPreference::Flexible
            ),
            LauncherPlan::Start {
                identity: identity.clone(),
                unit: identity
                    .unit
                    .clone()
                    .expect("managed identities carry a unit"),
            },
            "{identity}"
        );
        assert_eq!(
            plan(
                LauncherProbe::offline(identity.clone()),
                OwnerPreference::Flexible
            ),
            LauncherPlan::SpawnChild {
                command: spawn_command()
            },
            "{identity}"
        );
    }
    // An unidentified daemon answering on the endpoint is reused as standalone.
    assert_eq!(
        plan(
            LauncherProbe::online(ServiceIdentity::STANDALONE),
            OwnerPreference::Flexible
        ),
        LauncherPlan::Reuse {
            identity: ServiceIdentity::STANDALONE,
            endpoint: endpoint(),
        }
    );
    assert_eq!(
        plan(LauncherProbe::NOTHING, OwnerPreference::Flexible),
        LauncherPlan::SpawnChild {
            command: spawn_command()
        }
    );
    // A startable launcher without a unit label cannot be addressed.
    let unit_less = ServiceIdentity {
        unit: None,
        ..ServiceIdentity::systemd_user()
    };
    assert_eq!(
        plan(
            LauncherProbe::startable(unit_less),
            OwnerPreference::Flexible
        ),
        LauncherPlan::SpawnChild {
            command: spawn_command()
        }
    );
}

#[test]
fn selected_owner_never_spawns_a_child() {
    for selected in [
        ServiceIdentity::launchd_direct(),
        ServiceIdentity::homebrew(),
    ] {
        assert_eq!(
            plan(
                LauncherProbe::online(selected.clone()),
                OwnerPreference::Selected(selected.clone())
            ),
            LauncherPlan::Reuse {
                identity: selected.clone(),
                endpoint: endpoint(),
            }
        );
        assert_eq!(
            plan(
                LauncherProbe::offline(selected.clone()),
                OwnerPreference::Selected(selected.clone())
            ),
            LauncherPlan::Hold {
                identity: selected.clone(),
                reason: HoldReason::SelectedOwnerOffline,
            }
        );
        // Startable is not enough: the selected owner holds until an explicit
        // remedy starts it.
        assert_eq!(
            plan(
                LauncherProbe::startable(selected.clone()),
                OwnerPreference::Selected(selected.clone())
            ),
            LauncherPlan::Hold {
                identity: selected.clone(),
                reason: HoldReason::SelectedOwnerOffline,
            }
        );
        assert_eq!(
            plan(
                LauncherProbe::online(ServiceIdentity::STANDALONE),
                OwnerPreference::Selected(selected.clone())
            ),
            LauncherPlan::Hold {
                identity: selected.clone(),
                reason: HoldReason::SelectedOwnerDisplaced,
            }
        );
        assert_eq!(
            plan(
                LauncherProbe::NOTHING,
                OwnerPreference::Selected(selected.clone())
            ),
            LauncherPlan::Hold {
                identity: selected,
                reason: HoldReason::SelectedOwnerOffline,
            }
        );
    }
}

#[test]
fn selected_owner_matches_on_launcher_not_unit_label() {
    let relabelled = ServiceIdentity {
        unit: Some("homebrew.mxcl.hypercolor-renamed".to_owned()),
        ..ServiceIdentity::homebrew()
    };
    assert_eq!(
        plan(
            LauncherProbe::online(relabelled.clone()),
            OwnerPreference::Selected(ServiceIdentity::homebrew())
        ),
        LauncherPlan::Reuse {
            identity: relabelled,
            endpoint: endpoint(),
        }
    );
}

fn path_strings(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths.iter().map(|path| normalized(path)).collect()
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
