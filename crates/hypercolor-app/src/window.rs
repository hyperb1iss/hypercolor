//! Main window lifecycle helpers.

use tauri::{AppHandle, Manager, Runtime, WebviewWindow, Window};

/// Stable label for the app's main window.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Browser event dispatched when the native Tauri window is shown or hidden.
pub const WINDOW_VISIBILITY_EVENT: &str = "hypercolor-window-visibility";

/// Browser global used by the web UI to read the native Tauri window state.
pub const WINDOW_VISIBILITY_GLOBAL: &str = "__HYPERCOLOR_TAURI_WINDOW_VISIBLE";

/// Build the JavaScript that mirrors native window visibility into the web UI.
#[must_use]
pub fn visibility_state_script(visible: bool) -> String {
    format!(
        r#"(function () {{
  const visible = {visible};
  window.{WINDOW_VISIBILITY_GLOBAL} = visible;
  window.dispatchEvent(new CustomEvent("{WINDOW_VISIBILITY_EVENT}", {{ detail: {{ visible }} }}));
}})();"#
    )
}

/// Show and focus the main app window when it exists.
///
/// # Errors
///
/// Returns a Tauri error if the native window cannot be shown or focused.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window.show()?;
        window.set_focus()?;
        notify_visibility(&window, true);
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
            notify_visibility(&window, false);
        } else {
            window.show()?;
            window.set_focus()?;
            notify_visibility(&window, true);
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
    window.hide()?;
    if let Some(webview_window) = window.app_handle().get_webview_window(window.label()) {
        notify_visibility(&webview_window, false);
    }
    Ok(())
}

fn notify_visibility<R: Runtime>(window: &WebviewWindow<R>, visible: bool) {
    if let Err(error) = window.eval(visibility_state_script(visible)) {
        tracing::warn!(
            %error,
            label = %window.label(),
            visible,
            "failed to notify webview of native window visibility"
        );
    }
}
