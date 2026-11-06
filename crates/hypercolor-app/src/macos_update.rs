//! The app's part in an update of its own bundle.
//!
//! macOS only: an update swaps the bundle this process runs from, so the app
//! answers a control socket that tells it to leave, and an app that starts
//! while the update is open, or from a translocated path, leaves every
//! daemon alone and says why in its window.

use hypercolor_macos_owner::MacosAppStartupHold;
use tauri::State;

use crate::supervisor::SupervisorState;

/// Tauri event carrying the current startup hold to the bundled UI.
pub const MACOS_APP_STARTUP_HOLD_EVENT: &str = "macos-app-startup-hold-changed";

/// Why this app holds instead of running a daemon, for the bundled UI.
#[tauri::command]
#[must_use]
pub fn macos_app_startup_hold(state: State<'_, SupervisorState>) -> Option<MacosAppStartupHold> {
    state.macos_startup_hold()
}

/// Whether macOS translocated this copy of the app.
#[must_use]
pub fn launched_translocated() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe()
            .is_ok_and(|executable| hypercolor_macos_owner::is_macos_translocated_path(&executable))
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Off macOS there is no bundle to swap, and so no socket to serve.
#[cfg(not(target_os = "macos"))]
pub const fn serve_app_control<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

#[cfg(target_os = "macos")]
pub use macos::{hold_startup, serve_app_control};

#[cfg(target_os = "macos")]
mod macos {
    use hypercolor_macos_owner::{
        MacosAppControlServer, MacosAppStartupHold, MacosRunningApp, macos_app_bundle_of,
        macos_app_startup_hold, macos_user_update_state_root, wait_for_macos_update_end,
    };
    use tauri::{AppHandle, Emitter as _, Manager as _, Runtime};
    use url::Url;

    use super::MACOS_APP_STARTUP_HOLD_EVENT;
    use crate::supervisor::SupervisorState;

    /// Serve the update control socket for the life of this app.
    ///
    /// A build with no managed installation has no update state root and
    /// serves nothing. Neither does an app that does not run from a bundle.
    pub fn serve_app_control<R: Runtime>(app: &AppHandle<R>) {
        let Some(root) = macos_user_update_state_root().filter(|root| root.is_dir()) else {
            return;
        };
        let bundle = std::env::current_exe()
            .ok()
            .and_then(|executable| macos_app_bundle_of(&executable));
        let Some(bundle) = bundle else {
            return;
        };
        let running = MacosRunningApp {
            pid: std::process::id(),
            bundle,
        };
        let server = match MacosAppControlServer::bind(&root) {
            Ok(server) => server,
            Err(error) => {
                tracing::warn!(%error, "failed to bind the update control socket");
                return;
            }
        };
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("hypercolor-update-control".to_owned())
            .spawn(move || match server.serve_until_quit(&running) {
                Ok(()) => {
                    tracing::info!("quitting for an update of this installation");
                    app.exit(0);
                }
                Err(error) => tracing::warn!(%error, "update control socket stopped"),
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "failed to start the update control thread");
        }
    }

    /// Hold this startup if it must leave every daemon alone, and report
    /// whether it did. The caller's supervision then runs only once the
    /// update ends, through [`crate::supervisor::start`]; a translocated app
    /// holds until it exits.
    pub fn hold_startup<R: Runtime>(app: &AppHandle<R>, daemon_url: &Url) -> bool {
        let root = macos_user_update_state_root();
        let hold = std::env::current_exe()
            .ok()
            .and_then(|executable| macos_app_startup_hold(&executable, root.as_deref()));
        let Some(hold) = hold else {
            publish(app, None);
            return false;
        };
        tracing::info!(?hold, "holding daemon startup");
        publish(app, Some(hold));
        if let (MacosAppStartupHold::UpdateInProgress, Some(root)) = (hold, root) {
            let app = app.clone();
            let daemon_url = daemon_url.clone();
            tauri::async_runtime::spawn_blocking(move || match wait_for_macos_update_end(&root) {
                Ok(()) => {
                    if let Err(error) = crate::supervisor::start(&app, daemon_url) {
                        tracing::warn!(%error, "failed to start after the update ended");
                    }
                }
                Err(error) => tracing::warn!(%error, "lost track of the update"),
            });
        }
        true
    }

    fn publish<R: Runtime>(app: &AppHandle<R>, hold: Option<MacosAppStartupHold>) {
        let state = app.state::<SupervisorState>();
        if state.macos_startup_hold() == hold {
            return;
        }
        state.set_macos_startup_hold(hold);
        if let Err(error) = app.emit(MACOS_APP_STARTUP_HOLD_EVENT, hold) {
            tracing::warn!(%error, "failed to publish the startup hold");
        }
    }
}
