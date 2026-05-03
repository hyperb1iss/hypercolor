use hypercolor_app::{
    logging,
    tray::{
        actions::{self, ActionTarget},
        menu::MenuAction,
    },
};

#[test]
fn local_app_actions_resolve_to_native_targets() {
    assert_eq!(
        actions::target_for_action(&MenuAction::ShowWindow),
        ActionTarget::ShowWindow
    );
    assert!(matches!(
        actions::target_for_action(&MenuAction::OpenWebUi),
        ActionTarget::OpenWebUi(_)
    ));
    assert_eq!(
        actions::target_for_action(&MenuAction::Settings),
        ActionTarget::ShowSettings
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::Quit),
        ActionTarget::Quit
    );
}

#[test]
fn folder_actions_resolve_to_owned_local_directories() {
    assert_eq!(
        actions::target_for_action(&MenuAction::OpenLogsFolder),
        ActionTarget::OpenDirectory(logging::log_dir())
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::OpenUserEffectsFolder),
        ActionTarget::OpenDirectory(actions::user_effects_dir())
    );
}

#[test]
fn user_effects_dir_matches_core_layout() {
    let dir = actions::user_effects_dir();

    assert_eq!(dir.file_name().and_then(|name| name.to_str()), Some("user"));
    assert_eq!(
        dir.parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str()),
        Some("effects")
    );
}

#[test]
fn daemon_actions_remain_placeholders_for_client_wiring() {
    assert_eq!(
        actions::target_for_action(&MenuAction::ApplyEffect("aurora".to_owned())),
        ActionTarget::DaemonPlaceholder
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::ApplyProfile("movie".to_owned())),
        ActionTarget::DaemonPlaceholder
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::SwitchServer(1)),
        ActionTarget::DaemonPlaceholder
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::TogglePause),
        ActionTarget::DaemonPlaceholder
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::RefreshServers),
        ActionTarget::DaemonPlaceholder
    );
    assert_eq!(
        actions::target_for_action(&MenuAction::StopEffect),
        ActionTarget::DaemonPlaceholder
    );
}
