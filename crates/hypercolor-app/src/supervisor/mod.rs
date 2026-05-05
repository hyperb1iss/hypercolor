//! Daemon supervision primitives for the unified desktop app.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;
use tauri::{AppHandle, Manager, Runtime};
use url::Url;

/// Default daemon bind address used by the app-spawned daemon.
pub const DEFAULT_DAEMON_BIND: &str = "127.0.0.1:9420";

const DAEMON_EXECUTABLE_STEM: &str = "hypercolor-daemon";

/// Linux systemd user service name for the daemon.
pub const SYSTEMD_USER_SERVICE: &str = "hypercolor.service";

/// Timeout for one lightweight health probe.
pub const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Maximum time to wait for an app-spawned daemon to become healthy.
pub const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Delay between daemon startup health probes.
pub const DAEMON_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Platform-neutral command description for spawning the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCommand {
    /// Daemon executable path.
    pub program: PathBuf,
    /// Daemon command-line arguments.
    pub args: Vec<String>,
}

/// Current state of the Linux systemd user service from the app supervisor's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemdUserServiceProbe {
    /// The unit is active, so the app should connect to it instead of spawning a child.
    Active,
    /// The unit is enabled but not currently active, so the app may ask systemd to start it.
    EnabledInactive,
    /// The unit is missing, disabled, or otherwise unavailable for app startup.
    Unavailable,
}

/// Supervisor action selected for a Linux systemd user service probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemdUserServicePlan {
    /// Reuse the already-active service.
    Reuse,
    /// Start the enabled service through systemd.
    Start,
    /// Spawn the bundled daemon as an app-owned child process.
    SpawnChild,
}

/// App-managed daemon supervisor state.
#[derive(Clone, Default)]
pub struct SupervisorState {
    child: Arc<Mutex<Option<ManagedDaemon>>>,
}

impl SupervisorState {
    /// Return the app-owned daemon process ID, if one is running.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        self.child_guard().as_ref().map(ManagedDaemon::id)
    }

    fn replace_child(&self, daemon: ManagedDaemon) {
        *self.child_guard() = Some(daemon);
    }

    fn clear_child(&self) {
        *self.child_guard() = None;
    }

    fn child_guard(&self) -> MutexGuard<'_, Option<ManagedDaemon>> {
        self.child.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// App-owned daemon child process.
pub struct ManagedDaemon {
    child: Child,
    #[allow(dead_code)]
    platform_guard: PlatformGuard,
}

impl ManagedDaemon {
    /// Return the child process ID.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for ManagedDaemon {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Resolve the daemon executable name for the current target.
#[must_use]
pub const fn daemon_executable_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "hypercolor-daemon.exe"
    } else {
        DAEMON_EXECUTABLE_STEM
    }
}

/// Resolve the target triples Tauri sidecars may use for the current target.
#[must_use]
pub const fn target_triple_candidates() -> &'static [&'static str] {
    if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    )) {
        &["x86_64-pc-windows-msvc"]
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "aarch64",
        target_env = "msvc"
    )) {
        &["aarch64-pc-windows-msvc"]
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        &["x86_64-pc-windows-gnu"]
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        &["aarch64-apple-darwin"]
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        &["x86_64-apple-darwin"]
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        &["x86_64-unknown-linux-gnu"]
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        &["aarch64-unknown-linux-gnu"]
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "musl"
    )) {
        &["x86_64-unknown-linux-musl"]
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "musl"
    )) {
        &["aarch64-unknown-linux-musl"]
    } else {
        &[]
    }
}

/// Resolve the Tauri externalBin sidecar name for a target triple.
#[must_use]
pub fn tauri_sidecar_daemon_name(target_triple: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{DAEMON_EXECUTABLE_STEM}-{target_triple}.exe")
    } else {
        format!("{DAEMON_EXECUTABLE_STEM}-{target_triple}")
    }
}

/// Resolve the daemon path next to the app executable.
#[must_use]
pub fn sibling_daemon_path(current_exe: &Path) -> Option<PathBuf> {
    current_exe
        .parent()
        .map(|install_dir| install_dir.join(daemon_executable_name()))
}

/// Resolve likely daemon executable paths for supported package layouts.
#[must_use]
pub fn daemon_path_candidates(current_exe: &Path, resource_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(install_dir) = current_exe.parent() {
        push_daemon_candidates(&mut candidates, install_dir);
    }

    if let Some(resource_dir) = resource_dir {
        push_daemon_candidates(&mut candidates, resource_dir);
    }

    if let Some(resource_dir) = macos_app_resource_dir(current_exe) {
        push_daemon_candidates(&mut candidates, &resource_dir);
    }

    candidates
}

/// Resolve the installed web UI directory next to the app executable.
#[must_use]
pub fn sibling_ui_dir(current_exe: &Path) -> Option<PathBuf> {
    current_exe
        .parent()
        .map(|install_dir| install_dir.join("ui"))
}

/// Resolve likely installed web UI directories for supported package layouts.
#[must_use]
pub fn ui_dir_candidates(current_exe: &Path, resource_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(install_dir) = current_exe.parent() {
        push_unique_path(&mut candidates, install_dir.join("ui"));

        if let Some(prefix_dir) = install_dir.parent() {
            push_share_ui_candidate(&mut candidates, prefix_dir);
        }
    }

    if let Some(resource_dir) = resource_dir {
        push_resource_ui_candidates(&mut candidates, resource_dir);
    }

    if let Some(resource_dir) = macos_app_resource_dir(current_exe) {
        push_resource_ui_candidates(&mut candidates, &resource_dir);
    }

    candidates
}

/// Resolve the macOS `.app` resource directory from a `Contents/MacOS` executable.
#[must_use]
pub fn macos_app_resource_dir(current_exe: &Path) -> Option<PathBuf> {
    let executable_dir = current_exe.parent()?;
    if executable_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return None;
    }

    let contents_dir = executable_dir.parent()?;
    if contents_dir.file_name().and_then(|name| name.to_str()) != Some("Contents") {
        return None;
    }

    Some(contents_dir.join("Resources"))
}

/// Build the daemon command used by the app supervisor.
#[must_use]
pub fn build_daemon_command(
    program: impl Into<PathBuf>,
    bind: &str,
    ui_dir: Option<&Path>,
) -> DaemonCommand {
    let mut args = vec!["--bind".to_owned(), bind.to_owned()];

    if let Some(ui_dir) = ui_dir {
        args.push("--ui-dir".to_owned());
        args.push(ui_dir.display().to_string());
    }

    DaemonCommand {
        program: program.into(),
        args,
    }
}

/// Convert the app's daemon URL into a daemon bind address.
#[must_use]
pub fn bind_from_daemon_url(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Some(format!("{host}:{port}"))
}

/// Resolve the daemon health endpoint from the base daemon URL.
#[must_use]
pub fn health_url(base: &Url) -> Url {
    base.join("/health")
        .expect("static health endpoint path should be valid")
}

/// Probe whether a daemon is already accepting requests.
pub async fn probe_health(client: &reqwest::Client, base: &Url, timeout: Duration) -> bool {
    let response = client.get(health_url(base)).timeout(timeout).send().await;
    response.is_ok_and(|response| response.status().is_success())
}

/// Wait until a daemon reports healthy or the startup timeout expires.
pub async fn wait_until_healthy(
    client: &reqwest::Client,
    base: &Url,
    timeout: Duration,
    poll_interval: Duration,
) -> bool {
    let started = Instant::now();

    loop {
        if probe_health(client, base, HEALTH_PROBE_TIMEOUT).await {
            return true;
        }

        let elapsed = started.elapsed();
        let Some(remaining) = timeout.checked_sub(elapsed) else {
            return false;
        };

        let Some(delay) = startup_retry_delay(remaining, poll_interval) else {
            return false;
        };

        tokio::time::sleep(delay).await;
    }
}

/// Cap a startup retry delay at the remaining startup budget.
#[must_use]
pub fn startup_retry_delay(remaining: Duration, poll_interval: Duration) -> Option<Duration> {
    if remaining.is_zero() {
        None
    } else {
        Some(remaining.min(poll_interval))
    }
}

/// Parse `systemctl --user is-active` output.
#[must_use]
pub fn systemctl_is_active_output(output: &str) -> bool {
    first_systemctl_output_line(output) == "active"
}

/// Parse `systemctl --user is-enabled` output for states that represent an
/// installed unit intended to be user-managed.
#[must_use]
pub fn systemctl_is_enabled_output(output: &str) -> bool {
    matches!(
        first_systemctl_output_line(output),
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias"
    )
}

/// Select the supervisor action for a Linux systemd user service probe.
#[must_use]
pub const fn systemd_user_service_plan(probe: SystemdUserServiceProbe) -> SystemdUserServicePlan {
    match probe {
        SystemdUserServiceProbe::Active => SystemdUserServicePlan::Reuse,
        SystemdUserServiceProbe::EnabledInactive => SystemdUserServicePlan::Start,
        SystemdUserServiceProbe::Unavailable => SystemdUserServicePlan::SpawnChild,
    }
}

fn first_systemctl_output_line(output: &str) -> &str {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// Start supervising the daemon in the background.
///
/// # Errors
///
/// Returns an error if the app executable path or daemon URL cannot be resolved.
pub fn start<R: Runtime>(app: &AppHandle<R>, daemon_url: Url) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to resolve app executable path")?;
    let resource_dir = app.path().resource_dir().ok();
    let daemon_candidates = daemon_path_candidates(&current_exe, resource_dir.as_deref());
    let daemon_path = daemon_candidates
        .iter()
        .find(|path| path.is_file())
        .or_else(|| daemon_candidates.first())
        .cloned()
        .context("failed to resolve daemon path from app executable or resource directory")?;
    let ui_dir = ui_dir_candidates(&current_exe, resource_dir.as_deref())
        .into_iter()
        .find(|path| path.join("index.html").exists());
    let bind = bind_from_daemon_url(&daemon_url).unwrap_or_else(|| DEFAULT_DAEMON_BIND.to_owned());
    let state = app.state::<SupervisorState>().inner().clone();

    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        if probe_health(&client, &daemon_url, HEALTH_PROBE_TIMEOUT).await {
            tracing::info!(url = %daemon_url, "daemon already running; reusing existing instance");
            return;
        }

        #[cfg(target_os = "linux")]
        if try_start_systemd_user_service(&client, &daemon_url).await {
            return;
        }

        if !daemon_path.is_file() {
            tracing::warn!(
                path = %daemon_path.display(),
                "daemon executable not found; skipping app-owned daemon spawn"
            );
            return;
        }

        let command = build_daemon_command(&daemon_path, &bind, ui_dir.as_deref());
        match spawn_daemon(&command) {
            Ok(daemon) => {
                let pid = daemon.id();
                state.replace_child(daemon);
                tracing::info!(pid, "app-owned daemon spawned");
                if wait_until_healthy(
                    &client,
                    &daemon_url,
                    DAEMON_STARTUP_TIMEOUT,
                    DAEMON_STARTUP_POLL_INTERVAL,
                )
                .await
                {
                    tracing::info!(pid, "app-owned daemon reported healthy");
                } else {
                    tracing::warn!(
                        pid,
                        timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                        "app-owned daemon did not become healthy before timeout"
                    );
                    state.clear_child();
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to spawn app-owned daemon");
            }
        }
    });

    Ok(())
}

#[cfg(target_os = "linux")]
async fn try_start_systemd_user_service(client: &reqwest::Client, daemon_url: &Url) -> bool {
    match systemd_user_service_plan(detect_systemd_user_service()) {
        SystemdUserServicePlan::Reuse => {
            tracing::info!(
                service = SYSTEMD_USER_SERVICE,
                "systemd user service active; waiting for daemon health"
            );
            if wait_until_healthy(
                client,
                daemon_url,
                DAEMON_STARTUP_TIMEOUT,
                DAEMON_STARTUP_POLL_INTERVAL,
            )
            .await
            {
                tracing::info!(
                    service = SYSTEMD_USER_SERVICE,
                    "systemd-managed daemon reported healthy"
                );
            } else {
                tracing::warn!(
                    service = SYSTEMD_USER_SERVICE,
                    timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                    "systemd user service is active but the daemon did not become healthy"
                );
            }
            true
        }
        SystemdUserServicePlan::Start => {
            tracing::info!(
                service = SYSTEMD_USER_SERVICE,
                "starting enabled systemd user service"
            );
            match start_systemd_user_service() {
                Ok(status) if status.success() => {
                    if wait_until_healthy(
                        client,
                        daemon_url,
                        DAEMON_STARTUP_TIMEOUT,
                        DAEMON_STARTUP_POLL_INTERVAL,
                    )
                    .await
                    {
                        tracing::info!(
                            service = SYSTEMD_USER_SERVICE,
                            "systemd-managed daemon reported healthy"
                        );
                    } else {
                        tracing::warn!(
                            service = SYSTEMD_USER_SERVICE,
                            timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                            "started systemd user service but daemon did not become healthy"
                        );
                    }
                    true
                }
                Ok(status) => {
                    tracing::warn!(
                        service = SYSTEMD_USER_SERVICE,
                        status = ?status.code(),
                        "failed to start systemd user service"
                    );
                    false
                }
                Err(error) => {
                    tracing::warn!(
                        service = SYSTEMD_USER_SERVICE,
                        %error,
                        "failed to run systemctl for systemd user service"
                    );
                    false
                }
            }
        }
        SystemdUserServicePlan::SpawnChild => false,
    }
}

#[cfg(target_os = "linux")]
fn detect_systemd_user_service() -> SystemdUserServiceProbe {
    if systemctl_user_output(&["is-active", SYSTEMD_USER_SERVICE])
        .as_deref()
        .is_ok_and(systemctl_is_active_output)
    {
        return SystemdUserServiceProbe::Active;
    }

    if systemctl_user_output(&["is-enabled", SYSTEMD_USER_SERVICE])
        .as_deref()
        .is_ok_and(systemctl_is_enabled_output)
    {
        SystemdUserServiceProbe::EnabledInactive
    } else {
        SystemdUserServiceProbe::Unavailable
    }
}

#[cfg(target_os = "linux")]
fn systemctl_user_output(args: &[&str]) -> std::io::Result<String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn start_systemd_user_service() -> std::io::Result<std::process::ExitStatus> {
    Command::new("systemctl")
        .args(["--user", "start", SYSTEMD_USER_SERVICE])
        .status()
}

fn push_daemon_candidates(candidates: &mut Vec<PathBuf>, directory: &Path) {
    push_unique_path(candidates, directory.join(daemon_executable_name()));

    for target_triple in target_triple_candidates() {
        push_unique_path(
            candidates,
            directory.join(tauri_sidecar_daemon_name(target_triple)),
        );
    }
}

fn push_resource_ui_candidates(candidates: &mut Vec<PathBuf>, resource_dir: &Path) {
    push_unique_path(candidates, resource_dir.join("ui"));
    push_share_ui_candidate(candidates, resource_dir);
}

fn push_share_ui_candidate(candidates: &mut Vec<PathBuf>, base_dir: &Path) {
    push_unique_path(
        candidates,
        base_dir.join("share").join("hypercolor").join("ui"),
    );
}

fn push_unique_path(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

/// Spawn a daemon process and bind it to the app lifetime.
///
/// # Errors
///
/// Returns an error when the child process cannot be spawned or platform
/// ownership cannot be attached.
pub fn spawn_daemon(command: &DaemonCommand) -> Result<ManagedDaemon> {
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(daemon_log_file()?.try_clone()?))
        .stderr(Stdio::from(daemon_log_file()?));

    configure_platform_command(&mut process);

    let mut child = process
        .spawn()
        .with_context(|| format!("failed to spawn {}", command.program.display()))?;

    let platform_guard = match attach_platform_guard(&child) {
        Ok(platform_guard) => platform_guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };

    Ok(ManagedDaemon {
        child,
        platform_guard,
    })
}

fn daemon_log_file() -> std::io::Result<std::fs::File> {
    let log_dir = data_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon-supervised.log"))
}

#[cfg(target_os = "windows")]
type PlatformGuard = win32job::Job;

#[cfg(target_os = "windows")]
fn configure_platform_command(_command: &mut Command) {}

#[cfg(target_os = "windows")]
fn attach_platform_guard(child: &Child) -> Result<PlatformGuard> {
    use std::os::windows::io::AsRawHandle;

    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limits)?;
    job.assign_process(child.as_raw_handle() as isize)?;
    Ok(job)
}

#[cfg(unix)]
#[derive(Debug)]
struct PlatformGuard;

#[cfg(unix)]
fn configure_platform_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the platform helper signature aligned with Windows"
)]
fn attach_platform_guard(_child: &Child) -> Result<PlatformGuard> {
    Ok(PlatformGuard)
}
