//! Native support helpers exposed to the app-hosted web UI.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "windows")]
const PAWNIO_HOME_ENV: &str = "HYPERCOLOR_PAWNIO_HOME";
#[cfg(target_os = "windows")]
const PAWNIO_DLL_NAME: &str = "PawnIOLib.dll";
const PAWNIO_SERVICE_NAME: &str = "PawnIO";
const SMBUS_SERVICE_NAME: &str = "HypercolorSmBus";
const HARDWARE_SUPPORT_SCRIPT_NAME: &str = "install-windows-hardware-support.ps1";
const PAWNIO_SETUP_NAME: &str = "PawnIO_setup.exe";
const SMBUS_SERVICE_EXE_NAME: &str = "hypercolor-smbus-service.exe";
const REQUIRED_PAWNIO_MODULES: [&str; 3] = ["SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin"];

/// Status for a Windows service used by hardware support.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSupportStatus {
    /// Whether the service exists.
    pub installed: bool,
    /// Service state reported by `sc.exe`, such as `RUNNING` or `STOPPED`.
    pub state: Option<String>,
}

impl ServiceSupportStatus {
    fn missing() -> Self {
        Self {
            installed: false,
            state: None,
        }
    }
}

/// Bundled PawnIO module availability.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoModuleStatus {
    /// Module filename.
    pub name: String,
    /// Whether this module exists in the bundled payload directory.
    pub bundled: bool,
}

/// PawnIO and SMBus broker readiness as seen by the native app shell.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoSupportStatus {
    /// Whether the current platform can install/use PawnIO support.
    pub platform_supported: bool,
    /// Installed PawnIO runtime directory, when detected.
    pub pawnio_home: Option<String>,
    /// Whether `PawnIOLib.dll` was found.
    pub pawnio_runtime_installed: bool,
    /// Status of the third-party PawnIO service.
    pub pawnio_service: ServiceSupportStatus,
    /// Status of the narrow Hypercolor SMBus broker service.
    pub smbus_service: ServiceSupportStatus,
    /// Resource root containing the bundled PawnIO payload.
    pub bundled_asset_root: Option<String>,
    /// Bundled installer helper path.
    pub helper_script: Option<String>,
    /// Bundled SMBus broker binary path.
    pub broker_executable: Option<String>,
    /// Whether the bundled PawnIO installer exists.
    pub bundled_installer_available: bool,
    /// Per-module bundled payload status.
    pub bundled_modules: Vec<PawnIoModuleStatus>,
    /// Whether the full local install payload is available.
    pub install_available: bool,
}

/// User-selected options for the elevated Windows hardware support helper.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PawnIoHelperOptions {
    /// Force re-running the PawnIO installer even when the runtime is detected.
    pub force_pawn_io: bool,
    /// Pass PawnIO's silent installer flag.
    pub silent: bool,
    /// Replace an existing `HypercolorSmBus` service registration.
    pub reinstall_service: bool,
    /// Install but do not start the `HypercolorSmBus` service.
    pub no_start_service: bool,
}

/// Platform-neutral process command model for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportCommand {
    /// Program to execute.
    pub program: PathBuf,
    /// Command-line arguments.
    pub args: Vec<String>,
}

/// Result returned after launching the hardware support helper.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PawnIoHelperLaunchResult {
    /// Helper process exit code. `None` means the process ended by signal or OS policy.
    pub exit_code: Option<i32>,
}

/// Detect PawnIO and SMBus support status for the app UI.
///
/// # Errors
///
/// Returns a stringified Tauri path error when the app resource directory cannot
/// be queried. The command still succeeds when the resource directory is absent.
#[tauri::command]
pub fn detect_pawnio_support(app: AppHandle) -> Result<PawnIoSupportStatus, String> {
    let resource_dir = app.path().resource_dir().ok();
    Ok(detect_pawnio_support_from_resource_dir(
        resource_dir.as_deref(),
    ))
}

/// Launch the bundled elevated PawnIO + SMBus broker installer.
///
/// # Errors
///
/// Returns an error when the platform is unsupported, the bundled helper payload
/// is incomplete, or the helper process fails to launch.
#[tauri::command]
pub async fn launch_pawnio_helper(
    app: AppHandle,
    options: Option<PawnIoHelperOptions>,
) -> Result<PawnIoHelperLaunchResult, String> {
    let resource_dir = app.path().resource_dir().ok();
    let options = options.unwrap_or_default();

    tokio::task::spawn_blocking(move || {
        launch_pawnio_helper_from_resource_dir(resource_dir.as_deref(), options)
    })
    .await
    .map_err(|error| format!("hardware support helper task failed: {error}"))?
    .map_err(|error| error.to_string())
}

/// Detect PawnIO and SMBus support status from an optional Tauri resource root.
#[must_use]
pub fn detect_pawnio_support_from_resource_dir(resource_dir: Option<&Path>) -> PawnIoSupportStatus {
    let tools_dir = resource_dir.map(tools_dir);
    let asset_root = tools_dir.as_deref().map(pawnio_asset_root);
    let helper_script = tools_dir.as_deref().map(hardware_support_script_path);
    let broker_executable = tools_dir.as_deref().map(smbus_service_executable_path);
    let bundled_modules = bundled_module_status(asset_root.as_deref());
    let bundled_installer_available = asset_root
        .as_deref()
        .is_some_and(|root| pawnio_setup_path(root).is_file());
    let bundled_modules_available = bundled_modules.iter().all(|module| module.bundled);
    let helper_available = helper_script.as_ref().is_some_and(|path| path.is_file());
    let broker_available = broker_executable
        .as_ref()
        .is_some_and(|path| path.is_file());
    let pawnio_home = resolve_pawnio_home();
    let platform_supported = cfg!(target_os = "windows");

    PawnIoSupportStatus {
        platform_supported,
        pawnio_runtime_installed: pawnio_home.is_some(),
        pawnio_home: pawnio_home.as_deref().map(path_to_string),
        pawnio_service: query_service_status(PAWNIO_SERVICE_NAME),
        smbus_service: query_service_status(SMBUS_SERVICE_NAME),
        bundled_asset_root: asset_root.as_deref().map(path_to_string),
        helper_script: helper_script.as_deref().map(path_to_string),
        broker_executable: broker_executable.as_deref().map(path_to_string),
        bundled_installer_available,
        bundled_modules,
        install_available: platform_supported
            && helper_available
            && broker_available
            && bundled_installer_available
            && bundled_modules_available,
    }
}

/// Build the command used to run the elevated Windows hardware support helper.
#[must_use]
pub fn build_pawnio_helper_command(
    tools_dir: &Path,
    options: PawnIoHelperOptions,
) -> SupportCommand {
    let script = hardware_support_script_path(tools_dir);
    let asset_root = pawnio_asset_root(tools_dir);
    let broker_executable = smbus_service_executable_path(tools_dir);
    let mut args = vec![
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-ExecutionPolicy".to_owned(),
        "Bypass".to_owned(),
        "-File".to_owned(),
        script.display().to_string(),
        "-AssetRoot".to_owned(),
        asset_root.display().to_string(),
        "-BrokerExe".to_owned(),
        broker_executable.display().to_string(),
    ];

    if options.force_pawn_io {
        args.push("-ForcePawnIo".to_owned());
    }
    if options.silent {
        args.push("-Silent".to_owned());
    }
    if options.reinstall_service {
        args.push("-ReinstallService".to_owned());
    }
    if options.no_start_service {
        args.push("-NoStartService".to_owned());
    }

    SupportCommand {
        program: PathBuf::from("powershell.exe"),
        args,
    }
}

/// Launch the bundled elevated Windows hardware support helper.
///
/// # Errors
///
/// Returns an error when the platform is unsupported, the Tauri resource
/// directory is unavailable, the bundle payload is incomplete, or PowerShell
/// cannot launch the helper process.
pub fn launch_pawnio_helper_from_resource_dir(
    resource_dir: Option<&Path>,
    options: PawnIoHelperOptions,
) -> Result<PawnIoHelperLaunchResult> {
    if !cfg!(target_os = "windows") {
        bail!("PawnIO hardware support is only available on Windows");
    }

    let resource_dir = resource_dir.context("Tauri resource directory is unavailable")?;
    let tools_dir = tools_dir(resource_dir);
    validate_pawnio_helper_payload(&tools_dir)?;

    let command = build_pawnio_helper_command(&tools_dir, options);
    let status = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("failed to launch {}", command.program.display()))?;

    Ok(PawnIoHelperLaunchResult {
        exit_code: status.code(),
    })
}

/// Parse the service state from `sc.exe query` output.
#[must_use]
pub fn parse_sc_query_state(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        if !line.starts_with("STATE") {
            return None;
        }
        line.split_whitespace().last().map(str::to_owned)
    })
}

fn validate_pawnio_helper_payload(tools_dir: &Path) -> Result<()> {
    let script = hardware_support_script_path(tools_dir);
    if !script.is_file() {
        bail!(
            "bundled hardware support helper is missing: {}",
            script.display()
        );
    }

    let broker = smbus_service_executable_path(tools_dir);
    if !broker.is_file() {
        bail!("bundled SMBus broker is missing: {}", broker.display());
    }

    let asset_root = pawnio_asset_root(tools_dir);
    let setup = pawnio_setup_path(&asset_root);
    if !setup.is_file() {
        bail!("bundled PawnIO installer is missing: {}", setup.display());
    }

    let missing_modules: Vec<_> = bundled_module_status(Some(&asset_root))
        .into_iter()
        .filter(|module| !module.bundled)
        .map(|module| module.name)
        .collect();
    if !missing_modules.is_empty() {
        bail!(
            "bundled PawnIO modules are missing: {}",
            missing_modules.join(", ")
        );
    }

    Ok(())
}

fn bundled_module_status(asset_root: Option<&Path>) -> Vec<PawnIoModuleStatus> {
    REQUIRED_PAWNIO_MODULES
        .into_iter()
        .map(|name| {
            let bundled = asset_root.is_some_and(|root| root.join("modules").join(name).is_file());
            PawnIoModuleStatus {
                name: name.to_owned(),
                bundled,
            }
        })
        .collect()
}

fn tools_dir(resource_dir: &Path) -> PathBuf {
    resource_dir.join("tools")
}

fn hardware_support_script_path(tools_dir: &Path) -> PathBuf {
    tools_dir.join(HARDWARE_SUPPORT_SCRIPT_NAME)
}

fn pawnio_asset_root(tools_dir: &Path) -> PathBuf {
    tools_dir.join("pawnio")
}

fn pawnio_setup_path(asset_root: &Path) -> PathBuf {
    asset_root.join(PAWNIO_SETUP_NAME)
}

fn smbus_service_executable_path(tools_dir: &Path) -> PathBuf {
    tools_dir.join(SMBUS_SERVICE_EXE_NAME)
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(target_os = "windows")]
fn resolve_pawnio_home() -> Option<PathBuf> {
    pawnio_home_candidates()
        .into_iter()
        .find(|candidate| candidate.join(PAWNIO_DLL_NAME).is_file())
}

#[cfg(not(target_os = "windows"))]
fn resolve_pawnio_home() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "windows")]
fn pawnio_home_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(path) = std::env::var_os(PAWNIO_HOME_ENV).filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }

    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(var).filter(|value| !value.is_empty()) {
            candidates.push(PathBuf::from(root).join("PawnIO"));
        }
    }

    candidates
}

#[cfg(target_os = "windows")]
fn query_service_status(service_name: &str) -> ServiceSupportStatus {
    let output = Command::new("sc.exe")
        .args(["query", service_name])
        .output();
    let Ok(output) = output else {
        return ServiceSupportStatus::missing();
    };

    if !output.status.success() {
        return ServiceSupportStatus::missing();
    }

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    ServiceSupportStatus {
        installed: true,
        state: parse_sc_query_state(&text),
    }
}

#[cfg(not(target_os = "windows"))]
fn query_service_status(_service_name: &str) -> ServiceSupportStatus {
    ServiceSupportStatus::missing()
}
