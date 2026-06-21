//! Leptos 0.8 CSR WASM web frontend for the Hypercolor lighting engine.
//!
//! Excluded from the Cargo workspace — build with `just ui-dev` (Trunk dev server)
//! or `just ui-build` (production). Communicates with `hypercolor-daemon` over
//! REST and WebSocket using `hypercolor-leptos-ext`.
//!
//! Everything lives in this lib target; the `hypercolor-ui` bin is a thin
//! shim over [`run`]. That keeps every module reachable from integration
//! tests as `hypercolor_ui::...` without `#[path]` source includes.

pub mod api;
pub mod app;
pub mod apply_target;
pub mod async_helpers;
pub mod channel_names;
pub mod color;
pub mod components;
pub mod compound_selection;
pub mod config_state;
pub mod control_geometry;
pub mod control_session;
pub mod control_surface_api;
pub mod control_surface_values;
pub mod control_surface_view;
pub mod control_value_json;
pub mod device_event_logic;
pub mod device_metrics;
pub mod display_preview_state;
pub mod display_utils;
pub mod driver_settings;
pub mod effect_search;
pub mod extensions;
pub mod icons;
pub mod label_utils;
pub mod layout_geometry;
pub mod layout_history;
pub mod layout_page_state;
pub mod layout_utils;
pub mod nav;
pub mod optimistic_controls;
pub mod pages;
pub mod preferences;
pub mod preview_telemetry;
pub mod render_presets;
pub mod route_ui;
pub mod settings_audio_devices;
pub mod storage;
pub mod style_utils;
pub mod tauri_bridge;
pub mod thumbnails;
pub mod toasts;
pub mod vendors;
pub mod ws;
pub mod zones;

use hypercolor_leptos_ext::prelude::console_log_styled;
use leptos::prelude::mount_to_body;

// ── Public extension seam ─────────────────────────────────────────────────
// Generic, cloud-agnostic surface a downstream entry crate composes against.
// Default-empty, so the standalone OSS app is unchanged. See `extensions` and
// `nav` for the contract.
pub use extensions::{UiExtensions, UiNavItem, UiSettingsSection, parent_route, ui_route};
pub use nav::{NavEntry, NavExtensionItems, nav_model, nav_shortcut_path};

// Re-export the shared HTTP client helpers (envelope unwrap + auth + Trunk
// dev-proxy) so an embedder can call the daemon's local API through the same
// plumbing the OSS UI uses, instead of hand-rolling `gloo-net` against bare
// paths.
pub use api::client;

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

/// Initialize logging and mount the app with the given extensions.
///
/// Routes from `ext` are threaded **by value** into the mount closure rather
/// than through context: the erased route defs ([`extensions::UiExtensions::routes`])
/// are `Send` but not `Sync`, so `provide_context` cannot carry them. Nav items
/// are plain data and are surfaced through context inside [`app::app_view`].
pub fn run_with_extensions(ext: UiExtensions) {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    print_banner();
    mount_to_body(move || app::app_view(ext));
}

/// Initialize logging and mount the standalone OSS app. The bin target's whole
/// job. Equivalent to [`run_with_extensions`] with no extensions installed.
pub fn run() {
    run_with_extensions(UiExtensions::default());
}
