//! Tray menu construction and update logic.
//!
//! Builds the tray context menu using the `muda` types re-exported by
//! `tray-icon` and provides functions to rebuild the menu when applet
//! state changes (new effect, brightness, etc.).

use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::state::AppState;

/// Well-known menu item IDs for dispatching click events.
pub mod ids {
    pub const OPEN_WEB_UI: &str = "open_web_ui";
    pub const PAUSE_RESUME: &str = "pause_resume";
    pub const REFRESH_SERVERS: &str = "refresh_servers";
    pub const STOP_EFFECT: &str = "stop_effect";
    pub const QUIT: &str = "quit";

    /// Prefix for dynamically generated effect menu items.
    pub const EFFECT_PREFIX: &str = "effect:";

    /// Prefix for dynamically generated profile menu items.
    pub const PROFILE_PREFIX: &str = "profile:";

    /// Prefix for dynamically generated server items.
    pub const SERVER_PREFIX: &str = "server:";
}

/// Build the complete tray menu for the current application state.
///
/// # Errors
///
/// Returns an error if menu construction fails (platform API errors).
pub fn build_menu(state: &AppState) -> anyhow::Result<Menu> {
    let menu = Menu::new();

    // Header
    let header_text = if state.connected {
        "Hypercolor"
    } else {
        "Hypercolor (Disconnected)"
    };
    let header = MenuItem::with_id(
        MenuId::new(header_text),
        header_text,
        false, // disabled
        None,
    );
    menu.append(&header)?;
    menu.append(&PredefinedMenuItem::separator())?;

    if state.connected {
        build_connected_menu(&menu, state)?;
    } else {
        build_disconnected_menu(&menu, state)?;
    }

    menu.append(&PredefinedMenuItem::separator())?;

    // Quit
    let quit = MenuItem::with_id(MenuId::new(ids::QUIT), "Quit", true, None);
    menu.append(&quit)?;

    Ok(menu)
}

/// Build menu items shown when connected to the daemon.
fn build_connected_menu(menu: &Menu, state: &AppState) -> anyhow::Result<()> {
    // Current effect label
    let effect_label = match &state.current_effect {
        Some(effect) => format!("\u{25b6} {}", effect.name),
        None => "No effect active".to_owned(),
    };
    let current_effect = MenuItem::with_id(
        MenuId::new("current_effect"),
        &effect_label,
        false, // disabled label
        None,
    );
    menu.append(&current_effect)?;
    if let Some(scene_name) = &state.active_scene_name {
        let scene_suffix = if state.scene_snapshot_locked {
            " [snap]"
        } else {
            ""
        };
        let scene_label = format!("Scene: {scene_name}{scene_suffix}");
        let current_scene =
            MenuItem::with_id(MenuId::new("current_scene"), &scene_label, false, None);
        menu.append(&current_scene)?;
    }
    menu.append(&PredefinedMenuItem::separator())?;

    // Effects submenu
    if !state.effects.is_empty() {
        let effects_submenu = Submenu::new("Effects", true);
        for effect in &state.effects {
            let item_id = format!("{}{}", ids::EFFECT_PREFIX, effect.id);
            let item = MenuItem::with_id(MenuId::new(&item_id), &effect.name, true, None);
            effects_submenu.append(&item)?;
        }
        menu.append(&effects_submenu)?;
    }

    // Profiles submenu
    if !state.profiles.is_empty() {
        let profiles_submenu = Submenu::new("Profiles", true);
        for profile in &state.profiles {
            let item_id = format!("{}{}", ids::PROFILE_PREFIX, profile.id);
            let item = MenuItem::with_id(MenuId::new(&item_id), &profile.name, true, None);
            profiles_submenu.append(&item)?;
        }
        menu.append(&profiles_submenu)?;
    }

    if should_show_servers_menu(state) {
        menu.append(&build_servers_submenu(state)?)?;
    }

    menu.append(&PredefinedMenuItem::separator())?;

    // Brightness label
    let brightness_label = format!("Brightness: {}%", state.brightness);
    let brightness = MenuItem::with_id(
        MenuId::new("brightness"),
        &brightness_label,
        false, // disabled label (informational)
        None,
    );
    menu.append(&brightness)?;

    // Pause/Resume toggle
    let pause_label = if state.paused { "Resume" } else { "Pause" };
    let pause_item = MenuItem::with_id(MenuId::new(ids::PAUSE_RESUME), pause_label, true, None);
    menu.append(&pause_item)?;

    // Stop effect (only when an effect is active)
    if state.current_effect.is_some() {
        let stop_item = MenuItem::with_id(MenuId::new(ids::STOP_EFFECT), "Stop Effect", true, None);
        menu.append(&stop_item)?;
    }

    menu.append(&PredefinedMenuItem::separator())?;

    // Open Web UI
    let open_ui = MenuItem::with_id(MenuId::new(ids::OPEN_WEB_UI), "Open Web UI", true, None);
    menu.append(&open_ui)?;

    Ok(())
}

/// Build menu items shown when disconnected from the daemon.
fn build_disconnected_menu(menu: &Menu, state: &AppState) -> anyhow::Result<()> {
    let status = MenuItem::with_id(
        MenuId::new("status_disconnected"),
        "Daemon not reachable",
        false,
        None,
    );
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator())?;

    if should_show_servers_menu(state) {
        menu.append(&build_servers_submenu(state)?)?;
        menu.append(&PredefinedMenuItem::separator())?;
    }

    let open_ui = MenuItem::with_id(MenuId::new(ids::OPEN_WEB_UI), "Open Web UI", true, None);
    menu.append(&open_ui)?;

    Ok(())
}

fn build_servers_submenu(state: &AppState) -> anyhow::Result<Submenu> {
    let submenu = Submenu::new("Servers", true);

    for (index, entry) in state.servers.iter().enumerate() {
        let active_prefix = if state.active_server == Some(index) {
            "\u{25cf} "
        } else {
            ""
        };
        let key_suffix = if entry.server.auth_required && !entry.has_api_key {
            " (Key required)"
        } else {
            ""
        };
        let label = format!(
            "{active_prefix}{} ({}){key_suffix}",
            entry.server.identity.instance_name, entry.server.host
        );
        let item_id = format!("{}{}", ids::SERVER_PREFIX, index);
        let item = MenuItem::with_id(MenuId::new(&item_id), &label, true, None);
        submenu.append(&item)?;
    }

    submenu.append(&PredefinedMenuItem::separator())?;
    let refresh = MenuItem::with_id(
        MenuId::new(ids::REFRESH_SERVERS),
        "Refresh Servers",
        true,
        None,
    );
    submenu.append(&refresh)?;

    Ok(submenu)
}

fn should_show_servers_menu(state: &AppState) -> bool {
    state.servers.len() > 1
}
