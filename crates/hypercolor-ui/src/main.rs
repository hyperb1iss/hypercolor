mod api;
mod app;
mod async_helpers;
mod channel_names;
mod color;
mod components;
mod compound_selection;
mod config_state;
mod control_geometry;
mod device_event_logic;
mod device_metrics;
mod display_preview_state;
mod display_utils;
mod driver_settings;
mod effect_search;
mod icons;
mod label_utils;
mod layout_geometry;
mod layout_history;
mod layout_page_state;
mod layout_utils;
mod pages;
mod preferences;
mod preview_telemetry;
mod render_presets;
mod route_ui;
mod settings_audio_devices;
mod storage;
mod style_utils;
mod tauri_bridge;
mod thumbnails;
mod toasts;
mod vendors;
mod ws;

use app::App;
use hypercolor_leptos_ext::prelude::console_log_styled;
use leptos::prelude::*;

fn print_banner() {
    let version = env!("CARGO_PKG_VERSION");
    let msg = format!(
        "%c✦ Hypercolor %cv{version}%c\n🔮 RGB Lighting Engine for Linux\ngithub.com/hyperb1iss/hypercolor"
    );
    console_log_styled(
        &msg,
        &[
            "color:#e135ff;font-size:20px;font-weight:bold;text-shadow:0 0 10px #e135ff80;",
            "color:#80ffea;font-size:14px;font-weight:normal;",
            "color:#ff6ac1;font-size:12px;",
        ],
    );
}

fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    print_banner();
    mount_to_body(App);
}
