//! Native tray integration for the unified desktop app.

use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc},
    thread,
};

use tauri::{
    AppHandle, Manager, Runtime,
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    daemon_client::DaemonClient,
    state::{AppState, DaemonMessage, TrayCommand},
    window,
};

pub mod actions;
pub mod icons;
pub mod menu;

const TRAY_ID: &str = "main";

/// Shared native tray state managed by Tauri.
#[derive(Clone, Default)]
pub struct TrayRuntime {
    command_tx: Arc<Mutex<Option<UnboundedSender<TrayCommand>>>>,
}

impl TrayRuntime {
    /// Attach the daemon command sender used by native menu events.
    pub fn set_command_sender(&self, command_tx: UnboundedSender<TrayCommand>) {
        *self.command_guard() = Some(command_tx);
    }

    /// Dispatch a daemon command from a native menu event.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon client has not started or its command
    /// channel has closed.
    pub fn dispatch_command(&self, command: TrayCommand) -> anyhow::Result<()> {
        let Some(command_tx) = self.command_guard().as_ref().cloned() else {
            anyhow::bail!("daemon client command channel is not ready");
        };
        command_tx
            .send(command)
            .map_err(|_| anyhow::anyhow!("daemon client command channel is closed"))
    }

    fn command_guard(&self) -> MutexGuard<'_, Option<UnboundedSender<TrayCommand>>> {
        self.command_tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Register the native tray icon and its event handlers.
///
/// # Errors
///
/// Returns a Tauri error if native tray or menu construction fails.
pub fn register<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let state = AppState::disconnected();
    let tray_menu = menu::build_menu(app, &state)?;
    let icon = icons::build_icon(icons::icon_state_for(&state));

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip_for(&state))
        .icon(icon)
        .menu(&tray_menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_event)
        .build(app)?;

    start_daemon_client(app);

    Ok(tray)
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
        && let Err(error) = window::toggle_main(tray.app_handle())
    {
        tracing::warn!(%error, "failed to toggle main window from tray");
    }
}

fn run_menu_action<R: Runtime>(app: &AppHandle<R>, action: menu::MenuAction) -> anyhow::Result<()> {
    match actions::target_for_action(&action) {
        actions::ActionTarget::ShowWindow => window::show_main(app)?,
        actions::ActionTarget::OpenWebUi(url) => {
            open::that_detached(url)?;
        }
        actions::ActionTarget::OpenDirectory(path) => {
            open_or_create_dir(&path)?;
        }
        actions::ActionTarget::ShowSettings => window::show_settings(app)?,
        actions::ActionTarget::Quit => app.exit(0),
        actions::ActionTarget::DaemonCommand(command) => {
            app.state::<TrayRuntime>()
                .inner()
                .dispatch_command(command)?;
        }
    }
    Ok(())
}

fn start_daemon_client<R: Runtime>(app: &AppHandle<R>) {
    let (message_tx, message_rx) = mpsc::channel::<DaemonMessage>();
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();

    app.state::<TrayRuntime>()
        .inner()
        .set_command_sender(command_tx);

    tauri::async_runtime::spawn(async move {
        DaemonClient::new(message_tx, command_rx).run().await;
    });

    start_message_pump(app.clone(), message_rx);
}

fn start_message_pump<R: Runtime>(app: AppHandle<R>, message_rx: mpsc::Receiver<DaemonMessage>) {
    let spawn_result = thread::Builder::new()
        .name("hypercolor-tray-state".to_owned())
        .spawn(move || {
            let mut state = AppState::disconnected();
            for message in message_rx {
                state.apply_daemon_message(message);
                let snapshot = state.clone();
                let app_handle = app.clone();
                if let Err(error) = app.run_on_main_thread(move || {
                    if let Err(error) = refresh_tray(&app_handle, &snapshot) {
                        tracing::warn!(%error, "failed to refresh tray state");
                    }
                }) {
                    tracing::warn!(%error, "failed to schedule tray refresh");
                }
            }
        });

    if let Err(error) = spawn_result {
        tracing::warn!(%error, "failed to start tray state thread");
    }
}

fn refresh_tray<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    let tray_menu = menu::build_menu(app, state)?;
    let icon = icons::build_icon(icons::icon_state_for(state));
    tray.set_menu(Some(tray_menu))?;
    tray.set_icon(Some(icon))?;
    tray.set_tooltip(Some(tooltip_for(state)))?;
    Ok(())
}

fn open_or_create_dir(path: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    open::that_detached(path)?;
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
