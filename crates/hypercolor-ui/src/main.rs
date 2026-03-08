mod api;
mod app;
mod components;
mod icons;
mod layout_geometry;
mod pages;
mod toasts;
mod ws;

use app::App;
use leptos::prelude::*;

fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
