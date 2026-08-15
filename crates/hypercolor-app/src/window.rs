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

const INPUT_MONITORING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent";
const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture";

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

/// Resolve a permitted macOS privacy pane to its System Settings deep link.
///
/// # Errors
///
/// Returns an error unless `pane` names one of Hypercolor's two supported
/// privacy remedies.
pub fn macos_system_settings_url(pane: &str) -> Result<&'static str, String> {
    match pane {
        "input_monitoring" => Ok(INPUT_MONITORING_SETTINGS_URL),
        "screen_recording" => Ok(SCREEN_RECORDING_SETTINGS_URL),
        _ => Err("unsupported macOS System Settings pane".to_owned()),
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

/// Open one of Hypercolor's macOS privacy remedies through the native shell.
///
/// # Errors
///
/// Returns an error when the pane is not allowlisted, the app is not running
/// on macOS, or the operating system rejects the handoff.
#[tauri::command]
pub fn open_macos_system_settings(pane: String) -> Result<(), String> {
    let url = macos_system_settings_url(&pane)?;

    #[cfg(target_os = "macos")]
    {
        open::that_detached(url)
            .map_err(|error| format!("failed to open macOS System Settings: {error}"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("macOS System Settings are unavailable on this platform".to_owned())
    }
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

/// Whether a main-frame navigation may proceed inside the trusted webview.
///
/// The webview holds `window.__TAURI__` and the per-session protected
/// control credential, so only the bundled app origin may load in it: the
/// custom `tauri:` scheme on macOS and Linux, and the `tauri.localhost`
/// host on Windows. Everything else is denied; new-window requests
/// already route to the system browser.
#[must_use]
pub fn navigation_is_trusted(url: &Url) -> bool {
    if url.scheme() == "tauri" {
        return true;
    }
    matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost")
}

/// Release a secure-input assertion retained by the embedded macOS webview.
///
/// # Errors
///
/// Returns a Tauri error when the webview cannot be accessed on its UI thread.
pub fn release_webview_secure_input<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    window.with_webview(|platform_webview| {
        use objc2::{msg_send, runtime::AnyObject, sel};

        // SAFETY: Tauri documents this pointer as a live WKWebView for the
        // duration of the closure. The private selector is checked before a
        // no-argument, void-returning message is sent.
        unsafe {
            let webview = &*platform_webview.inner().cast::<AnyObject>();
            let reset = sel!(_resetSecureInputState);
            let responds: bool = msg_send![webview, respondsToSelector: reset];
            if responds {
                let _: () = msg_send![webview, _resetSecureInputState];
            } else {
                // The private selector is the only release mechanism; if
                // WebKit renames it the workaround dies silently, so say
                // so once instead of never.
                static MISSING_SELECTOR_WARNED: std::sync::Once = std::sync::Once::new();
                MISSING_SELECTOR_WARNED.call_once(|| {
                    tracing::warn!(
                        "WKWebView no longer responds to _resetSecureInputState; \
                         secure-input release after focus loss is inoperative"
                    );
                });
            }
        }
    })?;

    #[cfg(not(target_os = "macos"))]
    let _ = window;

    Ok(())
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
