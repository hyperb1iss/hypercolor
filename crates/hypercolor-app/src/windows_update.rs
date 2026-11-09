//! The app's part in an update of its own installation.
//!
//! Windows only: an update replaces the unit this process runs from, so
//! the app follows the installation's update service. Each time the service
//! starts, the app connects to the pipe it published, once the service's
//! own process proves to serve it, and when the service asks, the app
//! closes its window, ends its daemon and exits, exactly as the tray's quit
//! does. An app that does not run from a unit has no installation to
//! update and follows nothing.

/// Off Windows there is no update service to follow.
#[cfg(not(target_os = "windows"))]
pub const fn follow_update_service<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

#[cfg(target_os = "windows")]
pub use windows::follow_update_service;

#[cfg(target_os = "windows")]
mod windows {
    use hypercolor_install::windows::{
        UpdatePipeError, UpdateServiceEnd, WindowsInstallLayout, WindowsUpdatePipeHost,
    };
    use tauri::{AppHandle, Runtime};

    /// Follow the update service for the life of this app, on a thread of
    /// its own, since every wait there blocks until the service acts.
    pub fn follow_update_service<R: Runtime>(app: &AppHandle<R>) {
        let Some(layout) = installation() else {
            return;
        };
        let host = WindowsUpdatePipeHost::new(&layout);
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("hypercolor-update-pipe".to_owned())
            .spawn(move || {
                let followed = hypercolor_install::windows::follow_update_service(
                    &host,
                    |error: UpdatePipeError| {
                        tracing::warn!(%error, "missed a run of the update service");
                    },
                );
                match followed {
                    Ok(UpdateServiceEnd::Quit) => {
                        tracing::info!("quitting for an update of this installation");
                        app.exit(0);
                    }
                    Ok(UpdateServiceEnd::Unregistered) => {
                        tracing::debug!("this installation has no update service");
                    }
                    Err(error) => tracing::warn!(%error, "stopped following the update service"),
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "failed to start the update pipe thread");
        }
    }

    /// The installation this app's unit belongs to.
    fn installation() -> Option<WindowsInstallLayout> {
        let program = std::env::current_exe().ok()?;
        let program_data = std::env::var_os("ProgramData")?;
        WindowsInstallLayout::of_unit_program(&program, program_data.as_ref())
    }
}
