//! Main window lifecycle helpers.

use tauri::{
    AppHandle, Manager, Runtime, WebviewWindow, Window,
    webview::{NewWindowFeatures, NewWindowResponse},
};
use url::Url;

/// Stable label for the app's main window.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Browser event dispatched when the native Tauri window is shown or hidden.
pub const WINDOW_VISIBILITY_EVENT: &str = "hypercolor-window-visibility";

/// Browser global used by the web UI to read the native Tauri window state.
pub const WINDOW_VISIBILITY_GLOBAL: &str = "__HYPERCOLOR_TAURI_WINDOW_VISIBLE";

/// Web UI route for the settings page.
pub const SETTINGS_ROUTE: &str = "/settings";

/// Return true when a webview new-window request should open in the system browser.
#[must_use]
pub fn should_open_in_system_browser(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// Parse and validate a URL that may be opened outside the native shell.
///
/// # Errors
///
/// Returns an error for malformed URLs or unsupported schemes.
pub fn system_browser_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid URL: {error}"))?;
    if should_open_in_system_browser(&url) {
        Ok(url)
    } else {
        Err(format!("unsupported URL scheme: {}", url.scheme()))
    }
}

/// Open a URL in the system browser for the embedded web UI.
///
/// # Errors
///
/// Returns an error when the URL is invalid, uses an unsupported scheme, or
/// cannot be handed off to the operating system.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let url = system_browser_url(&url)?;
    open::that_detached(url.as_str()).map_err(|error| format!("failed to open URL: {error}"))
}

/// Open a new-window request in the system browser instead of spawning a Tauri webview.
#[must_use]
pub fn open_new_window_in_system_browser<R: Runtime>(
    url: Url,
    _features: NewWindowFeatures,
) -> NewWindowResponse<R> {
    match open_external_url(url.to_string()) {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(%error, %url, "failed to handle external URL request");
        }
    }

    NewWindowResponse::Deny
}

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
