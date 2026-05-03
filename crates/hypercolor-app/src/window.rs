//! Main window lifecycle helpers.

use tauri::{AppHandle, Manager, Runtime, Window};

/// Stable label for the app's main window.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Show and focus the main app window when it exists.
///
/// # Errors
///
/// Returns a Tauri error if the native window cannot be shown or focused.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

/// Toggle main window visibility when it exists.
///
/// # Errors
///
/// Returns a Tauri error if the native window visibility query or mutation
/// fails.
pub fn toggle_main<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if window.is_visible()? {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}

/// Hide the window for close-to-tray behavior.
///
/// # Errors
///
/// Returns a Tauri error if the native window cannot be hidden.
pub fn hide<R: Runtime>(window: &Window<R>) -> tauri::Result<()> {
    window.hide()
}
