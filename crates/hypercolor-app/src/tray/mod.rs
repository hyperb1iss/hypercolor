//! Native tray integration for the unified desktop app.

use std::path::Path;

use hypercolor_core::config::paths::data_dir;
use tauri::{
    AppHandle, Manager, Runtime,
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

use crate::{DEFAULT_DAEMON_URL, state::AppState};

pub mod icons;
pub mod menu;

const TRAY_ID: &str = "main";
const WINDOW_LABEL: &str = "main";

/// Register the native tray icon and its event handlers.
///
/// # Errors
///
/// Returns a Tauri error if native tray or menu construction fails.
pub fn register<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let state = AppState::disconnected();
    let tray_menu = menu::build_menu(app, &state)?;
    let icon = icons::build_icon(icons::icon_state_for(&state));

    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip_for(&state))
        .icon(icon)
        .menu(&tray_menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_event)
        .build(app)
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();

    let Some(action) = menu::action_for_menu_id(id) else {
        return;
    };

    if let Err(error) = run_menu_action(app, action) {
        tracing::warn!(%error, id, "failed to handle tray menu action");
    }
}

fn handle_tray_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    if should_toggle_window(&event)
        && let Err(error) = toggle_main_window(tray.app_handle())
    {
        tracing::warn!(%error, "failed to toggle main window from tray");
    }
}

fn run_menu_action<R: Runtime>(app: &AppHandle<R>, action: menu::MenuAction) -> anyhow::Result<()> {
    match action {
        menu::MenuAction::ShowWindow | menu::MenuAction::Settings => show_main_window(app)?,
        menu::MenuAction::OpenWebUi => {
            open::that_detached(daemon_url())?;
        }
        menu::MenuAction::OpenLogsFolder => {
            open_or_create_dir(&data_dir().join("logs"))?;
        }
        menu::MenuAction::OpenUserEffectsFolder => {
            open_or_create_dir(&data_dir().join("effects").join("user"))?;
        }
        menu::MenuAction::Quit => app.exit(0),
        menu::MenuAction::TogglePause
        | menu::MenuAction::RefreshServers
        | menu::MenuAction::StopEffect
        | menu::MenuAction::ApplyEffect(_)
        | menu::MenuAction::ApplyProfile(_)
        | menu::MenuAction::SwitchServer(_) => {
            tracing::debug!(?action, "daemon menu action queued for client wiring");
        }
    }
    Ok(())
}

fn daemon_url() -> String {
    std::env::var("HYPERCOLOR_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_owned())
}

fn open_or_create_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    open::that_detached(path)?;
    Ok(())
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if window.is_visible()? {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}

fn should_toggle_window(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

fn tooltip_for(state: &AppState) -> String {
    if state.connected {
        let effect_label = state
            .current_effect
            .as_ref()
            .map_or("No effect", |effect| effect.name.as_str());
        match &state.active_scene_name {
            Some(scene) if state.scene_snapshot_locked => {
                format!("Hypercolor - {effect_label} [{scene} snap]")
            }
            Some(scene) => format!("Hypercolor - {effect_label} [{scene}]"),
            None => format!("Hypercolor - {effect_label}"),
        }
    } else {
        "Hypercolor - Disconnected".to_owned()
    }
}
