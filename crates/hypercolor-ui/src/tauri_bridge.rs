//! Optional bridge to native Tauri commands when the UI is hosted in hypercolor-app.

use serde::Deserialize;
#[cfg(any(target_arch = "wasm32", test))]
use std::{cell::Cell, future::Future};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

#[cfg(target_arch = "wasm32")]
use hypercolor_leptos_ext::events::window as browser_window;

/// macOS privacy remedy that the native app may open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosSystemSettingsPane {
    InputMonitoring,
    ScreenRecording,
}

impl MacosSystemSettingsPane {
    #[must_use]
    pub const fn invoke_value(self) -> &'static str {
        match self {
            Self::InputMonitoring => "input_monitoring",
            Self::ScreenRecording => "screen_recording",
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const MACOS_SYSTEM_SETTINGS_COMMAND: &str = "open_macos_system_settings";

#[cfg(any(target_arch = "wasm32", test))]
const VERIFIED_DAEMON_CONNECTION_COMMAND: &str = "get_verified_daemon_connection";

#[cfg(target_arch = "wasm32")]
const VERIFIED_DAEMON_CONNECTION_EVENT: &str = "verified-daemon-connection-changed";

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct VerifiedDaemonConnection {
    base_url: String,
    #[serde(default)]
    server_session_id: Option<String>,
    #[serde(default)]
    protected_control_credential: Option<String>,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct VerifiedDaemonConnectionSnapshot {
    revision: u64,
    connection: Option<VerifiedDaemonConnection>,
}

#[cfg(any(target_arch = "wasm32", test))]
thread_local! {
    static VERIFIED_DAEMON_REVISION: Cell<u64> = const { Cell::new(0) };
}

#[cfg(target_arch = "wasm32")]
#[derive(Deserialize)]
struct VerifiedDaemonConnectionEvent {
    payload: VerifiedDaemonConnectionSnapshot,
}

#[cfg(any(target_arch = "wasm32", test))]
fn apply_verified_daemon_connection(snapshot: VerifiedDaemonConnectionSnapshot) -> bool {
    let accepted = VERIFIED_DAEMON_REVISION.with(|revision| {
        if snapshot.revision <= revision.get() {
            false
        } else {
            revision.set(snapshot.revision);
            true
        }
    });
    if !accepted {
        return false;
    }
    if let Some(connection) = snapshot.connection {
        crate::api::client::install_verified_daemon_connection(
            &connection.base_url,
            connection.protected_control_credential.as_deref(),
        );
    } else {
        crate::api::client::clear_verified_daemon_connection();
    }
    notify_verified_daemon_connection_change();
    true
}

#[cfg(any(target_arch = "wasm32", test))]
async fn snapshot_after_listener_registration<Registration, Snapshot, SnapshotFuture, T>(
    registration: Registration,
    snapshot: Snapshot,
) -> Option<T>
where
    Registration: Future<Output = bool>,
    Snapshot: FnOnce() -> SnapshotFuture,
    SnapshotFuture: Future<Output = Option<T>>,
{
    if !registration.await {
        return None;
    }
    snapshot().await
}

#[cfg(target_arch = "wasm32")]
fn notify_verified_daemon_connection_change() {
    let Some(window) = browser_window() else {
        return;
    };
    if let Ok(event) = web_sys::Event::new("hypercolor-verified-daemon-connection-changed") {
        let _ = window.dispatch_event(&event);
    }
}

#[cfg(test)]
fn notify_verified_daemon_connection_change() {}

/// Initialize the bundled app's process-memory daemon transport.
pub fn initialize_daemon_transport() {
    #[cfg(target_arch = "wasm32")]
    {
        if tauri_invoke().is_none() {
            return;
        }
        crate::api::client::begin_native_daemon_verification();

        wasm_bindgen_futures::spawn_local(async {
            let connection = snapshot_after_listener_registration(
                subscribe_verified_daemon_connection_events(),
                || async {
                    let invoke = tauri_invoke()?;
                    invoke_command(&invoke, VERIFIED_DAEMON_CONNECTION_COMMAND, None)
                        .await
                        .ok()
                        .and_then(|value| serde_json_from_js_value(value).ok())
                },
            )
            .await;
            if let Some(snapshot) = connection {
                apply_verified_daemon_connection(snapshot);
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
async fn subscribe_verified_daemon_connection_events() -> bool {
    let Some(window) = browser_window() else {
        return false;
    };
    let Some(listen) = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("__TAURI__"))
        .ok()
        .and_then(|tauri| js_sys::Reflect::get(&tauri, &JsValue::from_str("event")).ok())
        .and_then(|event| js_sys::Reflect::get(&event, &JsValue::from_str("listen")).ok())
        .and_then(|listen| listen.dyn_into::<js_sys::Function>().ok())
    else {
        return false;
    };
    let callback = Closure::<dyn FnMut(JsValue)>::new(|value| {
        if let Ok(event) = serde_json_from_js_value::<VerifiedDaemonConnectionEvent>(value) {
            apply_verified_daemon_connection(event.payload);
        }
    });
    let Ok(registration) = listen.call2(
        &JsValue::NULL,
        &JsValue::from_str(VERIFIED_DAEMON_CONNECTION_EVENT),
        callback.as_ref(),
    ) else {
        return false;
    };
    let Ok(registration) = registration.dyn_into::<js_sys::Promise>() else {
        return false;
    };
    if JsFuture::from(registration).await.is_err() {
        return false;
    }
    callback.forget();
    true
}

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
    pub motherboard: Option<hypercolor_types::motherboard::MotherboardInfo>,
    #[serde(default)]
    pub conflicting_rgb_tools: Vec<ConflictingRgbTool>,
}

/// Competing RGB-control tool detected on the host.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictingRgbTool {
    pub name: String,
    pub identifier: String,
    pub running: bool,
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

/// Result returned after invoking `hypercolor-windows-helper` for a
/// privileged verb (e.g. install/uninstall flows). Exit code 0 means
/// the verb succeeded; any non-zero value should surface as an error
/// toast at the call site.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code, reason = "reserved for future helper invocations")]
pub struct HelperOutcome {
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

/// Local macOS daemon topology selectable through the native app coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosDaemonOwnerChoice {
    AppSidecar,
    DirectLaunchd,
    Homebrew,
    Standalone,
}

impl MacosDaemonOwnerChoice {
    #[cfg(target_arch = "wasm32")]
    const fn invoke_value(self) -> &'static str {
        match self {
            Self::AppSidecar => "app_sidecar",
            Self::DirectLaunchd => "direct_launchd",
            Self::Homebrew => "homebrew",
            Self::Standalone => "standalone",
        }
    }
}

/// Topology-specific local action attached to an owner-coordinator outcome.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacosOwnerRemedy {
    StopStandaloneOwner {
        pid: u32,
    },
    RestartStandalone {
        pid: u32,
    },
    StartAppSidecar,
    StartLaunchdService,
    StartHomebrewService,
    #[serde(other)]
    Unknown,
}

/// Synchronous result from the native daemon-owner coordinator.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosOwnerCoordinatorOutcome {
    Active {
        owner: String,
        owner_epoch: u64,
    },
    PendingStandalone {
        requested_owner: String,
        remedy: MacosOwnerRemedy,
    },
    RolledBack {
        prior_owner: String,
        failure: String,
    },
    RecoveryRequired {
        requested_owner: String,
        prior_owner: String,
        phase: String,
    },
    #[serde(other)]
    Unknown,
}

/// Native app status for a selected external daemon that is offline.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MacosDaemonOwnerOfflineStatus {
    pub code: String,
    pub selected_owner: String,
    pub remedy: MacosOwnerRemedy,
}

/// Successful execution of a selected external owner's local start remedy.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MacosDaemonOwnerOfflineRemedyOutcome {
    pub status: String,
    pub owner: String,
}

/// Result from explicitly restarting the active macOS protected-source owner.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosCaptureOwnerRestartOutcome {
    Restarted {
        owner: String,
        previous_owner_epoch: u64,
        owner_epoch: u64,
    },
    UserActionRequired {
        owner: String,
        owner_epoch: u64,
        remedy: MacosOwnerRemedy,
    },
    #[serde(other)]
    Unknown,
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

/// Select the active macOS daemon topology through the local app coordinator.
///
/// `Ok(None)` means the UI is running in a browser and must not mutate local
/// process or autostart state.
///
/// # Errors
///
/// Returns an error when the native coordinator rejects the handover or its
/// result cannot be decoded.
#[cfg(target_arch = "wasm32")]
pub async fn choose_macos_daemon_owner(
    owner: MacosDaemonOwnerChoice,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let args = string_arg_to_js("requestedOwner", owner.invoke_value())?;
    let value = invoke_command(&invoke, "choose_daemon_owner", Some(args)).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn choose_macos_daemon_owner(
    _owner: MacosDaemonOwnerChoice,
) -> Result<Option<MacosOwnerCoordinatorOutcome>, String> {
    Ok(None)
}

/// Read app-local status for a selected external macOS daemon that is offline.
///
/// # Errors
///
/// Returns an error when the native command rejects or returns malformed data.
#[cfg(target_arch = "wasm32")]
pub async fn macos_daemon_owner_offline_status()
-> Result<Option<MacosDaemonOwnerOfflineStatus>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let value = invoke_command(&invoke, "macos_daemon_owner_offline_status", None).await?;
    serde_json_from_js_value(value)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn macos_daemon_owner_offline_status()
-> Result<Option<MacosDaemonOwnerOfflineStatus>, String> {
    Ok(None)
}

/// Execute the exact app-local start remedy published for an offline owner.
///
/// `Ok(None)` means the browser UI has no local process authority.
///
/// # Errors
///
/// Returns an error when the remedy is stale, mismatched, unsupported, or the
/// selected service cannot be started.
#[cfg(target_arch = "wasm32")]
pub async fn execute_macos_daemon_owner_offline_remedy(
    remedy: &MacosOwnerRemedy,
) -> Result<Option<MacosDaemonOwnerOfflineRemedyOutcome>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let args = macos_owner_remedy_to_js(remedy)?;
    let value = invoke_command(
        &invoke,
        "execute_macos_daemon_owner_offline_remedy",
        Some(args),
    )
    .await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn execute_macos_daemon_owner_offline_remedy(
    _remedy: &MacosOwnerRemedy,
) -> Result<Option<MacosDaemonOwnerOfflineRemedyOutcome>, String> {
    Ok(None)
}

/// Restart the exact active macOS owner after a positive grant requires it.
///
/// `Ok(None)` means the browser UI has no local process authority.
///
/// # Errors
///
/// Returns an error when owner identity changed, the epoch is stale, or the
/// managed owner cannot complete the restart.
#[cfg(target_arch = "wasm32")]
pub async fn restart_macos_capture_owner(
    active_owner: &str,
    owner_epoch: u64,
) -> Result<Option<MacosCaptureOwnerRestartOutcome>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let args = macos_capture_owner_restart_to_js(active_owner, owner_epoch)?;
    let value = invoke_command(&invoke, "restart_macos_capture_owner", Some(args)).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn restart_macos_capture_owner(
    _active_owner: &str,
    _owner_epoch: u64,
) -> Result<Option<MacosCaptureOwnerRestartOutcome>, String> {
    Ok(None)
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

/// True when the welcome overlay should be shown. Returns `Ok(None)`
/// when the Tauri bridge isn't present (browser/dev mode without the
/// native shell) so callers can keep the dashboard rendering normally.
///
/// # Errors
///
/// Returns an error when the native command rejects or returns malformed
/// data.
#[cfg(target_arch = "wasm32")]
pub async fn is_first_run_pending() -> Result<Option<bool>, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(None);
    };

    let value = invoke_command(&invoke, "is_first_run_pending", None).await?;
    serde_json_from_js_value(value).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn is_first_run_pending() -> Result<Option<bool>, String> {
    Ok(None)
}

/// Persist that the welcome wizard has been dismissed.
///
/// # Errors
///
/// Returns an error when the Tauri bridge is unavailable or the native
/// command rejects.
#[cfg(target_arch = "wasm32")]
pub async fn mark_first_run_complete() -> Result<(), String> {
    let Some(invoke) = tauri_invoke() else {
        return Err("native app bridge is unavailable".to_owned());
    };

    let _ = invoke_command(&invoke, "mark_first_run_complete", None).await?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn mark_first_run_complete() -> Result<(), String> {
    Err("native app bridge is unavailable".to_owned())
}

/// Clear the first-run marker so the welcome overlay shows again on
/// the next launch.
///
/// # Errors
///
/// Returns an error when the Tauri bridge is unavailable or the native
/// command rejects.
#[cfg(target_arch = "wasm32")]
pub async fn reset_first_run() -> Result<(), String> {
    let Some(invoke) = tauri_invoke() else {
        return Err("native app bridge is unavailable".to_owned());
    };

    let _ = invoke_command(&invoke, "reset_first_run", None).await?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reset_first_run() -> Result<(), String> {
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

/// Open a limited macOS privacy remedy through the native app bridge.
///
/// Returns `Ok(false)` when the UI is not running inside the native app.
#[cfg(target_arch = "wasm32")]
pub async fn open_macos_system_settings(pane: MacosSystemSettingsPane) -> Result<bool, String> {
    let Some(invoke) = tauri_invoke() else {
        return Ok(false);
    };

    let args = string_arg_to_js("pane", pane.invoke_value())?;
    let _ = invoke_command(&invoke, MACOS_SYSTEM_SETTINGS_COMMAND, Some(args)).await?;
    Ok(true)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn open_macos_system_settings(_pane: MacosSystemSettingsPane) -> Result<bool, String> {
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
fn macos_owner_remedy_to_js(remedy: &MacosOwnerRemedy) -> Result<JsValue, String> {
    let kind = match remedy {
        MacosOwnerRemedy::StartLaunchdService => "start_launchd_service",
        MacosOwnerRemedy::StartHomebrewService => "start_homebrew_service",
        MacosOwnerRemedy::StopStandaloneOwner { .. }
        | MacosOwnerRemedy::RestartStandalone { .. }
        | MacosOwnerRemedy::StartAppSidecar
        | MacosOwnerRemedy::Unknown => {
            return Err("offline owner remedy cannot be executed by this action".to_owned());
        }
    };
    let root = js_sys::Object::new();
    let inner = js_sys::Object::new();
    js_sys::Reflect::set(&inner, &JsValue::from_str("kind"), &JsValue::from_str(kind))
        .map_err(js_error_string)?;
    js_sys::Reflect::set(&root, &JsValue::from_str("remedy"), &inner).map_err(js_error_string)?;
    Ok(root.into())
}

#[cfg(target_arch = "wasm32")]
fn macos_capture_owner_restart_to_js(
    active_owner: &str,
    owner_epoch: u64,
) -> Result<JsValue, String> {
    let root = js_sys::Object::new();
    js_sys::Reflect::set(
        &root,
        &JsValue::from_str("activeOwner"),
        &JsValue::from_str(active_owner),
    )
    .map_err(js_error_string)?;
    js_sys::Reflect::set(
        &root,
        &JsValue::from_str("ownerEpoch"),
        &JsValue::from_f64(owner_epoch as f64),
    )
    .map_err(js_error_string)?;
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
    use std::{
        cell::{Cell, RefCell},
        future::Future,
        task::{Context, Poll, Waker},
    };

    use super::{
        MACOS_SYSTEM_SETTINGS_COMMAND, MacosOwnerCoordinatorOutcome, MacosOwnerRemedy,
        MacosSystemSettingsPane, PawnIoModuleStatus, PawnIoSupportStatus, ServiceSupportStatus,
        VERIFIED_DAEMON_CONNECTION_COMMAND, VERIFIED_DAEMON_REVISION, VerifiedDaemonConnection,
        VerifiedDaemonConnectionSnapshot, apply_verified_daemon_connection, bundled_payload_ready,
        smbus_support_ready, snapshot_after_listener_registration, windows_daemon_service_conflict,
    };

    fn run_ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("fixture future should resolve without suspension"),
        }
    }

    #[test]
    fn snapshot_request_waits_for_listener_registration_and_skips_rejection() {
        let steps = RefCell::new(Vec::new());
        let snapshot = run_ready(snapshot_after_listener_registration(
            async {
                steps.borrow_mut().push("listener-ready");
                true
            },
            || async {
                assert_eq!(steps.borrow().as_slice(), &["listener-ready"]);
                steps.borrow_mut().push("snapshot-requested");
                Some(42_u8)
            },
        ));
        assert_eq!(snapshot, Some(42));
        assert_eq!(
            steps.into_inner(),
            vec!["listener-ready", "snapshot-requested"]
        );

        let snapshot_requested = Cell::new(false);
        let rejected = run_ready(snapshot_after_listener_registration(
            async { false },
            || async {
                snapshot_requested.set(true);
                Some(42_u8)
            },
        ));
        assert_eq!(rejected, None);
        assert!(!snapshot_requested.get());
    }

    #[test]
    fn verified_connection_command_installs_and_rotates_only_process_memory() {
        assert_eq!(
            VERIFIED_DAEMON_CONNECTION_COMMAND,
            "get_verified_daemon_connection"
        );
        VERIFIED_DAEMON_REVISION.with(|revision| revision.set(0));
        crate::api::client::begin_native_daemon_verification();
        let connection: VerifiedDaemonConnection = serde_json::from_value(serde_json::json!({
            "baseUrl": "http://127.0.0.1:9420",
            "serverSessionId": "hcs1_11111111111111111111111111111111",
            "protectedControlCredential": format!("hcc1_{}", "22".repeat(32)),
        }))
        .expect("verified native connection should decode");
        assert!(apply_verified_daemon_connection(
            VerifiedDaemonConnectionSnapshot {
                revision: 2,
                connection: Some(connection),
            }
        ));
        assert_eq!(
            crate::api::client::daemon_url("/api/v1/devices"),
            Some("http://127.0.0.1:9420/api/v1/devices".to_owned())
        );
        assert!(crate::api::client::authorization_token().is_some());

        assert!(!apply_verified_daemon_connection(
            VerifiedDaemonConnectionSnapshot {
                revision: 1,
                connection: None,
            }
        ));
        assert!(crate::api::client::daemon_url("/api/v1/devices").is_some());

        assert!(apply_verified_daemon_connection(
            VerifiedDaemonConnectionSnapshot {
                revision: 3,
                connection: None,
            }
        ));
        assert!(crate::api::client::authorization_token().is_none());
        assert!(crate::api::client::daemon_url("/api/v1/devices").is_none());
        crate::api::client::reset_daemon_transport_for_test();
    }

    #[test]
    fn health_verified_native_connection_routes_without_protected_credential() {
        VERIFIED_DAEMON_REVISION.with(|revision| revision.set(0));
        crate::api::client::begin_native_daemon_verification();
        assert!(crate::api::client::daemon_url("/api/v1/system").is_none());

        let connection: VerifiedDaemonConnection = serde_json::from_value(serde_json::json!({
            "baseUrl": "https://daemon.lan:19420",
            "serverSessionId": null,
            "protectedControlCredential": null,
        }))
        .expect("health-proven native connection should decode");
        assert!(apply_verified_daemon_connection(
            VerifiedDaemonConnectionSnapshot {
                revision: 1,
                connection: Some(connection),
            }
        ));
        assert_eq!(
            crate::api::client::daemon_url("/api/v1/system"),
            Some("https://daemon.lan:19420/api/v1/system".to_owned())
        );
        assert!(crate::api::client::authorization_token().is_none());
        crate::api::client::reset_daemon_transport_for_test();
    }

    #[test]
    fn macos_system_settings_panes_route_to_the_scoped_native_command() {
        assert_eq!(MACOS_SYSTEM_SETTINGS_COMMAND, "open_macos_system_settings");
        assert_eq!(
            MacosSystemSettingsPane::InputMonitoring.invoke_value(),
            "input_monitoring"
        );
        assert_eq!(
            MacosSystemSettingsPane::ScreenRecording.invoke_value(),
            "screen_recording"
        );
    }

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

    #[test]
    fn macos_owner_outcomes_decode_closed_native_shapes() {
        let active: MacosOwnerCoordinatorOutcome = serde_json::from_value(serde_json::json!({
            "status": "active",
            "owner": "homebrew",
            "owner_epoch": 9
        }))
        .expect("active owner outcome should decode");
        assert_eq!(
            active,
            MacosOwnerCoordinatorOutcome::Active {
                owner: "homebrew".to_owned(),
                owner_epoch: 9,
            }
        );

        let pending: MacosOwnerCoordinatorOutcome = serde_json::from_value(serde_json::json!({
            "status": "pending_standalone",
            "requested_owner": "app_sidecar",
            "remedy": {
                "kind": "stop_standalone_owner",
                "pid": 412
            }
        }))
        .expect("pending standalone outcome should decode");
        assert!(matches!(
            pending,
            MacosOwnerCoordinatorOutcome::PendingStandalone {
                remedy: MacosOwnerRemedy::StopStandaloneOwner { pid: 412 },
                ..
            }
        ));

        let restart: MacosOwnerCoordinatorOutcome = serde_json::from_value(serde_json::json!({
            "status": "pending_standalone",
            "requested_owner": "app_sidecar",
            "remedy": {
                "kind": "restart_standalone",
                "pid": 77
            }
        }))
        .expect("restart standalone outcome should decode");
        assert!(matches!(
            restart,
            MacosOwnerCoordinatorOutcome::PendingStandalone {
                remedy: MacosOwnerRemedy::RestartStandalone { pid: 77 },
                ..
            }
        ));
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
            motherboard: Some(hypercolor_types::motherboard::MotherboardInfo {
                manufacturer: "ASUSTeK COMPUTER INC.".to_string(),
                product: "ROG STRIX X670E-E GAMING WIFI".to_string(),
                version: Some("Rev 1.xx".to_string()),
            }),
            conflicting_rgb_tools: Vec::new(),
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
