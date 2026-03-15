mod api;
mod app;
mod components;
mod icons;
mod layout_geometry;
mod layout_utils;
mod pages;
mod style_utils;
mod toasts;
mod ws;

use app::App;
use leptos::prelude::*;

fn print_banner() {
    let version = env!("CARGO_PKG_VERSION");
    let msg = format!(
        "%c✦ Hypercolor %cv{version}%c\n🔮 RGB Lighting Engine for Linux\ngithub.com/hyperb1iss/hypercolor"
    );
    let args = js_sys::Array::new();
    args.push(&wasm_bindgen::JsValue::from_str(&msg));
    args.push(&wasm_bindgen::JsValue::from_str(
        "color:#e135ff;font-size:20px;font-weight:bold;text-shadow:0 0 10px #e135ff80;",
    ));
    args.push(&wasm_bindgen::JsValue::from_str(
        "color:#80ffea;font-size:14px;font-weight:normal;",
    ));
    args.push(&wasm_bindgen::JsValue::from_str(
        "color:#ff6ac1;font-size:12px;",
    ));
    web_sys::console::log(&args);
}

fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    print_banner();
    mount_to_body(App);
}
