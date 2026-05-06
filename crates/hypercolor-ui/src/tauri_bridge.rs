//! Optional bridge to native Tauri commands when the UI is hosted in hypercolor-app.

use serde::Deserialize;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

#[cfg(target_arch = "wasm32")]
use hypercolor_leptos_ext::events::window as browser_window;

/// Status for a native Windows service.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSupportStatus {
    pub installed: bool,
    pub state: Option<String>,
}

/// Bundled PawnIO module availability.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoModuleStatus {
    pub name: String,
    pub bundled: bool,
}

/// Native app hardware support status.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoSupportStatus {
    pub platform_supported: bool,
    pub pawnio_home: Option<String>,
    pub pawnio_runtime_installed: bool,
    pub pawnio_service: ServiceSupportStatus,
    pub smbus_service: ServiceSupportStatus,
    pub bundled_asset_root: Option<String>,
    pub helper_script: Option<String>,
    pub broker_executable: Option<String>,
    pub bundled_installer_available: bool,
    pub bundled_modules: Vec<PawnIoModuleStatus>,
    pub install_available: bool,
}

/// Options for launching the native PawnIO helper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PawnIoHelperOptions {
    pub force_pawn_io: bool,
    pub silent: bool,
    pub reinstall_service: bool,
    pub no_start_service: bool,
}

/// Result returned after launching the native PawnIO helper.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoHelperLaunchResult {
    pub exit_code: Option<i32>,
}

/// Optional full Hypercolor daemon Windows SCM service status.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowsDaemonServiceStatus {
    pub platform_supported: bool,
    pub service_name: String,
    pub service: ServiceSupportStatus,
    pub running: bool,
    pub reuse_recommended: bool,
}

/// Returns true when the UI is running inside a Tauri WebView.
#[must_use]
#[cfg(target_arch = "wasm32")]
pub fn is_tauri_available() -> bool {
    tauri_invoke().is_some()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_tauri_available() -> bool {
    false
}

/// Returns true when the bundled setup payload is complete.
#[must_use]
pub fn bundled_payload_ready(status: &PawnIoSupportStatus) -> bool {
    status.bundled_installer_available && status.bundled_modules.iter().all(|module| module.bundled)
}

/// Returns true when Windows SMBus support is installed and running.
#[must_use]
pub fn smbus_support_ready(status: &PawnIoSupportStatus) -> bool {
    status.pawnio_runtime_installed
        && status.smbus_service.installed
        && status.smbus_service.state.as_deref() == Some("RUNNING")
}

/// Returns true when the native app is connected to a running Windows SCM daemon service.
#[must_use]
pub const fn windows_daemon_service_conflict(status: &WindowsDaemonServiceStatus) -> bool {
    status.platform_supported
        && status.service.installed
        && status.running
        && status.reuse_recommended
}

/// Detect native PawnIO support when the Tauri bridge exists.
///
/// # Errors
///
/// Returns an error when the native command rejects or returns malformed data.
#[cfg(target_arch = "wasm32")]
pub async fn detect_pawnio_support() -> Result<Option<PawnIoSupportStatus>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let value = invoke_command(&invoke, "detect_pawnio_support", None).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn detect_pawnio_support() -> Result<Option<PawnIoSupportStatus>, String> {
    Ok(None)
}

/// Detect the optional full Hypercolor Windows SCM daemon service.
///
/// # Errors
///
/// Returns an error when the native command rejects or returns malformed data.
#[cfg(target_arch = "wasm32")]
pub async fn detect_windows_daemon_service() -> Result<Option<WindowsDaemonServiceStatus>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let value = invoke_command(&invoke, "detect_windows_daemon_service", None).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn detect_windows_daemon_service() -> Result<Option<WindowsDaemonServiceStatus>, String> {
    Ok(None)
}

/// Launch the elevated native PawnIO helper.
///
/// # Errors
///
/// Returns an error when the Tauri bridge is unavailable, the native command
/// rejects, or the command result cannot be decoded.
#[cfg(target_arch = "wasm32")]
pub async fn launch_pawnio_helper(
    options: PawnIoHelperOptions,
) -> Result<PawnIoHelperLaunchResult, String> {
    let Some(invoke) = tauri_invoke() else {
        return Err("native app bridge is unavailable".to_owned());
    };

    let args = pawnio_helper_options_to_js(options)?;
    let value = invoke_command(&invoke, "launch_pawnio_helper", Some(args)).await?;
    serde_json_from_js_value(value)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn launch_pawnio_helper(
    _options: PawnIoHelperOptions,
) -> Result<PawnIoHelperLaunchResult, String> {
    Err("native app bridge is unavailable".to_owned())
}

/// Read the native app autostart state when the Tauri bridge exists.
///
/// # Errors
///
/// Returns an error when the autostart plugin rejects or returns malformed data.
#[cfg(target_arch = "wasm32")]
pub async fn get_autostart_enabled() -> Result<Option<bool>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let value = invoke_command(&invoke, "plugin:autostart|is_enabled", None).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn get_autostart_enabled() -> Result<Option<bool>, String> {
    Ok(None)
}

/// Enable or disable native app autostart.
///
/// # Errors
///
/// Returns an error when the Tauri bridge is unavailable or the autostart
/// plugin rejects the requested state change.
#[cfg(target_arch = "wasm32")]
pub async fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    let Some(invoke) = tauri_invoke() else {
        return Err("native app bridge is unavailable".to_owned());
    };

    let command = if enabled {
        "plugin:autostart|enable"
    } else {
        "plugin:autostart|disable"
    };
    let _ = invoke_command(&invoke, command, None).await?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn set_autostart_enabled(_enabled: bool) -> Result<(), String> {
    Err("native app bridge is unavailable".to_owned())
}

/// Open an external URL through the native shell when available.
///
/// # Errors
///
/// Returns an error when the native command rejects the URL or cannot hand it
/// off to the operating system.
#[cfg(target_arch = "wasm32")]
pub async fn open_external_url(url: &str) -> Result<bool, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(false);
    };

    let args = string_arg_to_js("url", url)?;
    let _ = invoke_command(&invoke, "open_external_url", Some(args)).await?;
    Ok(true)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn open_external_url(_url: &str) -> Result<bool, String> {
    Ok(false)
}

#[cfg(target_arch = "wasm32")]
async fn invoke_command(
    invoke: &js_sys::Function,
    command: &str,
    args: Option<JsValue>,
) -> Result<JsValue, String> {
    let command = JsValue::from_str(command);
    let value = if let Some(args) = args {
        invoke
            .call2(&JsValue::NULL, &command, &args)
            .map_err(js_error_string)?
    } else {
        invoke
            .call1(&JsValue::NULL, &command)
            .map_err(js_error_string)?
    };

    let promise = js_sys::Promise::from(value);
    JsFuture::from(promise).await.map_err(js_error_string)
}

#[cfg(target_arch = "wasm32")]
fn string_arg_to_js(key: &str, value: &str) -> Result<JsValue, String> {
    let root = js_sys::Object::new();
    js_sys::Reflect::set(&root, &JsValue::from_str(key), &JsValue::from_str(value))
        .map_err(js_error_string)?;
    Ok(root.into())
}

#[cfg(target_arch = "wasm32")]
fn tauri_invoke() -> Option<js_sys::Function> {
    let window = browser_window()?;
    let tauri = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("__TAURI__")).ok()?;
    let core = js_sys::Reflect::get(&tauri, &JsValue::from_str("core")).ok()?;
    js_sys::Reflect::get(&core, &JsValue::from_str("invoke"))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()
}

#[cfg(target_arch = "wasm32")]
fn pawnio_helper_options_to_js(options: PawnIoHelperOptions) -> Result<JsValue, String> {
    let root = js_sys::Object::new();
    let inner = js_sys::Object::new();
    set_bool(&inner, "forcePawnIo", options.force_pawn_io)?;
    set_bool(&inner, "silent", options.silent)?;
    set_bool(&inner, "reinstallService", options.reinstall_service)?;
    set_bool(&inner, "noStartService", options.no_start_service)?;
    js_sys::Reflect::set(&root, &JsValue::from_str("options"), &inner).map_err(js_error_string)?;
    Ok(root.into())
}

#[cfg(target_arch = "wasm32")]
fn set_bool(target: &js_sys::Object, key: &str, value: bool) -> Result<(), String> {
    js_sys::Reflect::set(target, &JsValue::from_str(key), &JsValue::from_bool(value))
        .map_err(js_error_string)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn serde_json_from_js_value<T>(value: JsValue) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let json = js_sys::JSON::stringify(&value)
        .map_err(js_error_string)?
        .as_string()
        .ok_or_else(|| "native command returned a non-JSON value".to_owned())?;
    serde_json::from_str(&json).map_err(|error| format!("native command decode failed: {error}"))
}

#[cfg(target_arch = "wasm32")]
fn js_error_string(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| {
        js_sys::JSON::stringify(&value)
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown JavaScript error".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PawnIoModuleStatus, PawnIoSupportStatus, ServiceSupportStatus, bundled_payload_ready,
        smbus_support_ready, windows_daemon_service_conflict,
    };

    #[test]
    fn bundled_payload_ready_requires_installer_and_all_modules() {
        let mut status = status();
        assert!(bundled_payload_ready(&status));

        status.bundled_modules[1].bundled = false;
        assert!(!bundled_payload_ready(&status));

        status.bundled_modules[1].bundled = true;
        status.bundled_installer_available = false;
        assert!(!bundled_payload_ready(&status));
    }

    #[test]
    fn smbus_support_ready_requires_runtime_installed_service_and_running_state() {
        let mut status = status();
        assert!(smbus_support_ready(&status));

        status.smbus_service.state = Some("STOPPED".to_string());
        assert!(!smbus_support_ready(&status));

        status.smbus_service.state = Some("RUNNING".to_string());
        status.pawnio_runtime_installed = false;
        assert!(!smbus_support_ready(&status));
    }

    #[test]
    fn windows_daemon_service_conflict_requires_supported_running_service() {
        let mut status = windows_service_status();
        assert!(windows_daemon_service_conflict(&status));

        status.running = false;
        assert!(!windows_daemon_service_conflict(&status));

        status.running = true;
        status.reuse_recommended = false;
        assert!(!windows_daemon_service_conflict(&status));

        status.reuse_recommended = true;
        status.platform_supported = false;
        assert!(!windows_daemon_service_conflict(&status));
    }

    fn status() -> PawnIoSupportStatus {
        PawnIoSupportStatus {
            platform_supported: true,
            pawnio_home: Some(r"C:\Program Files\PawnIO".to_string()),
            pawnio_runtime_installed: true,
            pawnio_service: ServiceSupportStatus {
                installed: true,
                state: Some("RUNNING".to_string()),
            },
            smbus_service: ServiceSupportStatus {
                installed: true,
                state: Some("RUNNING".to_string()),
            },
            bundled_asset_root: Some(r"C:\Program Files\Hypercolor\tools\pawnio".to_string()),
            helper_script: Some(
                r"C:\Program Files\Hypercolor\tools\install-windows-hardware-support.ps1"
                    .to_string(),
            ),
            broker_executable: Some(
                r"C:\Program Files\Hypercolor\tools\hypercolor-smbus-service.exe".to_string(),
            ),
            bundled_installer_available: true,
            bundled_modules: vec![
                module("SmbusI801.bin"),
                module("SmbusPIIX4.bin"),
                module("SmbusNCT6793.bin"),
            ],
            install_available: true,
        }
    }

    fn module(name: &str) -> PawnIoModuleStatus {
        PawnIoModuleStatus {
            name: name.to_string(),
            bundled: true,
        }
    }

    fn windows_service_status() -> super::WindowsDaemonServiceStatus {
        super::WindowsDaemonServiceStatus {
            platform_supported: true,
            service_name: "Hypercolor".to_string(),
            service: ServiceSupportStatus {
                installed: true,
                state: Some("RUNNING".to_string()),
            },
            running: true,
            reuse_recommended: true,
        }
    }
}
