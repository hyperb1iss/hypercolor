//! The launchd agents of a managed app bundle installation, which belong to
//! the installer: an update never rewrites what its own rollback runs under.

use std::path::Path;

use super::super::InstallPlatformError;
use super::model::{MacosBundleConfig, error};

/// The direct agent, which runs the daemon inside the installed bundle.
pub const MACOS_BUNDLE_DAEMON_LABEL: &str = "tech.hyperbliss.hypercolor";
/// Publishes a staged release as a unit, started by `launchctl kickstart`.
pub const MACOS_BUNDLE_UPDATE_PREPARE_LABEL: &str = "tech.hyperbliss.hypercolor.update-prepare";
/// Runs an armed transaction, started by `launchctl kickstart`.
pub const MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL: &str = "tech.hyperbliss.hypercolor.update-activate";
/// Finishes an interrupted transaction whenever launchd sees it pending.
pub const MACOS_BUNDLE_UPDATE_RECOVER_LABEL: &str = "tech.hyperbliss.hypercolor.update-recover";

const APP_IDENTIFIER: &str = "tech.hyperbliss.hypercolor";
const SERVICE_IDENTITY: &str = "user_service:launchd:tech.hyperbliss.hypercolor";
const DAEMON_PATH: &str = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin";
/// launchd's own pace between starts of a daemon that exited.
const DAEMON_THROTTLE_SECONDS: u32 = 3;
/// How long a recovery that found the lock held waits to try again.
const RECOVER_THROTTLE_SECONDS: u32 = 10;
/// `0o077`: what an activator writes is this user's alone.
const UPDATE_UMASK: u32 = 63;

/// One agent's property list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleAgentDefinition {
    pub label: &'static str,
    pub bytes: Vec<u8>,
}

impl MacosBundleAgentDefinition {
    /// The definition's file in `~/Library/LaunchAgents`.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.plist", self.label)
    }
}

/// The direct agent of `config`'s bundle.
///
/// The path names the installed bundle, never a release, so a swap changes
/// which daemon it starts without touching this file.
///
/// # Errors
/// Returns an error for a path that is not UTF-8.
pub fn macos_bundle_daemon_agent(
    config: &MacosBundleConfig,
    logs: &Path,
) -> Result<MacosBundleAgentDefinition, InstallPlatformError> {
    let resources = config.installed_bundle.join("Contents/Resources");
    let daemon = config.installed_daemon();
    let ui = resources.join("ui");
    let effects = resources.join("effects/bundled");
    let log = logs.join("hypercolor.log");
    let body = format!(
        "{program}<key>RunAtLoad</key><true/>\n\
         <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
         <key>ThrottleInterval</key><integer>{DAEMON_THROTTLE_SECONDS}</integer>\n\
         {output}\
         <key>EnvironmentVariables</key><dict>\
         <key>HYPERCOLOR_MACOS_OWNER</key><string>direct-launchd</string>\
         <key>HYPERCOLOR_SERVICE_IDENTITY</key><string>{SERVICE_IDENTITY}</string>\
         <key>HYPERCOLOR_LOG</key><string>info</string>\
         <key>PATH</key><string>{DAEMON_PATH}</string></dict>\n\
         <key>ProcessType</key><string>Standard</string>\n\
         <key>LowPriorityBackgroundIO</key><true/>\n",
        program = program_arguments(&[
            text(&daemon)?,
            "--macos-owner",
            "direct-launchd",
            "--ui-dir",
            text(&ui)?,
            "--effects-dir",
            text(&effects)?,
        ]),
        output = output_paths(text(&log)?),
    );
    Ok(definition(MACOS_BUNDLE_DAEMON_LABEL, &body))
}

/// The three update agents. Each runs the stable launcher, so a recovery
/// runs the transaction's prior release whatever the active pointer names.
///
/// Prepare and activate only run when kickstarted. Recover is held up by
/// launchd for as long as the marker at `pending` exists, which covers a
/// login, a killed activator and a power cut alike.
///
/// # Errors
/// Returns an error for a path that is not UTF-8.
pub fn macos_bundle_update_agents(
    launcher: &Path,
    pending: &Path,
    logs: &Path,
) -> Result<[MacosBundleAgentDefinition; 3], InstallPlatformError> {
    let recover = format!(
        "<key>KeepAlive</key><dict><key>PathState</key><dict>\
         <key>{pending}</key><true/></dict></dict>\n\
         <key>ThrottleInterval</key><integer>{RECOVER_THROTTLE_SECONDS}</integer>\n",
        pending = escape(text(pending)?),
    );
    Ok([
        update_agent(
            MACOS_BUNDLE_UPDATE_PREPARE_LABEL,
            "prepare",
            launcher,
            logs,
            "",
        )?,
        update_agent(
            MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL,
            "activate",
            launcher,
            logs,
            "",
        )?,
        update_agent(
            MACOS_BUNDLE_UPDATE_RECOVER_LABEL,
            "recover",
            launcher,
            logs,
            &recover,
        )?,
    ])
}

fn update_agent(
    label: &'static str,
    mode: &str,
    launcher: &Path,
    logs: &Path,
    schedule: &str,
) -> Result<MacosBundleAgentDefinition, InstallPlatformError> {
    let log = logs.join(format!("update-{mode}.log"));
    let body = format!(
        "{program}{schedule}\
         <key>Umask</key><integer>{UPDATE_UMASK}</integer>\n\
         {output}\
         <key>EnvironmentVariables</key><dict>\
         <key>HYPERCOLOR_LOG</key><string>info</string></dict>\n",
        program = program_arguments(&[
            text(launcher)?,
            "--role",
            "update-executor",
            "--",
            "update",
            "activator",
            mode,
        ]),
        output = output_paths(text(&log)?),
    );
    Ok(definition(label, &body))
}

/// Associating the agent with the app files it under Hypercolor in System
/// Settings, rather than under the signer of whatever program it runs.
fn definition(label: &'static str, body: &str) -> MacosBundleAgentDefinition {
    MacosBundleAgentDefinition {
        label,
        bytes: format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{label}</string>\n\
             <key>AssociatedBundleIdentifiers</key><string>{APP_IDENTIFIER}</string>\n\
             {body}</dict></plist>\n"
        )
        .into_bytes(),
    }
}

fn program_arguments(arguments: &[&str]) -> String {
    let mut list = String::from("<key>ProgramArguments</key><array>");
    for argument in arguments {
        list.push_str("<string>");
        list.push_str(&escape(argument));
        list.push_str("</string>");
    }
    list.push_str("</array>\n");
    list
}

fn output_paths(log: &str) -> String {
    let log = escape(log);
    format!(
        "<key>StandardOutPath</key><string>{log}</string>\n\
         <key>StandardErrorPath</key><string>{log}</string>\n"
    )
}

fn text(path: &Path) -> Result<&str, InstallPlatformError> {
    path.to_str()
        .ok_or_else(|| error("a launchd agent path is not UTF-8"))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
