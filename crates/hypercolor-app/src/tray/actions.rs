//! Pure tray action resolution for native handlers.

use std::path::PathBuf;

use hypercolor_core::config::paths::data_dir;

use crate::{DEFAULT_DAEMON_URL, logging};

use super::menu::MenuAction;

/// Local behavior resolved from a tray menu action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionTarget {
    ShowWindow,
    OpenWebUi(String),
    OpenDirectory(PathBuf),
    ShowSettings,
    Quit,
    DaemonPlaceholder,
}

/// Resolve the native target for a menu action.
#[must_use]
pub fn target_for_action(action: &MenuAction) -> ActionTarget {
    match action {
        MenuAction::ShowWindow => ActionTarget::ShowWindow,
        MenuAction::OpenWebUi => ActionTarget::OpenWebUi(daemon_url()),
        MenuAction::OpenLogsFolder => ActionTarget::OpenDirectory(logging::log_dir()),
        MenuAction::OpenUserEffectsFolder => ActionTarget::OpenDirectory(user_effects_dir()),
        MenuAction::Settings => ActionTarget::ShowSettings,
        MenuAction::Quit => ActionTarget::Quit,
        MenuAction::TogglePause
        | MenuAction::RefreshServers
        | MenuAction::StopEffect
        | MenuAction::ApplyEffect(_)
        | MenuAction::ApplyProfile(_)
        | MenuAction::SwitchServer(_) => ActionTarget::DaemonPlaceholder,
    }
}

/// Resolve the daemon UI URL used by browser-opening tray actions.
#[must_use]
pub fn daemon_url() -> String {
    std::env::var("HYPERCOLOR_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_owned())
}

/// Resolve the user-editable HTML effects directory.
#[must_use]
pub fn user_effects_dir() -> PathBuf {
    data_dir().join("effects").join("user")
}
