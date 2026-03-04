// Hypercolor Desktop — native window shell
//
// Connects to a running hypercolor-daemon and renders the Leptos
// web UI inside a system webview. The daemon owns the hardware;
// this app is just the control surface.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{WebviewUrl, webview::WebviewWindowBuilder};

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:9420";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("hypercolor_desktop=debug,tauri=info")
        .init();

    let daemon_url =
        std::env::var("HYPERCOLOR_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_string());

    tracing::info!(url = %daemon_url, "launching desktop shell");

    tauri::Builder::default()
        .setup(move |app| {
            let url: url::Url = daemon_url
                .parse()
                .expect("HYPERCOLOR_URL must be a valid URL");

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Hypercolor")
                .inner_size(1200.0, 800.0)
                .min_inner_size(800.0, 500.0)
                .build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("tauri runtime error: {e}"))?;

    Ok(())
}
