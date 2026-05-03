// Hypercolor App — unified native desktop front door.
//
// This starts as the existing Tauri webview shell. Tray, app lifecycle,
// daemon supervision, and installer payload wiring land in follow-up slices.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{WebviewUrl, webview::WebviewWindowBuilder};

fn main() -> anyhow::Result<()> {
    let _log_guard = hypercolor_app::logging::init()?;

    let cli = hypercolor_app::cli::AppArgs::parse_env();
    if cli.quit {
        tracing::info!("quit requested with no running app instance");
        return Ok(());
    }

    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| hypercolor_app::DEFAULT_DAEMON_URL.to_string());

    tracing::info!(url = %daemon_url, "launching Hypercolor app shell");

    tauri::Builder::default()
        .manage(hypercolor_app::supervisor::SupervisorState::default())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let forwarded = hypercolor_app::cli::AppArgs::parse(args);
            if forwarded.quit {
                tracing::info!("quit requested by forwarded app invocation");
                app.exit(0);
            } else if let Err(error) = hypercolor_app::window::show_main(app) {
                tracing::warn!(%error, "failed to show main window from forwarded invocation");
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            let url: url::Url = daemon_url
                .parse()
                .expect("HYPERCOLOR_URL must be a valid URL");

            tracing::info!(%url, "creating webview window");

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.clone()))
                .title("Hypercolor")
                .inner_size(1200.0, 800.0)
                .min_inner_size(800.0, 500.0)
                .initialization_script(hypercolor_app::window::visibility_state_script(
                    !cli.start_minimized,
                ))
                .visible(!cli.start_minimized)
                .build()?;

            #[cfg(debug_assertions)]
            window.open_devtools();

            tracing::info!("window created");

            hypercolor_app::tray::register(app.handle())?;
            tracing::info!("tray icon registered");

            hypercolor_app::supervisor::start(app.handle(), url)?;
            tracing::info!("daemon supervisor started");

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                tracing::info!(label = %window.label(), "hiding window instead of closing");
                if let Err(error) = hypercolor_app::window::hide(window) {
                    tracing::warn!(%error, label = %window.label(), "failed to hide window");
                }
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("tauri runtime error: {e}"))?;

    Ok(())
}
