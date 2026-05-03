//! Tray menu construction for the unified desktop app.
//!
//! The menu is represented as a pure model first so state-dependent behavior
//! can be tested without creating a native Tauri runtime.

use tauri::{
    Manager, Runtime,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
};

use crate::state::AppState;

/// Well-known menu item IDs for dispatching click events.
pub mod ids {
    pub const SHOW_WINDOW: &str = "show_window";
    pub const OPEN_WEB_UI: &str = "open_web_ui";
    pub const OPEN_LOGS_FOLDER: &str = "open_logs_folder";
    pub const OPEN_USER_EFFECTS_FOLDER: &str = "open_user_effects_folder";
    pub const SETTINGS: &str = "settings";
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

/// Platform-neutral tray menu description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable or disabled menu item.
    Item(MenuItemModel),
    /// A nested submenu.
    Submenu(SubmenuModel),
    /// A visual separator.
    Separator,
}

/// Platform-neutral menu item description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemModel {
    /// Stable ID emitted by native menu events.
    pub id: String,
    /// Visible item label.
    pub label: String,
    /// Whether the user can activate the item.
    pub enabled: bool,
}

/// Platform-neutral submenu description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmenuModel {
    /// Visible submenu label.
    pub label: String,
    /// Child entries.
    pub entries: Vec<MenuEntry>,
}

impl MenuItemModel {
    fn new(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled,
        }
    }
}

impl SubmenuModel {
    fn new(label: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            entries,
        }
    }
}

/// Build the platform-neutral menu model for the current application state.
#[must_use]
pub fn menu_model(state: &AppState) -> Vec<MenuEntry> {
    let mut entries = Vec::new();

    let header_text = if state.connected {
        "Hypercolor"
    } else {
        "Hypercolor (Disconnected)"
    };
    entries.push(item("header", header_text, false));
    entries.push(MenuEntry::Separator);

    if state.connected {
        build_connected_entries(&mut entries, state);
    } else {
        build_disconnected_entries(&mut entries, state);
    }

    entries.push(MenuEntry::Separator);
    entries.push(item(ids::QUIT, "Quit", true));

    entries
}

/// Build the complete Tauri tray menu for the current application state.
///
/// # Errors
///
/// Returns a Tauri error if native menu construction fails.
pub fn build_menu<R, M>(manager: &M, state: &AppState) -> tauri::Result<Menu<R>>
where
    R: Runtime,
    M: Manager<R>,
{
    let menu = Menu::new(manager)?;

    for entry in menu_model(state) {
        append_entry(manager, &menu, &entry)?;
    }

    Ok(menu)
}

fn build_connected_entries(entries: &mut Vec<MenuEntry>, state: &AppState) {
    let effect_label = match &state.current_effect {
        Some(effect) => format!("\u{25b6} {}", effect.name),
        None => "No effect active".to_owned(),
    };
    entries.push(item("current_effect", effect_label, false));

    if let Some(scene_name) = &state.active_scene_name {
        let scene_suffix = if state.scene_snapshot_locked {
            " [snap]"
        } else {
            ""
        };
        entries.push(item(
            "current_scene",
            format!("Scene: {scene_name}{scene_suffix}"),
            false,
        ));
    }

    entries.push(MenuEntry::Separator);

    if !state.effects.is_empty() {
        entries.push(MenuEntry::Submenu(SubmenuModel::new(
            "Effects",
            state
                .effects
                .iter()
                .map(|effect| {
                    item(
                        format!("{}{}", ids::EFFECT_PREFIX, effect.id),
                        effect.name.clone(),
                        true,
                    )
                })
                .collect(),
        )));
    }

    if !state.profiles.is_empty() {
        entries.push(MenuEntry::Submenu(SubmenuModel::new(
            "Profiles",
            state
                .profiles
                .iter()
                .map(|profile| {
                    item(
                        format!("{}{}", ids::PROFILE_PREFIX, profile.id),
                        profile.name.clone(),
                        true,
                    )
                })
                .collect(),
        )));
    }

    if should_show_servers_menu(state) {
        entries.push(servers_submenu(state));
    }

    entries.push(MenuEntry::Separator);
    entries.push(item(
        "brightness",
        format!("Brightness: {}%", state.brightness),
        false,
    ));

    let pause_label = if state.paused { "Resume" } else { "Pause" };
    entries.push(item(ids::PAUSE_RESUME, pause_label, true));

    if state.current_effect.is_some() {
        entries.push(item(ids::STOP_EFFECT, "Stop Effect", true));
    }

    entries.push(MenuEntry::Separator);
    build_app_entries(entries);
}

fn build_disconnected_entries(entries: &mut Vec<MenuEntry>, state: &AppState) {
    entries.push(item("status_disconnected", "Daemon not reachable", false));
    entries.push(MenuEntry::Separator);

    if should_show_servers_menu(state) {
        entries.push(servers_submenu(state));
        entries.push(MenuEntry::Separator);
    }

    build_app_entries(entries);
}

fn build_app_entries(entries: &mut Vec<MenuEntry>) {
    entries.push(item(ids::SHOW_WINDOW, "Show Window", true));
    entries.push(item(ids::OPEN_WEB_UI, "Open Web UI", true));
    entries.push(item(ids::OPEN_LOGS_FOLDER, "Open Logs Folder", true));
    entries.push(item(
        ids::OPEN_USER_EFFECTS_FOLDER,
        "Open User Effects Folder",
        true,
    ));
    entries.push(item(ids::SETTINGS, "Settings", true));
}

fn servers_submenu(state: &AppState) -> MenuEntry {
    let mut entries = state
        .servers
        .iter()
        .enumerate()
        .map(|(index, entry)| {
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
            item(format!("{}{}", ids::SERVER_PREFIX, index), label, true)
        })
        .collect::<Vec<_>>();

    entries.push(MenuEntry::Separator);
    entries.push(item(ids::REFRESH_SERVERS, "Refresh Servers", true));

    MenuEntry::Submenu(SubmenuModel::new("Servers", entries))
}

fn append_entry<R, M>(manager: &M, menu: &Menu<R>, entry: &MenuEntry) -> tauri::Result<()>
where
    R: Runtime,
    M: Manager<R>,
{
    match entry {
        MenuEntry::Item(item) => menu.append(&MenuItem::with_id(
            manager,
            item.id.as_str(),
            item.label.as_str(),
            item.enabled,
            None::<&str>,
        )?)?,
        MenuEntry::Submenu(submenu) => menu.append(&build_submenu(manager, submenu)?)?,
        MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator(manager)?)?,
    }
    Ok(())
}

fn append_submenu_entry<R, M>(
    manager: &M,
    submenu: &Submenu<R>,
    entry: &MenuEntry,
) -> tauri::Result<()>
where
    R: Runtime,
    M: Manager<R>,
{
    match entry {
        MenuEntry::Item(item) => submenu.append(&MenuItem::with_id(
            manager,
            item.id.as_str(),
            item.label.as_str(),
            item.enabled,
            None::<&str>,
        )?)?,
        MenuEntry::Submenu(child) => submenu.append(&build_submenu(manager, child)?)?,
        MenuEntry::Separator => submenu.append(&PredefinedMenuItem::separator(manager)?)?,
    }
    Ok(())
}

fn build_submenu<R, M>(manager: &M, model: &SubmenuModel) -> tauri::Result<Submenu<R>>
where
    R: Runtime,
    M: Manager<R>,
{
    let submenu = Submenu::new(manager, model.label.as_str(), true)?;

    for entry in &model.entries {
        append_submenu_entry(manager, &submenu, entry)?;
    }

    Ok(submenu)
}

fn item(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> MenuEntry {
    MenuEntry::Item(MenuItemModel::new(id, label, enabled))
}

fn should_show_servers_menu(state: &AppState) -> bool {
    state.servers.len() > 1
}
