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
use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosExternalOwnerMode, MacosOwnerRemedy, MacosOwnerStore,
};
#[cfg(target_os = "macos")]
use hypercolor_macos_owner::{MacosOwnerExecutionError, MacosOwnerIncarnation};
use hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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

/// Watchdog circuit-breaker: max rapid daemon restarts within
/// [`WATCHDOG_FAILURE_WINDOW`] before the supervisor gives up.
pub const WATCHDOG_MAX_RAPID_RESTARTS: u32 = 5;

/// Rolling window for the watchdog circuit breaker.
pub const WATCHDOG_FAILURE_WINDOW: Duration = Duration::from_secs(300);

/// Minimum healthy uptime before a daemon restart counts toward the rapid-
/// failure window resetting. A daemon that runs for at least this long is
/// considered "stable enough" — its next exit starts a fresh window.
pub const WATCHDOG_STABLE_UPTIME: Duration = Duration::from_secs(60);

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
///
/// Tracks the PID of whichever daemon child the watchdog is currently
/// supervising. On macOS, the handover authority and watchdog share the
/// retained `Child` through `app_sidecar_child`.
#[derive(Clone, Default)]
pub struct SupervisorState {
    child_pid: Arc<Mutex<Option<u32>>>,
    #[cfg(target_os = "macos")]
    app_sidecar_child: Arc<Mutex<Option<AppSidecarChild>>>,
    /// Latched true when the watchdog circuit-breaker fires —
    /// `WATCHDOG_MAX_RAPID_RESTARTS` failures within `WATCHDOG_FAILURE_WINDOW`.
    /// The tray reads this to surface the red `IconState::Error` so users
    /// know the supervisor has given up trying to restart the daemon.
    permanent_failure: Arc<std::sync::atomic::AtomicBool>,
    owner_handover_stop: Arc<std::sync::atomic::AtomicBool>,
    macos_external_owner: Arc<Mutex<Option<MacosExternalOwnerMode>>>,
    macos_owner_offline: Arc<Mutex<Option<MacosDaemonOwnerOfflineStatus>>>,
}

/// App-local status when a persisted external owner is not reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MacosDaemonOwnerOfflineStatus {
    pub code: &'static str,
    pub selected_owner: MacosDaemonOwner,
    pub remedy: MacosOwnerRemedy,
}

impl SupervisorState {
    /// Return the app-owned daemon process ID, if one is running.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        *self.child_guard()
    }

    /// Returns true when the watchdog has hit the rapid-failure cap and
    /// stopped trying to respawn. Tray surfaces this as an Error icon.
    #[must_use]
    pub fn permanent_failure(&self) -> bool {
        self.permanent_failure
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Return the persisted external owner selected before watchdog startup.
    #[must_use]
    pub fn macos_external_owner(&self) -> Option<MacosExternalOwnerMode> {
        *self
            .macos_external_owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Return the topology-specific offline status for the app bridge.
    #[must_use]
    pub fn macos_owner_offline(&self) -> Option<MacosDaemonOwnerOfflineStatus> {
        *self
            .macos_owner_offline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_owner_handover_stop(&self, stopping: bool) {
        self.owner_handover_stop
            .store(stopping, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn owner_handover_stop(&self) -> bool {
        self.owner_handover_stop
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn set_macos_external_owner(&self, owner: Option<MacosExternalOwnerMode>) {
        *self
            .macos_external_owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = owner;
    }

    pub(crate) fn set_macos_owner_offline(&self, status: Option<MacosDaemonOwnerOfflineStatus>) {
        *self
            .macos_owner_offline
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = status;
    }

    pub(crate) fn clear_macos_owner_offline_if(
        &self,
        status: MacosDaemonOwnerOfflineStatus,
    ) -> bool {
        let mut current = self
            .macos_owner_offline
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != Some(status) {
            return false;
        }
        *current = None;
        true
    }

    fn replace_child_pid(&self, pid: u32) {
        *self.child_guard() = Some(pid);
    }

    fn clear_child(&self) {
        *self.child_guard() = None;
        #[cfg(target_os = "macos")]
        {
            *self
                .app_sidecar_child
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    #[cfg(target_os = "macos")]
    fn register_app_sidecar_child(
        &self,
        incarnation: MacosOwnerIncarnation,
        daemon: SharedManagedDaemon,
    ) -> Result<(), MacosOwnerExecutionError> {
        let child_pid = daemon.lock().unwrap_or_else(PoisonError::into_inner).id();
        if incarnation.owner != MacosDaemonOwner::AppSidecar
            || incarnation.identity.pid != child_pid
        {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar owner identity does not match the retained child",
            ));
        }
        *self
            .app_sidecar_child
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(AppSidecarChild {
            incarnation,
            daemon,
        });
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn preflight_app_sidecar_stop(
        &self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        let authority = self
            .app_sidecar_child
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(authority) = authority.as_ref() else {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar termination requires a retained child handle",
            ));
        };
        if authority.incarnation != *incarnation {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar owner identity does not match the retained child",
            ));
        }
        let mut daemon = authority
            .daemon
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(child) = daemon.child.as_mut() else {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar termination requires an unreaped child handle",
            ));
        };
        if child
            .try_wait()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .is_some()
        {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar termination requires a live unreaped child handle",
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn stop_app_sidecar(
        &self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        let authority = self
            .app_sidecar_child
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(authority) = authority.as_ref() else {
            return Ok(());
        };
        if authority.incarnation != *incarnation {
            return Err(MacosOwnerExecutionError::new(
                "app-sidecar owner identity does not match the retained child",
            ));
        }
        let mut daemon = authority
            .daemon
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(child) = daemon.child.as_mut() else {
            return Ok(());
        };
        hypercolor_macos_owner::request_macos_child_termination(child)
    }

    fn mark_permanent_failure(&self) {
        self.permanent_failure
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn child_guard(&self) -> MutexGuard<'_, Option<u32>> {
        self.child_pid
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

type SharedManagedDaemon = Arc<Mutex<ManagedDaemon>>;

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct AppSidecarChild {
    incarnation: MacosOwnerIncarnation,
    daemon: SharedManagedDaemon,
}

/// App-owned daemon child process.
pub struct ManagedDaemon {
    /// Child handle retained until the watchdog reaps it. On macOS, the
    /// handover authority shares this same managed daemon.
    pub(crate) child: Option<Child>,
    #[allow(dead_code)]
    pub(crate) platform_guard: PlatformGuard,
}

impl ManagedDaemon {
    /// Return the child process ID, or 0 after the watchdog reaps it.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.as_ref().map_or(0, Child::id)
    }
}

impl Drop for ManagedDaemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && matches!(child.try_wait(), Ok(None))
        {
            let _ = child.kill();
            let _ = child.wait();
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

/// Resolve likely bundled effect directories for supported package layouts.
///
/// Mirrors [`ui_dir_candidates`]: the catalog is read where it was installed
/// rather than copied into the user's data directory, so a stale copy cannot
/// shadow the version that actually shipped.
#[must_use]
pub fn effects_dir_candidates(current_exe: &Path, resource_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(install_dir) = current_exe.parent() {
        push_unique_path(&mut candidates, install_dir.join("effects").join("bundled"));

        if let Some(prefix_dir) = install_dir.parent() {
            push_unique_path(
                &mut candidates,
                prefix_dir
                    .join("share")
                    .join("hypercolor")
                    .join("effects")
                    .join("bundled"),
            );
        }
    }

    if let Some(resource_dir) = resource_dir {
        push_unique_path(
            &mut candidates,
            resource_dir.join("effects").join("bundled"),
        );
    }

    if let Some(resource_dir) = macos_app_resource_dir(current_exe) {
        push_unique_path(
            &mut candidates,
            resource_dir.join("effects").join("bundled"),
        );
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
    effects_dir: Option<&Path>,
) -> DaemonCommand {
    let mut args = vec!["--bind".to_owned(), bind.to_owned()];

    #[cfg(target_os = "macos")]
    args.extend(["--macos-owner".to_owned(), "app-sidecar".to_owned()]);

    if let Some(ui_dir) = ui_dir {
        args.push("--ui-dir".to_owned());
        args.push(ui_dir.display().to_string());
    }

    if let Some(effects_dir) = effects_dir {
        args.push("--effects-dir".to_owned());
        args.push(effects_dir.display().to_string());
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

#[cfg(target_os = "macos")]
fn system_status_url(base: &Url) -> Url {
    base.join("/api/v1/status")
        .expect("static system-status endpoint path should be valid")
}

#[cfg(target_os = "macos")]
#[derive(Debug, serde::Deserialize)]
struct SystemStatusEnvelope {
    data: SystemStatusData,
}

#[cfg(target_os = "macos")]
#[derive(Debug, serde::Deserialize)]
struct SystemStatusData {
    macos_daemon_ownership: Option<SystemStatusMacosOwnership>,
}

#[cfg(target_os = "macos")]
#[derive(Debug, serde::Deserialize)]
struct SystemStatusMacosOwnership {
    active_owner: SystemStatusMacosOwner,
    owner_epoch: u64,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SystemStatusMacosOwner {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    Standalone,
}

#[cfg(target_os = "macos")]
const fn system_status_owner_matches(
    selected_owner: MacosDaemonOwner,
    observed_owner: SystemStatusMacosOwner,
) -> bool {
    matches!(
        (selected_owner, observed_owner),
        (
            MacosDaemonOwner::AppSidecar,
            SystemStatusMacosOwner::AppSidecar
        ) | (
            MacosDaemonOwner::DirectLaunchd,
            SystemStatusMacosOwner::LaunchdService
        ) | (
            MacosDaemonOwner::Homebrew,
            SystemStatusMacosOwner::HomebrewService
        ) | (
            MacosDaemonOwner::Standalone,
            SystemStatusMacosOwner::Standalone
        )
    )
}

#[cfg(target_os = "macos")]
const fn authoritative_owner_matches(
    selected_owner: MacosDaemonOwner,
    after_epoch: Option<u64>,
    ownership: &SystemStatusMacosOwnership,
) -> bool {
    system_status_owner_matches(selected_owner, ownership.active_owner)
        && match after_epoch {
            Some(epoch) => ownership.owner_epoch > epoch,
            None => true,
        }
}

#[cfg(target_os = "macos")]
async fn probe_authoritative_macos_owner(
    client: &reqwest::Client,
    base: &Url,
    selected_owner: MacosDaemonOwner,
    after_epoch: Option<u64>,
) -> bool {
    if !probe_health(client, base, HEALTH_PROBE_TIMEOUT).await {
        return false;
    }
    let response = client
        .get(system_status_url(base))
        .timeout(HEALTH_PROBE_TIMEOUT)
        .send()
        .await;
    let Ok(response) = response else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<SystemStatusEnvelope>()
        .await
        .ok()
        .and_then(|envelope| envelope.data.macos_daemon_ownership)
        .is_some_and(|ownership| {
            authoritative_owner_matches(selected_owner, after_epoch, &ownership)
        })
}

#[cfg(target_os = "macos")]
pub(crate) async fn wait_for_authoritative_macos_owner(
    client: &reqwest::Client,
    base: &Url,
    selected_owner: MacosDaemonOwner,
    after_epoch: Option<u64>,
    timeout: Duration,
) -> bool {
    let started = Instant::now();
    loop {
        if probe_authoritative_macos_owner(client, base, selected_owner, after_epoch).await {
            return true;
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return false;
        };
        let Some(delay) = startup_retry_delay(remaining, DAEMON_STARTUP_POLL_INTERVAL) else {
            return false;
        };
        tokio::time::sleep(delay).await;
    }
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
    #[cfg(target_os = "macos")]
    {
        let store = MacosOwnerStore::new(data_dir());
        let state = app.state::<SupervisorState>().inner().clone();
        match crate::ownership::recover_daemon_owner_before_supervisor(
            app,
            state,
            daemon_url.clone(),
            store.clone(),
        )? {
            crate::ownership::MacosStartupRecoveryDisposition::Continue => {}
            crate::ownership::MacosStartupRecoveryDisposition::SupervisorStarted
            | crate::ownership::MacosStartupRecoveryDisposition::SuppressSupervisor => {
                return Ok(());
            }
        }
        let external_owner = selected_external_owner_for_startup(&store)?;
        start_with_external_owner(app, daemon_url, external_owner)
    }
    #[cfg(not(target_os = "macos"))]
    {
        start_with_external_owner(app, daemon_url, None)
    }
}

#[cfg(target_os = "macos")]
fn selected_external_owner_for_startup(
    store: &MacosOwnerStore,
) -> Result<Option<MacosExternalOwnerMode>> {
    Ok(store
        .load_owner_record()
        .context("failed to read the selected macOS daemon owner before supervisor startup")?
        .and_then(|record| record.selected_external_owner))
}

#[cfg(target_os = "macos")]
pub(crate) fn start_app_sidecar_for_handover<R: Runtime>(
    app: &AppHandle<R>,
    daemon_url: Url,
) -> Result<()> {
    start_with_external_owner(app, daemon_url, None)
}

fn start_with_external_owner<R: Runtime>(
    app: &AppHandle<R>,
    daemon_url: Url,
    external_owner: Option<MacosExternalOwnerMode>,
) -> Result<()> {
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
    let effects_dir = effects_dir_candidates(&current_exe, resource_dir.as_deref())
        .into_iter()
        .find(|path| path.is_dir());
    let bind = bind_from_daemon_url(&daemon_url).unwrap_or_else(|| DEFAULT_DAEMON_BIND.to_owned());
    let state = app.state::<SupervisorState>().inner().clone();
    state.set_macos_external_owner(external_owner);
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let authoritative_owner_online = if let Some(owner) = external_owner {
            probe_authoritative_macos_owner(&client, &daemon_url, external_mode_owner(owner), None)
                .await
        } else {
            probe_health(&client, &daemon_url, HEALTH_PROBE_TIMEOUT).await
        };
        if authoritative_owner_online {
            state.set_macos_owner_offline(None);
            tracing::info!(url = %daemon_url, "daemon already running; reusing existing instance");
            return;
        }

        if let Some(owner) = external_owner {
            let status = macos_external_owner_offline(owner);
            state.set_macos_owner_offline(Some(status));
            if let Err(error) = app.emit("macos_daemon_owner_offline", status) {
                tracing::warn!(%error, "failed to publish app-local daemon owner status");
            }
            tracing::warn!(
                selected_owner = ?status.selected_owner,
                remedy = ?status.remedy,
                "persisted external macOS daemon owner is offline; sidecar remains suppressed"
            );
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

        run_watchdog_loop(
            client,
            daemon_url,
            daemon_path,
            bind,
            ui_dir,
            effects_dir,
            state,
        )
        .await;
    });

    Ok(())
}

/// Exponential backoff for daemon restart attempts: 1s, 2s, 5s, 10s, 30s.
#[must_use]
pub const fn restart_backoff(attempt: u32) -> Duration {
    let secs = match attempt {
        0 | 1 => 1,
        2 => 2,
        3 => 5,
        4 => 10,
        _ => 30,
    };
    Duration::from_secs(secs)
}

#[must_use]
pub fn is_terminal_daemon_exit_code(code: Option<i32>) -> bool {
    cfg!(target_os = "macos") && code == Some(MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE)
}

enum DaemonStartupOutcome {
    Healthy,
    Exited(std::process::ExitStatus),
    TimedOut,
}

#[derive(Debug, PartialEq, Eq)]
enum DaemonStartupObservation<T> {
    Healthy,
    Exited(T),
}

fn select_daemon_startup_observation<T>(
    child_exit: Option<T>,
    healthy: bool,
) -> Option<DaemonStartupObservation<T>> {
    child_exit.map_or_else(
        || healthy.then_some(DaemonStartupObservation::Healthy),
        |status| Some(DaemonStartupObservation::Exited(status)),
    )
}

fn poll_daemon_exit(daemon: &mut ManagedDaemon) -> Result<Option<std::process::ExitStatus>> {
    daemon
        .child
        .as_mut()
        .context("daemon child is unavailable during startup")?
        .try_wait()
        .context("failed to poll daemon startup")
}

async fn wait_for_daemon_startup(
    client: &reqwest::Client,
    base: &Url,
    daemon: &mut ManagedDaemon,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<DaemonStartupOutcome> {
    let started = Instant::now();
    loop {
        if let Some(DaemonStartupObservation::Exited(status)) =
            select_daemon_startup_observation(poll_daemon_exit(daemon)?, false)
        {
            return Ok(DaemonStartupOutcome::Exited(status));
        }
        let healthy = probe_health(client, base, HEALTH_PROBE_TIMEOUT).await;
        match select_daemon_startup_observation(poll_daemon_exit(daemon)?, healthy) {
            Some(DaemonStartupObservation::Exited(status)) => {
                return Ok(DaemonStartupOutcome::Exited(status));
            }
            Some(DaemonStartupObservation::Healthy) => {
                return Ok(DaemonStartupOutcome::Healthy);
            }
            None => {}
        }
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(DaemonStartupOutcome::TimedOut);
        };
        tokio::time::sleep(poll_interval.min(remaining)).await;
    }
}

/// Watchdog loop: keeps the daemon alive across crashes, with a
/// circuit breaker that gives up after [`WATCHDOG_MAX_RAPID_RESTARTS`]
/// restarts in [`WATCHDOG_FAILURE_WINDOW`].
///
/// Per-attempt flow:
/// 1. Spawn the daemon as a child process.
/// 2. Wait up to [`DAEMON_STARTUP_TIMEOUT`] for `/health` to respond.
/// 3. If healthy, block on the child until it exits.
/// 4. On exit, log the status and decide whether to keep restarting
///    (under the rapid-restart cap) or give up (cap exhausted).
///
/// "Rapid" here is gated by [`WATCHDOG_STABLE_UPTIME`]: a daemon that
/// stays healthy for at least that long resets the counter on its
/// eventual exit. This lets the daemon naturally cycle (e.g. updater
/// swap) without exhausting the budget.
async fn run_watchdog_loop(
    client: reqwest::Client,
    daemon_url: Url,
    daemon_path: PathBuf,
    bind: String,
    ui_dir: Option<PathBuf>,
    effects_dir: Option<PathBuf>,
    state: SupervisorState,
) {
    let mut restart_count: u32 = 0;
    let mut window_anchor: Option<Instant> = None;

    loop {
        if state.owner_handover_stop() {
            state.clear_child();
            tracing::info!("daemon watchdog suppressed for owner handover");
            return;
        }
        if let Some(anchor) = window_anchor
            && anchor.elapsed() > WATCHDOG_FAILURE_WINDOW
        {
            restart_count = 0;
            window_anchor = None;
        }

        if restart_count >= WATCHDOG_MAX_RAPID_RESTARTS {
            tracing::error!(
                restarts = restart_count,
                window_secs = WATCHDOG_FAILURE_WINDOW.as_secs(),
                "daemon failed to stay alive too many times in window; supervisor giving up"
            );
            state.clear_child();
            state.mark_permanent_failure();
            return;
        }

        let command = build_daemon_command(
            &daemon_path,
            &bind,
            ui_dir.as_deref(),
            effects_dir.as_deref(),
        );
        let mut daemon = match spawn_daemon(&command) {
            Ok(daemon) => daemon,
            Err(error) => {
                tracing::warn!(%error, attempt = restart_count + 1, "failed to spawn daemon");
                record_failure(&mut restart_count, &mut window_anchor);
                tokio::time::sleep(restart_backoff(restart_count)).await;
                continue;
            }
        };
        let pid = daemon.id();
        state.replace_child_pid(pid);
        tracing::info!(
            pid,
            attempt = restart_count + 1,
            "supervisor: daemon spawned"
        );

        let startup = wait_for_daemon_startup(
            &client,
            &daemon_url,
            &mut daemon,
            DAEMON_STARTUP_TIMEOUT,
            DAEMON_STARTUP_POLL_INTERVAL,
        )
        .await;
        let retry = match startup {
            Ok(DaemonStartupOutcome::Healthy) => false,
            Ok(DaemonStartupOutcome::Exited(status))
                if is_terminal_daemon_exit_code(status.code()) =>
            {
                tracing::info!(
                    pid,
                    ?status,
                    "daemon ownership contender exited; supervisor will not restart it"
                );
                state.clear_child();
                return;
            }
            Ok(DaemonStartupOutcome::Exited(status)) => {
                tracing::warn!(
                    pid,
                    ?status,
                    "daemon exited before becoming healthy; supervisor will restart"
                );
                true
            }
            Ok(DaemonStartupOutcome::TimedOut) => {
                tracing::warn!(
                    pid,
                    timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                    "daemon did not become healthy before timeout; killing and retrying"
                );
                true
            }
            Err(error) => {
                tracing::warn!(
                    pid,
                    %error,
                    "daemon startup observation failed; supervisor will restart"
                );
                true
            }
        };
        if retry {
            drop(daemon);
            state.clear_child();
            if state.owner_handover_stop() {
                tracing::info!(pid, "daemon stopped for owner handover during startup");
                return;
            }
            record_failure(&mut restart_count, &mut window_anchor);
            tokio::time::sleep(restart_backoff(restart_count)).await;
            continue;
        }

        tracing::info!(pid, "supervisor: daemon healthy");
        let spawned_at = Instant::now();
        let daemon = Arc::new(Mutex::new(daemon));
        #[cfg(target_os = "macos")]
        if let Err(error) = bind_app_sidecar_child(&state, pid, Arc::clone(&daemon)) {
            tracing::error!(pid, %error, "healthy app sidecar did not publish its exact owner identity");
            drop(daemon);
            state.clear_child();
            record_failure(&mut restart_count, &mut window_anchor);
            tokio::time::sleep(restart_backoff(restart_count)).await;
            continue;
        }
        let exit = wait_for_exit(daemon).await;
        let uptime = spawned_at.elapsed();
        state.clear_child();

        match exit {
            Ok(status) if is_terminal_daemon_exit_code(status.code()) => {
                tracing::info!(
                    pid,
                    ?status,
                    "daemon ownership contender exited after health observation; supervisor will not restart it"
                );
                return;
            }
            Ok(status) => tracing::warn!(
                pid,
                ?status,
                uptime_secs = uptime.as_secs(),
                "daemon exited; supervisor will restart"
            ),
            Err(error) => tracing::error!(
                pid,
                %error,
                uptime_secs = uptime.as_secs(),
                "daemon wait failed; supervisor will restart"
            ),
        }

        if state.owner_handover_stop() {
            tracing::info!(
                pid,
                "daemon stopped for owner handover; watchdog remains suppressed"
            );
            return;
        }

        if uptime >= WATCHDOG_STABLE_UPTIME {
            // Stable run — reset the budget so the next failure starts fresh.
            restart_count = 0;
            window_anchor = None;
        } else {
            record_failure(&mut restart_count, &mut window_anchor);
        }
        tokio::time::sleep(restart_backoff(restart_count)).await;
    }
}

#[cfg(target_os = "macos")]
fn bind_app_sidecar_child(
    state: &SupervisorState,
    pid: u32,
    daemon: SharedManagedDaemon,
) -> Result<(), MacosOwnerExecutionError> {
    let store = MacosOwnerStore::new(data_dir());
    let record = store
        .load_owner_record()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        .filter(|record| {
            record.active_owner == MacosDaemonOwner::AppSidecar && record.active_identity.pid == pid
        })
        .ok_or_else(|| {
            MacosOwnerExecutionError::new(
                "healthy app sidecar has no matching authoritative owner publication",
            )
        })?;
    state.register_app_sidecar_child(record.incarnation(), daemon)
}

const fn macos_external_owner_offline(
    owner: MacosExternalOwnerMode,
) -> MacosDaemonOwnerOfflineStatus {
    match owner {
        MacosExternalOwnerMode::DirectLaunchd => MacosDaemonOwnerOfflineStatus {
            code: "macos_daemon_owner_offline",
            selected_owner: MacosDaemonOwner::DirectLaunchd,
            remedy: MacosOwnerRemedy::StartLaunchdService,
        },
        MacosExternalOwnerMode::Homebrew => MacosDaemonOwnerOfflineStatus {
            code: "macos_daemon_owner_offline",
            selected_owner: MacosDaemonOwner::Homebrew,
            remedy: MacosOwnerRemedy::StartHomebrewService,
        },
    }
}

#[cfg(target_os = "macos")]
const fn external_mode_owner(owner: MacosExternalOwnerMode) -> MacosDaemonOwner {
    match owner {
        MacosExternalOwnerMode::DirectLaunchd => MacosDaemonOwner::DirectLaunchd,
        MacosExternalOwnerMode::Homebrew => MacosDaemonOwner::Homebrew,
    }
}

fn record_failure(count: &mut u32, anchor: &mut Option<Instant>) {
    *count = count.saturating_add(1);
    if anchor.is_none() {
        *anchor = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonStartupObservation, MacosDaemonOwnerOfflineStatus, macos_external_owner_offline,
        select_daemon_startup_observation,
    };
    use hypercolor_macos_owner::{MacosDaemonOwner, MacosExternalOwnerMode, MacosOwnerRemedy};

    #[cfg(target_os = "macos")]
    async fn authoritative_probe_fixture(
        status_body: &'static str,
        selected_owner: MacosDaemonOwner,
        after_epoch: Option<u64>,
    ) -> bool {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener should bind");
        let address = listener
            .local_addr()
            .expect("fixture address should resolve");
        let server = tokio::spawn(async move {
            for (expected_path, body) in [("/health", "{}"), ("/api/v1/status", status_body)] {
                let (mut stream, _) = listener.accept().await.expect("request should connect");
                let mut request = [0_u8; 4_096];
                let read = stream
                    .read(&mut request)
                    .await
                    .expect("request should read");
                let request = std::str::from_utf8(&request[..read])
                    .expect("request should contain UTF-8 headers");
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1")));
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response should write");
            }
        });
        let base =
            url::Url::parse(&format!("http://{address}")).expect("fixture daemon URL should parse");
        let result = super::probe_authoritative_macos_owner(
            &reqwest::Client::new(),
            &base,
            selected_owner,
            after_epoch,
        )
        .await;
        server.await.expect("fixture server should finish");
        result
    }

    #[test]
    fn child_exit_precedes_shared_daemon_health() {
        assert_eq!(
            select_daemon_startup_observation(Some(73), true),
            Some(DaemonStartupObservation::Exited(73))
        );
        assert_eq!(
            select_daemon_startup_observation::<i32>(None, true),
            Some(DaemonStartupObservation::Healthy)
        );
    }

    #[test]
    fn external_owner_offline_status_uses_stable_topology_remedies() {
        assert_eq!(
            macos_external_owner_offline(MacosExternalOwnerMode::DirectLaunchd),
            MacosDaemonOwnerOfflineStatus {
                code: "macos_daemon_owner_offline",
                selected_owner: MacosDaemonOwner::DirectLaunchd,
                remedy: MacosOwnerRemedy::StartLaunchdService,
            }
        );
        assert_eq!(
            macos_external_owner_offline(MacosExternalOwnerMode::Homebrew),
            MacosDaemonOwnerOfflineStatus {
                code: "macos_daemon_owner_offline",
                selected_owner: MacosDaemonOwner::Homebrew,
                remedy: MacosOwnerRemedy::StartHomebrewService,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn authoritative_system_status_requires_exact_owner_and_newer_epoch() {
        use super::{SystemStatusEnvelope, authoritative_owner_matches, system_status_url};

        let base = url::Url::parse("http://127.0.0.1:9420").expect("URL should parse");
        assert_eq!(
            system_status_url(&base).as_str(),
            "http://127.0.0.1:9420/api/v1/status"
        );

        let launchd: SystemStatusEnvelope = serde_json::from_value(serde_json::json!({
            "data": {
                "macos_daemon_ownership": {
                    "active_owner": "launchd_service",
                    "owner_epoch": 8
                }
            }
        }))
        .expect("launchd status should decode");
        let launchd = launchd
            .data
            .macos_daemon_ownership
            .expect("ownership should be present");
        assert!(authoritative_owner_matches(
            MacosDaemonOwner::DirectLaunchd,
            Some(7),
            &launchd
        ));
        assert!(!authoritative_owner_matches(
            MacosDaemonOwner::DirectLaunchd,
            Some(8),
            &launchd
        ));
        assert!(!authoritative_owner_matches(
            MacosDaemonOwner::Homebrew,
            None,
            &launchd
        ));

        let missing: SystemStatusEnvelope = serde_json::from_value(serde_json::json!({
            "data": { "macos_daemon_ownership": null }
        }))
        .expect("missing ownership status should decode");
        assert!(missing.data.macos_daemon_ownership.is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn generic_health_does_not_satisfy_authoritative_owner_probe() {
        let launchd = r#"{"data":{"macos_daemon_ownership":{"active_owner":"launchd_service","owner_epoch":8}}}"#;
        let homebrew = r#"{"data":{"macos_daemon_ownership":{"active_owner":"homebrew_service","owner_epoch":9}}}"#;

        assert!(
            authoritative_probe_fixture(launchd, MacosDaemonOwner::DirectLaunchd, Some(7)).await
        );
        assert!(
            !authoritative_probe_fixture(launchd, MacosDaemonOwner::DirectLaunchd, Some(8)).await
        );
        assert!(
            !authoritative_probe_fixture(homebrew, MacosDaemonOwner::DirectLaunchd, None).await
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_reads_the_persisted_external_owner_mode() {
        use hypercolor_macos_owner::{MacosOwnerIdentity, MacosOwnerStore};

        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        store
            .publish_owner(
                MacosDaemonOwner::Homebrew,
                MacosOwnerIdentity::new(
                    "audit-homebrew",
                    "/opt/homebrew/bin/hypercolor-daemon",
                    "requirement-homebrew",
                    101,
                )
                .expect("identity should build"),
            )
            .expect("owner should publish");
        store
            .set_external_owner_mode(Some(MacosExternalOwnerMode::Homebrew))
            .expect("external mode should persist");
        assert_eq!(
            super::selected_external_owner_for_startup(&store)
                .expect("startup selection should load"),
            Some(MacosExternalOwnerMode::Homebrew)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_or_mismatched_sidecar_identity_never_stops_the_retained_child() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex, PoisonError};

        use hypercolor_macos_owner::{MacosOwnerIdentity, MacosOwnerIncarnation};

        use super::{ManagedDaemon, PlatformGuard, SupervisorState};

        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fixture child should spawn");
        let pid = child.id();
        let daemon = Arc::new(Mutex::new(ManagedDaemon {
            child: Some(child),
            platform_guard: PlatformGuard,
        }));
        let exact = MacosOwnerIncarnation {
            owner: MacosDaemonOwner::AppSidecar,
            owner_epoch: 9,
            identity: MacosOwnerIdentity::new(
                "audit-current",
                "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                "requirement-current",
                pid,
            )
            .expect("exact identity should build"),
        };
        let state = SupervisorState::default();
        state
            .register_app_sidecar_child(exact.clone(), Arc::clone(&daemon))
            .expect("exact child should register");

        let stale_pid_reuse = MacosOwnerIncarnation {
            owner: MacosDaemonOwner::AppSidecar,
            owner_epoch: 8,
            identity: MacosOwnerIdentity::new(
                "audit-stale",
                "/Applications/Old Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                "requirement-stale",
                pid,
            )
            .expect("stale identity should build"),
        };
        assert!(state.preflight_app_sidecar_stop(&stale_pid_reuse).is_err());
        assert!(state.stop_app_sidecar(&stale_pid_reuse).is_err());
        assert!(
            daemon
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .child
                .as_mut()
                .expect("child should remain retained")
                .try_wait()
                .expect("child state should read")
                .is_none()
        );

        state
            .preflight_app_sidecar_stop(&exact)
            .expect("exact live retained child should pass preflight");
        state
            .stop_app_sidecar(&exact)
            .expect("exact retained child should stop");
        {
            let mut daemon = daemon.lock().unwrap_or_else(PoisonError::into_inner);
            let status = daemon
                .child
                .as_mut()
                .expect("child should remain retained until reap")
                .wait()
                .expect("stopped child should reap");
            assert_eq!(status.signal(), Some(15), "handover stop must use SIGTERM");
            daemon.child.take();
        }
        state.clear_child();
        assert!(state.preflight_app_sidecar_stop(&exact).is_err());
        state
            .stop_app_sidecar(&exact)
            .expect("replayed exact stop should be idempotent after the child is cleared");
    }
}

/// Poll a retained `ManagedDaemon` child on a dedicated thread until it exits.
async fn wait_for_exit(daemon: SharedManagedDaemon) -> Result<std::process::ExitStatus> {
    let join = tauri::async_runtime::spawn_blocking(
        move || -> std::io::Result<std::process::ExitStatus> {
            loop {
                let status = {
                    let mut daemon = daemon.lock().unwrap_or_else(PoisonError::into_inner);
                    let child = daemon
                        .child
                        .as_mut()
                        .ok_or_else(|| std::io::Error::other("daemon child already taken"))?;
                    let status = child.try_wait()?;
                    if status.is_some() {
                        daemon.child.take();
                    }
                    status
                };
                if let Some(status) = status {
                    return Ok(status);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        },
    );
    match join.await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(error)) => Err(anyhow::Error::from(error)),
        Err(error) => Err(anyhow::Error::from(error)),
    }
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
        child: Some(child),
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
pub(crate) type PlatformGuard = win32job::Job;

#[cfg(target_os = "windows")]
fn configure_platform_command(command: &mut Command) {
    crate::process_ext::hide_console_window(command);
}

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
pub(crate) struct PlatformGuard;

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
