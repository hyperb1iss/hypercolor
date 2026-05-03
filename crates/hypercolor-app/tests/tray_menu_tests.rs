use std::net::{IpAddr, Ipv4Addr};

use hypercolor_app::{
    state::{AppState, EffectInfo, ProfileInfo, ServerEntry},
    tray::menu::{MenuEntry, ids, menu_model},
};
use hypercolor_types::server::{DiscoveredServer, ServerIdentity};

#[test]
fn disconnected_menu_contains_app_actions() {
    let entries = menu_model(&AppState::disconnected());

    assert_item(&entries, "header", "Hypercolor (Disconnected)", false);
    assert_item(
        &entries,
        "status_disconnected",
        "Daemon not reachable",
        false,
    );
    assert_item(&entries, ids::SHOW_WINDOW, "Show Window", true);
    assert_item(&entries, ids::OPEN_WEB_UI, "Open Web UI", true);
    assert_item(&entries, ids::OPEN_LOGS_FOLDER, "Open Logs Folder", true);
    assert_item(
        &entries,
        ids::OPEN_USER_EFFECTS_FOLDER,
        "Open User Effects Folder",
        true,
    );
    assert_item(&entries, ids::SETTINGS, "Settings", true);
    assert_item(&entries, ids::QUIT, "Quit", true);
}

#[test]
fn connected_menu_contains_dynamic_entries() {
    let entries = menu_model(&connected_state());

    assert_item(&entries, "header", "Hypercolor", false);
    assert_item(
        &entries,
        "current_effect",
        "\u{25b6} Aurora Borealis",
        false,
    );
    assert_item(
        &entries,
        "current_scene",
        "Scene: Movie Night [snap]",
        false,
    );
    assert_item(&entries, "brightness", "Brightness: 80%", false);
    assert_item(&entries, ids::PAUSE_RESUME, "Resume", true);
    assert_item(&entries, ids::STOP_EFFECT, "Stop Effect", true);

    let effects = find_submenu(&entries, "Effects");
    assert_item(effects, "effect:aurora", "Aurora Borealis", true);
    assert_item(effects, "effect:wave", "Color Wave", true);

    let profiles = find_submenu(&entries, "Profiles");
    assert_item(profiles, "profile:movie", "Movie Night", true);

    let servers = find_submenu(&entries, "Servers");
    assert_item(servers, "server:0", "\u{25cf} desk-pc (127.0.0.1)", true);
    assert_item(
        servers,
        "server:1",
        "laptop (127.0.0.2) (Key required)",
        true,
    );
    assert_item(servers, ids::REFRESH_SERVERS, "Refresh Servers", true);
}

#[test]
fn connected_menu_hides_stop_effect_without_current_effect() {
    let mut state = connected_state();
    state.current_effect = None;

    let entries = menu_model(&state);

    assert_item(&entries, "current_effect", "No effect active", false);
    assert_no_item(&entries, ids::STOP_EFFECT);
}

fn connected_state() -> AppState {
    AppState {
        connected: true,
        running: true,
        paused: true,
        brightness: 80,
        current_effect: Some(EffectInfo {
            id: "aurora".to_owned(),
            name: "Aurora Borealis".to_owned(),
        }),
        active_scene_name: Some("Movie Night".to_owned()),
        scene_snapshot_locked: true,
        device_count: 2,
        effects: vec![
            EffectInfo {
                id: "aurora".to_owned(),
                name: "Aurora Borealis".to_owned(),
            },
            EffectInfo {
                id: "wave".to_owned(),
                name: "Color Wave".to_owned(),
            },
        ],
        profiles: vec![ProfileInfo {
            id: "movie".to_owned(),
            name: "Movie Night".to_owned(),
        }],
        server_identity: None,
        servers: vec![
            server_entry(0, "desk-pc", false, true),
            server_entry(1, "laptop", true, false),
        ],
        active_server: Some(0),
    }
}

fn server_entry(index: u8, name: &str, auth_required: bool, has_api_key: bool) -> ServerEntry {
    ServerEntry {
        server: DiscoveredServer {
            identity: ServerIdentity {
                instance_id: format!("server-{index}"),
                instance_name: name.to_owned(),
                version: "0.1.0".to_owned(),
            },
            host: IpAddr::V4(Ipv4Addr::new(127, 0, 0, index + 1)),
            port: 9420,
            device_count: Some(1),
            auth_required,
        },
        has_api_key,
    }
}

fn find_submenu<'a>(entries: &'a [MenuEntry], label: &str) -> &'a [MenuEntry] {
    entries
        .iter()
        .find_map(|entry| match entry {
            MenuEntry::Submenu(submenu) if submenu.label == label => {
                Some(submenu.entries.as_slice())
            }
            _ => None,
        })
        .expect("submenu should exist")
}

fn assert_item(entries: &[MenuEntry], id: &str, label: &str, enabled: bool) {
    let item = entries
        .iter()
        .find_map(|entry| match entry {
            MenuEntry::Item(item) if item.id == id => Some(item),
            _ => None,
        })
        .expect("item should exist");

    assert_eq!(item.label, label);
    assert_eq!(item.enabled, enabled);
}

fn assert_no_item(entries: &[MenuEntry], id: &str) {
    let has_item = entries.iter().any(|entry| match entry {
        MenuEntry::Item(item) => item.id == id,
        MenuEntry::Submenu(submenu) => submenu.entries.iter().any(|entry| match entry {
            MenuEntry::Item(item) => item.id == id,
            MenuEntry::Submenu(_) | MenuEntry::Separator => false,
        }),
        MenuEntry::Separator => false,
    });

    assert!(!has_item);
}
