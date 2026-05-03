// Hypercolor App — unified native desktop front door.
//
// This starts as the existing Tauri webview shell. Tray, app lifecycle,
// daemon supervision, and installer payload wiring land in follow-up slices.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{WebviewUrl, webview::WebviewWindowBuilder};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "hypercolor_app=debug,tauri=info,wry=warn".to_string()),
        )
        .init();

    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| hypercolor_app::DEFAULT_DAEMON_URL.to_string());

    tracing::info!(url = %daemon_url, "launching Hypercolor app shell");

    tauri::Builder::default()
        .setup(move |app| {
            let url: url::Url = daemon_url
                .parse()
                .expect("HYPERCOLOR_URL must be a valid URL");

            tracing::info!(%url, "creating webview window");

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Hypercolor")
                .inner_size(1200.0, 800.0)
                .min_inner_size(800.0, 500.0)
                .build()?;

            #[cfg(debug_assertions)]
            window.open_devtools();

            tracing::info!("window created");

            hypercolor_app::tray::register(app.handle())?;
            tracing::info!("tray icon registered");

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                tracing::info!(label = %window.label(), "hiding window instead of closing");
                if let Err(error) = window.hide() {
                    tracing::warn!(%error, label = %window.label(), "failed to hide window");
                }
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("tauri runtime error: {e}"))?;

    Ok(())
}
