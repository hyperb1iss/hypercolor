//! Main window lifecycle helpers.

use tauri::{AppHandle, Manager, Runtime, WebviewWindow, Window};

/// Stable label for the app's main window.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Browser event dispatched when the native Tauri window is shown or hidden.
pub const WINDOW_VISIBILITY_EVENT: &str = "hypercolor-window-visibility";

/// Browser global used by the web UI to read the native Tauri window state.
pub const WINDOW_VISIBILITY_GLOBAL: &str = "__HYPERCOLOR_TAURI_WINDOW_VISIBLE";

/// Web UI route for the settings page.
pub const SETTINGS_ROUTE: &str = "/settings";

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

/// Build the JavaScript that navigates the embedded web UI to a route.
#[must_use]
pub fn route_navigation_script(route: &str) -> String {
    let route = serde_json::to_string(route).expect("route string should serialize to JSON");
    format!(
        r#"(function () {{
  const target = {route};
  if (window.location.pathname !== target) {{
    window.history.pushState({{}}, "", target);
    window.dispatchEvent(new PopStateEvent("popstate", {{ state: window.history.state }}));
  }}
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
        show_and_focus(&window)?;
    }
    Ok(())
}

/// Show, focus, and navigate the main app window to settings.
///
/// # Errors
///
/// Returns a Tauri error if the native window cannot be shown, focused, or
/// instructed to navigate.
pub fn show_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        show_and_focus(&window)?;
        window.eval(route_navigation_script(SETTINGS_ROUTE))?;
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
            show_and_focus(&window)?;
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

fn show_and_focus<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()?;
    notify_visibility(window, true);
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
