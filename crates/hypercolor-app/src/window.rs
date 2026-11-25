//! Main window lifecycle helpers.

use std::sync::atomic::{AtomicBool, Ordering};

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
            set_backgrounded(&window, true);
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
        set_backgrounded(&webview_window, true);
    }
    Ok(())
}

/// Follow a minimize or restore after the native window resized.
///
/// Neither Tauri nor the webview reports minimizing as hidden, so the
/// webview would keep painting and streaming previews for a window nobody
/// can see. A window hidden to the tray also receives resize events (on
/// creation, or when the display scale changes), and must stay in the
/// background through them.
///
/// # Errors
///
/// Returns a Tauri error if the window state cannot be read.
pub fn follow_minimized<R: Runtime>(window: &Window<R>) -> tauri::Result<()> {
    let backgrounded = window.is_minimized()? || !window.is_visible()?;
    if let Some(webview_window) = window.app_handle().get_webview_window(window.label()) {
        set_backgrounded(&webview_window, backgrounded);
    }
    Ok(())
}

/// Whether the main window is put away, either hidden to the tray or
/// minimized.
///
/// Records the last state handed to the webview, so the resize events a
/// window drag emits cost one atomic swap and nothing else.
#[derive(Debug, Default)]
pub struct WindowBackground {
    backgrounded: AtomicBool,
}

impl WindowBackground {
    /// Record a new state and report whether it differs from the last one.
    pub fn transition(&self, backgrounded: bool) -> bool {
        self.backgrounded.swap(backgrounded, Ordering::AcqRel) != backgrounded
    }
}

/// Move the webview into or out of the background.
///
/// The web UI stops streaming previews, and on Windows WebView2 is told the
/// window is not visible and asked to trim its memory. Hiding the native
/// window alone tells WebView2 nothing, so without this its renderer keeps
/// running and holds its full working set for as long as the app sits in
/// the tray.
pub fn set_backgrounded<R: Runtime>(window: &WebviewWindow<R>, backgrounded: bool) {
    let changed = window
        .try_state::<WindowBackground>()
        .is_none_or(|state| state.transition(backgrounded));
    if !changed {
        return;
    }
    notify_visibility(window, !backgrounded);
    apply_webview_background(window, backgrounded);
}

#[cfg(target_os = "windows")]
fn apply_webview_background<R: Runtime>(window: &WebviewWindow<R>, backgrounded: bool) {
    let dispatched = window.with_webview(move |platform_webview| {
        if let Err(error) = set_webview2_background(&platform_webview.controller(), backgrounded) {
            tracing::warn!(%error, backgrounded, "WebView2 refused a background state change");
        }
    });
    if let Err(error) = dispatched {
        tracing::warn!(
            %error,
            label = %window.label(),
            backgrounded,
            "failed to reach WebView2 to change its background state"
        );
    }
}

/// Visibility drops before the memory target and returns after it, so
/// WebView2 never trims a page that is still on screen.
#[cfg(target_os = "windows")]
fn set_webview2_background(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    backgrounded: bool,
) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        ICoreWebView2_19,
    };
    use windows_core::Interface;

    let level = if backgrounded {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    // SAFETY: Tauri hands this controller to `with_webview` closures on the
    // UI thread that owns it and keeps it alive for the call. Each call is a
    // plain COM method on that live interface.
    unsafe {
        if backgrounded {
            controller.SetIsVisible(false)?;
        }
        controller
            .CoreWebView2()?
            .cast::<ICoreWebView2_19>()?
            .SetMemoryUsageTargetLevel(level)?;
        if !backgrounded {
            controller.SetIsVisible(true)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_webview_background<R: Runtime>(_window: &WebviewWindow<R>, _backgrounded: bool) {}

fn show_and_focus<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    // The webview comes forward before the window shows, so the first frame
    // on screen is painted rather than blank. Both requests travel the same
    // event loop queue, in this order.
    set_backgrounded(window, false);
    window.show()?;
    if window.is_minimized()? {
        window.unminimize()?;
    }
    window.set_focus()?;
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
