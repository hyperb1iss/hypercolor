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
    MacosDaemonOwner, MacosExternalOwnerMode, MacosOwnerRemedy, MacosProtectedControlCredential,
    MacosServerSessionId,
};
#[cfg(target_os = "macos")]
use hypercolor_macos_owner::{
    MacosDaemonSessionAttestation, MacosOwnerExecutionError, MacosOwnerIncarnation,
    MacosOwnerStore, try_acquire_macos_daemon_guard,
};
#[cfg(target_os = "macos")]
use hypercolor_types::api::ApiResponse;
#[cfg(target_os = "macos")]
use hypercolor_types::api::system::{
    MacosCapabilityOwner, MacosDaemonOwnershipStatus, SystemResource,
};
use hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE;
use hypercolor_types::service::{SERVICE_IDENTITY_ENV, SUPERVISED_PARENT_PID_ENV, ServiceIdentity};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use url::Url;

mod plan;

pub use plan::{HoldReason, LauncherPlan, LauncherProbe, OwnerPreference, launcher_plan};

/// Default daemon bind address used by the app-spawned daemon.
pub const DEFAULT_DAEMON_BIND: &str = "127.0.0.1:9420";

const DAEMON_EXECUTABLE_STEM: &str = "hypercolor-daemon";

pub const VERIFIED_DAEMON_CONNECTION_CHANGED_EVENT: &str = "verified-daemon-connection-changed";

/// Process-memory connection proof exposed only to the bundled app UI.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedDaemonConnection {
    pub base_url: String,
    pub server_session_id: Option<MacosServerSessionId>,
    pub protected_control_credential: Option<MacosProtectedControlCredential>,
}

/// Monotonic supervisor snapshot used to reject invoke/event reordering.
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedDaemonConnectionSnapshot {
    pub revision: u64,
    pub connection: Option<VerifiedDaemonConnection>,
}

type VerifiedConnectionEmitter =
    Arc<dyn Fn(VerifiedDaemonConnectionSnapshot) + Send + Sync + 'static>;

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
    /// Explicit launcher metadata inherited by the daemon process.
    pub environment: Vec<(String, String)>,
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

impl From<SystemdUserServiceProbe> for LauncherProbe {
    fn from(probe: SystemdUserServiceProbe) -> Self {
        match probe {
            SystemdUserServiceProbe::Active => Self::online(ServiceIdentity::systemd_user()),
            SystemdUserServiceProbe::EnabledInactive => {
                Self::startable(ServiceIdentity::systemd_user())
            }
            SystemdUserServiceProbe::Unavailable => Self::NOTHING,
        }
    }
}

impl LauncherProbe {
    /// Fold a registered service-manager launcher status (the Windows SCM
    /// adapter today) into a probe: running reuses, stopped-but-installed
    /// starts, unregistered falls through to the endpoint health probe.
    #[must_use]
    pub fn from_launcher_status(status: &crate::support::DaemonLauncherStatus) -> Option<Self> {
        let identity = status.identity.clone()?;
        Some(if status.online {
            Self::online(identity)
        } else {
            Self::startable(identity)
        })
    }
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
    verified_connection: Arc<Mutex<VerifiedDaemonConnectionSnapshot>>,
    verified_connection_emitter: Arc<Mutex<Option<VerifiedConnectionEmitter>>>,
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

    /// Return the current exact daemon-session proof, if one remains valid.
    #[must_use]
    pub fn verified_daemon_connection(&self) -> VerifiedDaemonConnectionSnapshot {
        self.verified_connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn install_verified_connection_emitter(&self, emitter: VerifiedConnectionEmitter) {
        *self
            .verified_connection_emitter
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(emitter);
    }

    fn replace_verified_connection(&self, connection: Option<VerifiedDaemonConnection>) {
        let snapshot = {
            let mut current = self
                .verified_connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if current.connection == connection {
                return;
            }
            current.revision = current.revision.saturating_add(1);
            current.connection = connection;
            current.clone()
        };
        if let Some(emitter) = self
            .verified_connection_emitter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            emitter(snapshot);
        }
    }

    fn clear_verified_connection(&self) {
        self.replace_verified_connection(None);
    }

    pub(crate) fn set_owner_handover_stop(&self, stopping: bool) {
        if stopping {
            self.clear_verified_connection();
        }
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

    #[cfg(target_os = "macos")]
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
        self.clear_verified_connection();
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
    fn app_sidecar_is_live(&self, incarnation: &MacosOwnerIncarnation) -> bool {
        self.preflight_app_sidecar_stop(incarnation).is_ok()
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

    /// Reap the managed daemon child on app exit.
    ///
    /// Tray quit runs `app.exit(0)`, which terminates the process without
    /// unwinding, so `ManagedDaemon::Drop` never fires on its own. This is
    /// called from the `RunEvent::Exit` handler while the process is still
    /// alive: it suppresses the watchdog, requests graceful termination,
    /// and escalates to a hard kill if the daemon lingers. The daemon's
    /// own parent-death watch is the backstop for exit paths that skip
    /// even this (crash, SIGKILL).
    pub fn terminate_managed_daemon_for_exit(&self) {
        self.set_owner_handover_stop(true);
        #[cfg(target_os = "macos")]
        {
            let authority = self
                .app_sidecar_child
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let Some(authority) = authority else {
                return;
            };
            let mut daemon = authority
                .daemon
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(child) = daemon.child.as_mut() else {
                return;
            };
            let pid = child.id();
            if let Err(error) = hypercolor_macos_owner::request_macos_child_termination(child) {
                tracing::warn!(pid, %error, "graceful daemon termination failed on app exit");
            }
            // The daemon's own shutdown budget is ~8s worst case (3s API
            // drain, device teardown, 5s persistence flush); killing at 2s
            // would routinely lose LED blanking and shutdown persistence.
            // Ten seconds matches the managed-handover allotment.
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::info!(pid, ?status, "managed daemon reaped on app exit");
                        daemon.child = None;
                        return;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(pid, %error, "managed daemon wait failed on app exit");
                        break;
                    }
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
            daemon.child = None;
            tracing::info!(pid, "managed daemon force-killed on app exit");
        }
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

/// Read the connection proof currently held by the native supervisor.
#[tauri::command]
#[must_use]
pub fn get_verified_daemon_connection(
    state: tauri::State<'_, SupervisorState>,
) -> VerifiedDaemonConnectionSnapshot {
    state.verified_daemon_connection()
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
        // Kill unless the child has provably exited: a try_wait error
        // (EINTR, ECHILD) must not leak a live process.
        if let Some(mut child) = self.child.take()
            && !matches!(child.try_wait(), Ok(Some(_)))
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
    let environment = vec![
        // The daemon corroborates its supervised-child identity against this
        // pid (it must equal the live parent) and, where the kernel guard is
        // the parent-death signal, uses it to confirm the supervisor is still
        // alive after exec.
        (
            SUPERVISED_PARENT_PID_ENV.to_owned(),
            std::process::id().to_string(),
        ),
        // Neutral launcher declaration (Design 72 L1); every platform's
        // daemon reads it.
        (
            SERVICE_IDENTITY_ENV.to_owned(),
            ServiceIdentity::APP_SIDECAR.declaration(),
        ),
        // Legacy macOS claim kept equal to the neutral one until the
        // supported-version floor moves (spec 77 H1.5).
        #[cfg(target_os = "macos")]
        (
            "HYPERCOLOR_MACOS_OWNER".to_owned(),
            "app-sidecar".to_owned(),
        ),
    ];

    #[cfg(target_os = "macos")]
    {
        args.extend(["--macos-owner".to_owned(), "app-sidecar".to_owned()]);
    }

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
        environment,
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
fn system_url(base: &Url) -> Url {
    base.join("/api/v1/system")
        .expect("static system endpoint path should be valid")
}

fn health_verified_daemon_connection(base: &Url) -> VerifiedDaemonConnection {
    VerifiedDaemonConnection {
        base_url: base.as_str().trim_end_matches('/').to_owned(),
        server_session_id: None,
        protected_control_credential: None,
    }
}

#[cfg(target_os = "macos")]
fn canonical_daemon_guard_path() -> PathBuf {
    std::env::temp_dir().join("hypercolor-daemon.lock")
}

#[cfg(target_os = "macos")]
fn daemon_guard_is_contended(path: &Path) -> bool {
    try_acquire_macos_daemon_guard(&path.to_string_lossy()).is_ok_and(|guard| guard.is_none())
}

#[cfg(target_os = "macos")]
fn daemon_base_is_loopback(base: &Url) -> bool {
    if !matches!(base.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = base.host_str() else {
        return false;
    };
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(target_os = "macos")]
#[cfg(target_os = "macos")]
fn attestation_matches_record(
    attestation: &MacosDaemonSessionAttestation,
    record: &hypercolor_macos_owner::MacosOwnerRecord,
    expected_owner: MacosDaemonOwner,
) -> bool {
    record.active_owner == expected_owner && attestation.owner_incarnation() == record.incarnation()
}

#[cfg(target_os = "macos")]
async fn verify_macos_daemon_connection(
    client: &reqwest::Client,
    base: &Url,
    store: &MacosOwnerStore,
    guard_path: &Path,
    state: &SupervisorState,
    expected_owner: MacosDaemonOwner,
) -> Option<VerifiedDaemonConnection> {
    if !daemon_base_is_loopback(base) {
        tracing::warn!("macOS daemon verification rejected a non-loopback endpoint");
        return None;
    }
    let record = match store.load_owner_record() {
        Ok(Some(record)) => record,
        Ok(None) => {
            tracing::warn!("macOS daemon verification found no owner record");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "macOS daemon verification could not load the owner record");
            return None;
        }
    };
    let attestation = match store.load_daemon_session_attestation() {
        Ok(Some(attestation)) => attestation,
        Ok(None) => {
            tracing::warn!("macOS daemon verification found no session attestation");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "macOS daemon verification could not load the session attestation");
            return None;
        }
    };
    if !attestation_matches_record(&attestation, &record, expected_owner) {
        tracing::warn!("macOS daemon verification found mismatched owner artifacts");
        return None;
    }
    let incarnation = record.incarnation();
    if expected_owner == MacosDaemonOwner::AppSidecar && !state.app_sidecar_is_live(&incarnation) {
        tracing::warn!("macOS daemon verification found no live retained sidecar child");
        return None;
    }
    if !daemon_guard_is_contended(guard_path) {
        tracing::warn!("macOS daemon verification found an unclaimed daemon guard");
        return None;
    }

    let response = client
        .get(system_url(base))
        .timeout(HEALTH_PROBE_TIMEOUT)
        .send()
        .await;
    let Ok(response) = response else {
        tracing::warn!("macOS daemon verification could not read the server identity");
        return None;
    };
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "macOS daemon verification received a failed server identity response");
        return None;
    }
    let Ok(envelope) = response.json::<ApiResponse<SystemResource>>().await else {
        tracing::warn!("macOS daemon verification could not decode the server identity");
        return None;
    };
    let Some(observed_session) = envelope.data.identity.server_session_id else {
        tracing::warn!("macOS daemon verification found no server session identifier");
        return None;
    };

    if !daemon_guard_is_contended(guard_path) {
        tracing::warn!("macOS daemon verification lost the daemon guard during verification");
        return None;
    }
    let current_record = match store.load_owner_record() {
        Ok(Some(record)) => record,
        _ => {
            tracing::warn!("macOS daemon verification lost the owner record during verification");
            return None;
        }
    };
    let current_attestation = match store.load_daemon_session_attestation() {
        Ok(Some(attestation)) => attestation,
        _ => {
            tracing::warn!(
                "macOS daemon verification lost the session attestation during verification"
            );
            return None;
        }
    };
    if current_record != record
        || current_attestation != attestation
        || observed_session != attestation.server_session_id.as_str()
    {
        tracing::warn!("macOS daemon verification observed session drift");
        return None;
    }
    if expected_owner == MacosDaemonOwner::AppSidecar && !state.app_sidecar_is_live(&incarnation) {
        tracing::warn!("macOS daemon verification lost the retained sidecar child");
        return None;
    }

    Some(VerifiedDaemonConnection {
        base_url: base.as_str().trim_end_matches('/').to_owned(),
        server_session_id: Some(attestation.server_session_id),
        protected_control_credential: Some(attestation.protected_control_credential),
    })
}

/// Why the external-owner monitor re-verifies the connection.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalOwnerChange {
    /// The owner store directory changed (record, attestation, journal).
    OwnerStore,
    /// The daemon's event socket opened: a live session to confirm.
    SessionOpened,
    /// The daemon's event socket closed or could not open: the session the
    /// verified connection named may be gone.
    SessionLost,
    /// The daemon published a launcher identity change.
    IdentityChanged,
}

/// Keep the verified connection to a selected external owner current.
///
/// Re-verification is event driven: a `notify` watch on the owner store
/// directory reports record and attestation changes, and one WebSocket
/// subscription to the daemon's `events` topic reports session loss (the
/// socket closes when the daemon exits or restarts) and identity changes.
/// There is no periodic poll; the socket reconnect after a loss uses the
/// restart backoff ladder.
#[cfg(target_os = "macos")]
async fn monitor_external_macos_daemon_connection(
    client: &reqwest::Client,
    base: &Url,
    state: &SupervisorState,
    expected_owner: MacosDaemonOwner,
) {
    let store = MacosOwnerStore::new(data_dir());
    let guard_path = canonical_daemon_guard_path();
    let (change_tx, mut change_rx) = tokio::sync::mpsc::channel::<ExternalOwnerChange>(16);
    let _store_watch = match watch_owner_store(&store, change_tx.clone()) {
        Ok(watch) => Some(watch),
        Err(error) => {
            tracing::warn!(%error, "owner store watch unavailable; relying on the daemon session socket");
            None
        }
    };
    let session_watch = tokio::spawn(watch_daemon_session(base.clone(), change_tx));
    while let Some(change) = change_rx.recv().await {
        tracing::debug!(?change, "re-verifying the external macOS daemon connection");
        let connection = verify_macos_daemon_connection(
            client,
            base,
            &store,
            &guard_path,
            state,
            expected_owner,
        )
        .await;
        state.replace_verified_connection(connection);
    }
    session_watch.abort();
}

#[cfg(target_os = "macos")]
fn watch_owner_store(
    store: &MacosOwnerStore,
    change_tx: tokio::sync::mpsc::Sender<ExternalOwnerChange>,
) -> Result<notify::RecommendedWatcher> {
    use notify::Watcher as _;

    let directory = store
        .owner_record_path()
        .parent()
        .map(Path::to_path_buf)
        .context("owner record path has no parent directory")?;
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            // A full queue already carries a pending re-verify; dropping the
            // duplicate loses nothing.
            let _ = change_tx.try_send(ExternalOwnerChange::OwnerStore);
        }
    })
    .context("failed to create the owner store watcher")?;
    watcher
        .watch(&directory, notify::RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch {}", directory.display()))?;
    Ok(watcher)
}

/// Hold one `events` subscription to the daemon and report session edges.
#[cfg(target_os = "macos")]
async fn watch_daemon_session(
    base: Url,
    change_tx: tokio::sync::mpsc::Sender<ExternalOwnerChange>,
) {
    use futures_util::{SinkExt as _, StreamExt as _};

    let Some(ws_url) = daemon_events_socket_url(&base) else {
        tracing::warn!(url = %base, "daemon base URL has no WebSocket form; session watch disabled");
        return;
    };
    let mut attempt: u32 = 0;
    loop {
        let connected = match tokio_tungstenite::connect_async(ws_url.as_str()).await {
            Ok((stream, _)) => Some(stream),
            Err(error) => {
                tracing::debug!(%error, "daemon session socket unavailable");
                None
            }
        };
        if let Some(stream) = connected {
            let (mut write, mut read) = stream.split();
            let subscribe = serde_json::json!({
                "type": "subscribe",
                "topics": [{ "topic": "events" }]
            });
            if write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    subscribe.to_string().into(),
                ))
                .await
                .is_ok()
            {
                attempt = 0;
                if change_tx
                    .send(ExternalOwnerChange::SessionOpened)
                    .await
                    .is_err()
                {
                    return;
                }
                while let Some(Ok(message)) = read.next().await {
                    if let tokio_tungstenite::tungstenite::Message::Text(text) = message
                        && text.contains("service_identity_changed")
                        && change_tx
                            .send(ExternalOwnerChange::IdentityChanged)
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
            }
        }
        if change_tx
            .send(ExternalOwnerChange::SessionLost)
            .await
            .is_err()
        {
            return;
        }
        tokio::time::sleep(restart_backoff(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

#[cfg(target_os = "macos")]
fn daemon_events_socket_url(base: &Url) -> Option<Url> {
    let mut url = base.join("/api/v1/ws").ok()?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return None,
    };
    url.set_scheme(scheme).ok()?;
    Some(url)
}

#[cfg(target_os = "macos")]
const fn system_status_owner_matches(
    selected_owner: MacosDaemonOwner,
    observed_owner: MacosCapabilityOwner,
) -> bool {
    matches!(
        (selected_owner, observed_owner),
        (
            MacosDaemonOwner::AppSidecar,
            MacosCapabilityOwner::AppSidecar
        ) | (
            MacosDaemonOwner::DirectLaunchd,
            MacosCapabilityOwner::LaunchdService
        ) | (
            MacosDaemonOwner::Homebrew,
            MacosCapabilityOwner::HomebrewService
        ) | (
            MacosDaemonOwner::Standalone,
            MacosCapabilityOwner::Standalone
        )
    )
}

#[cfg(target_os = "macos")]
const fn authoritative_owner_matches(
    selected_owner: MacosDaemonOwner,
    after_epoch: Option<u64>,
    ownership: &MacosDaemonOwnershipStatus,
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
        .get(system_url(base))
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
        .json::<ApiResponse<SystemResource>>()
        .await
        .ok()
        .and_then(|envelope| envelope.data.status)
        .and_then(|status| status.macos_daemon_ownership)
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
            // Recovery already spawned and registered the sidecar supervisor;
            // a second plan would race it.
            crate::ownership::MacosStartupRecoveryDisposition::SupervisorStarted => {
                return Ok(());
            }
            // A standalone owner still holds the guard and the user must stop
            // it first; the plan holds rather than spawning beside it.
            crate::ownership::MacosStartupRecoveryDisposition::SuppressSupervisor => {
                return start_with_plan_inputs(
                    app,
                    daemon_url,
                    StartupLauncherInputs::hold(
                        ServiceIdentity::STANDALONE,
                        HoldReason::PendingStandaloneExit,
                    ),
                );
            }
        }
        let external_owner = selected_external_owner_for_startup(&store)?;
        start_with_plan_inputs(
            app,
            daemon_url,
            StartupLauncherInputs::probe(external_owner),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        start_with_plan_inputs(app, daemon_url, StartupLauncherInputs::probe(None))
    }
}

/// How the supervisor should derive its launcher plan at startup.
enum StartupLauncherInputs {
    /// Probe the platform service manager and plan from what it reports.
    Probe {
        external_owner: Option<MacosExternalOwnerMode>,
    },
    /// The plan is already decided: hold with this remedy. Only the macOS
    /// ownership arbitration pre-decides a hold today; other targets reach
    /// holds through the probe.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code))]
    Hold {
        identity: ServiceIdentity,
        reason: HoldReason,
    },
}

impl StartupLauncherInputs {
    const fn probe(external_owner: Option<MacosExternalOwnerMode>) -> Self {
        Self::Probe { external_owner }
    }

    #[cfg(target_os = "macos")]
    const fn hold(identity: ServiceIdentity, reason: HoldReason) -> Self {
        Self::Hold { identity, reason }
    }

    const fn external_owner(&self) -> Option<MacosExternalOwnerMode> {
        match self {
            Self::Probe { external_owner } => *external_owner,
            Self::Hold { .. } => None,
        }
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
    start_with_plan_inputs(app, daemon_url, StartupLauncherInputs::probe(None))
}

fn start_with_plan_inputs<R: Runtime>(
    app: &AppHandle<R>,
    daemon_url: Url,
    inputs: StartupLauncherInputs,
) -> Result<()> {
    let external_owner = inputs.external_owner();
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
    let event_app = app.clone();
    state.install_verified_connection_emitter(Arc::new(move |connection| {
        if let Err(error) = event_app.emit(VERIFIED_DAEMON_CONNECTION_CHANGED_EVENT, connection) {
            tracing::warn!(%error, "failed to publish verified daemon session change");
        }
    }));
    state.clear_verified_connection();
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let spawn = build_daemon_command(
            &daemon_path,
            &bind,
            ui_dir.as_deref(),
            effects_dir.as_deref(),
        );
        let plan = match inputs {
            StartupLauncherInputs::Hold { identity, reason } => {
                LauncherPlan::Hold { identity, reason }
            }
            StartupLauncherInputs::Probe { external_owner } => {
                let (probe, preference) =
                    probe_launcher(&client, &daemon_url, &state, external_owner).await;
                launcher_plan(&probe, &preference, &daemon_url, spawn)
            }
        };
        execute_launcher_plan(
            plan,
            LauncherPlanContext {
                app,
                client,
                daemon_url,
                daemon_path,
                bind,
                ui_dir,
                effects_dir,
                state,
                external_owner,
            },
        )
        .await;
    });

    Ok(())
}

struct LauncherPlanContext<R: Runtime> {
    app: AppHandle<R>,
    client: reqwest::Client,
    daemon_url: Url,
    daemon_path: PathBuf,
    bind: String,
    ui_dir: Option<PathBuf>,
    effects_dir: Option<PathBuf>,
    state: SupervisorState,
    external_owner: Option<MacosExternalOwnerMode>,
}

/// Measure the launcher already registered for the daemon and the owner
/// preference that constrains the plan.
///
/// macOS probes the selected external owner through the owner store and
/// session attestation; Linux asks systemd about the user unit; Windows
/// asks the Service Control Manager; every platform falls back to an
/// endpoint health probe, which reports an unidentified daemon as
/// standalone.
async fn probe_launcher(
    client: &reqwest::Client,
    daemon_url: &Url,
    state: &SupervisorState,
    external_owner: Option<MacosExternalOwnerMode>,
) -> (LauncherProbe, OwnerPreference) {
    #[cfg(target_os = "macos")]
    if let Some(owner) = external_owner {
        let identity = crate::ownership::service_identity(external_mode_owner(owner));
        let connection = verify_macos_daemon_connection(
            client,
            daemon_url,
            &MacosOwnerStore::new(data_dir()),
            &canonical_daemon_guard_path(),
            state,
            external_mode_owner(owner),
        )
        .await;
        let probe = if connection.is_some() {
            state.replace_verified_connection(connection);
            LauncherProbe::online(identity.clone())
        } else {
            LauncherProbe::offline(identity.clone())
        };
        return (probe, OwnerPreference::Selected(identity));
    }
    #[cfg(target_os = "macos")]
    {
        // Without a selected external owner the app owns the sidecar; the
        // watchdog reconciles any already-running sidecar itself.
        let _ = (client, daemon_url, state);
        (LauncherProbe::NOTHING, OwnerPreference::Flexible)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = external_owner;
        let managed = registered_launcher_probe();
        if let Some(probe) = managed {
            return (probe, OwnerPreference::Flexible);
        }
        let probe = if probe_health(client, daemon_url, HEALTH_PROBE_TIMEOUT).await {
            state.replace_verified_connection(Some(health_verified_daemon_connection(daemon_url)));
            LauncherProbe::online(ServiceIdentity::STANDALONE)
        } else {
            LauncherProbe::NOTHING
        };
        (probe, OwnerPreference::Flexible)
    }
}

#[cfg(target_os = "linux")]
fn registered_launcher_probe() -> Option<LauncherProbe> {
    match detect_systemd_user_service() {
        SystemdUserServiceProbe::Unavailable => None,
        probe => Some(probe.into()),
    }
}

#[cfg(target_os = "windows")]
fn registered_launcher_probe() -> Option<LauncherProbe> {
    LauncherProbe::from_launcher_status(&crate::support::detect_daemon_launcher())
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn registered_launcher_probe() -> Option<LauncherProbe> {
    None
}

async fn execute_launcher_plan<R: Runtime>(plan: LauncherPlan, context: LauncherPlanContext<R>) {
    let LauncherPlanContext {
        app,
        client,
        daemon_url,
        daemon_path,
        bind,
        ui_dir,
        effects_dir,
        state,
        external_owner,
    } = context;
    match plan {
        LauncherPlan::Reuse { identity, endpoint } => {
            state.set_macos_owner_offline(None);
            tracing::info!(
                url = %endpoint,
                launcher = %identity,
                "daemon already running; reusing existing instance"
            );
            if state.verified_daemon_connection().connection.is_none() {
                if wait_until_healthy(
                    &client,
                    &endpoint,
                    DAEMON_STARTUP_TIMEOUT,
                    DAEMON_STARTUP_POLL_INTERVAL,
                )
                .await
                {
                    state.replace_verified_connection(Some(health_verified_daemon_connection(
                        &endpoint,
                    )));
                } else {
                    tracing::warn!(
                        launcher = %identity,
                        timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                        "registered launcher is active but the daemon did not become healthy"
                    );
                }
            }
            #[cfg(target_os = "macos")]
            if let Some(owner) = external_owner {
                monitor_external_macos_daemon_connection(
                    &client,
                    &endpoint,
                    &state,
                    external_mode_owner(owner),
                )
                .await;
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (external_owner, app);
        }
        LauncherPlan::Hold { identity, reason } => {
            tracing::warn!(
                launcher = %identity,
                ?reason,
                "supervisor holding; no daemon will be spawned beside the selected owner"
            );
            if let Some(owner) = external_owner {
                let status = macos_external_owner_offline(owner);
                state.set_macos_owner_offline(Some(status));
                if let Err(error) = app.emit("macos_daemon_owner_offline", status) {
                    tracing::warn!(%error, "failed to publish app-local daemon owner status");
                }
            }
        }
        LauncherPlan::Start { identity, unit } => {
            tracing::info!(launcher = %identity, unit, "starting registered launcher");
            if start_registered_launcher(&unit) {
                if wait_until_healthy(
                    &client,
                    &daemon_url,
                    DAEMON_STARTUP_TIMEOUT,
                    DAEMON_STARTUP_POLL_INTERVAL,
                )
                .await
                {
                    tracing::info!(launcher = %identity, "registered launcher reported healthy");
                    state.replace_verified_connection(Some(health_verified_daemon_connection(
                        &daemon_url,
                    )));
                } else {
                    tracing::warn!(
                        launcher = %identity,
                        timeout_ms = DAEMON_STARTUP_TIMEOUT.as_millis(),
                        "started registered launcher but the daemon did not become healthy"
                    );
                }
                return;
            }
            // The service manager refused (typically privilege on the SCM
            // path). The registration is stopped, so a supervised child
            // cannot collide with it; fall through explicitly.
            tracing::warn!(
                launcher = %identity,
                "registered launcher did not start; spawning a supervised child instead"
            );
            spawn_supervised_child(
                client,
                daemon_url,
                daemon_path,
                bind,
                ui_dir,
                effects_dir,
                state,
            )
            .await;
        }
        LauncherPlan::SpawnChild { .. } => {
            spawn_supervised_child(
                client,
                daemon_url,
                daemon_path,
                bind,
                ui_dir,
                effects_dir,
                state,
            )
            .await;
        }
    }
}

async fn spawn_supervised_child(
    client: reqwest::Client,
    daemon_url: Url,
    daemon_path: PathBuf,
    bind: String,
    ui_dir: Option<PathBuf>,
    effects_dir: Option<PathBuf>,
    state: SupervisorState,
) {
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
}

#[cfg(target_os = "linux")]
fn start_registered_launcher(unit: &str) -> bool {
    match start_systemd_user_service(unit) {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(unit, status = ?status.code(), "failed to start systemd user service");
            false
        }
        Err(error) => {
            tracing::warn!(unit, %error, "failed to run systemctl for systemd user service");
            false
        }
    }
}

#[cfg(target_os = "windows")]
fn start_registered_launcher(unit: &str) -> bool {
    let mut command = Command::new("sc.exe");
    command.args(["start", unit]);
    crate::process_ext::hide_console_window(&mut command);
    match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(unit, status = ?status.code(), "failed to start the Windows service");
            false
        }
        Err(error) => {
            tracing::warn!(unit, %error, "failed to run sc.exe for the Windows service");
            false
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn start_registered_launcher(unit: &str) -> bool {
    tracing::warn!(unit, "no service manager start path on this platform");
    false
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
                drop(daemon);
                // A conflict exit is the primary "someone else holds our
                // guard" signal: when a same-owner contender loses guard
                // arbitration it exits terminally within a few hundred
                // milliseconds, often before the orphan even answers a
                // health probe. Reclaim covers the orphaned-app-sidecar
                // holder; a legitimate external owner declines inside.
                // Reclaims charge the restart budget so a pathological
                // reclaim loop (say, two live app instances fighting)
                // still trips the circuit breaker instead of ping-ponging
                // forever; the budget resets after stable uptime.
                #[cfg(target_os = "macos")]
                if reclaim_stale_app_sidecar(pid).await {
                    record_failure(&mut restart_count, &mut window_anchor);
                    continue;
                }
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
            // The classic cause: an orphaned daemon from a dead app
            // instance still holds the guard and answered the health
            // probe on behalf of our child. A successful reclaim skips
            // the backoff but still charges the restart budget, so a
            // pathological reclaim loop trips the circuit breaker.
            record_failure(&mut restart_count, &mut window_anchor);
            if reclaim_stale_app_sidecar(pid).await {
                continue;
            }
            tokio::time::sleep(restart_backoff(restart_count)).await;
            continue;
        }
        #[cfg(target_os = "macos")]
        {
            let verified = verify_macos_daemon_connection(
                &client,
                &daemon_url,
                &MacosOwnerStore::new(data_dir()),
                &canonical_daemon_guard_path(),
                &state,
                MacosDaemonOwner::AppSidecar,
            )
            .await;
            let Some(verified) = verified else {
                tracing::error!(pid, "healthy app sidecar failed exact session verification");
                state.clear_child();
                drop(daemon);
                record_failure(&mut restart_count, &mut window_anchor);
                if reclaim_stale_app_sidecar(pid).await {
                    continue;
                }
                tokio::time::sleep(restart_backoff(restart_count)).await;
                continue;
            };
            state.replace_verified_connection(Some(verified));
        }
        #[cfg(not(target_os = "macos"))]
        state.replace_verified_connection(Some(health_verified_daemon_connection(&daemon_url)));
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

/// Attempt to reclaim ownership from an orphaned app-sidecar daemon.
///
/// A previous app instance that died without reaping leaves its daemon
/// holding the port, the flock guard, and an owner record naming a pid this
/// supervisor never spawned. The single-instance plugin guarantees no other
/// live app owns that child, so after verifying the live process matches
/// the recorded identity the orphan is asked to terminate, and the guard
/// release is awaited so the next spawn can win it cleanly. Returns true
/// when the guard was reclaimed and a respawn should proceed immediately.
///
/// Spec 77 invariant 8 (managed owners stop through the topology that
/// launched them) holds here: the recorded owner is AppSidecar, this app is
/// the single live instance of that topology, and the launching instance no
/// longer exists, so this is the owning topology reaping its own orphan,
/// not a handover signaling a foreign pid.
#[cfg(target_os = "macos")]
async fn reclaim_stale_app_sidecar(current_child_pid: u32) -> bool {
    let record = match MacosOwnerStore::new(data_dir()).load_owner_record() {
        Ok(Some(record)) => record,
        Ok(None) => return false,
        Err(error) => {
            tracing::warn!(%error, "owner record unreadable during stale-sidecar reclaim");
            return false;
        }
    };
    if record.active_owner != MacosDaemonOwner::AppSidecar {
        return false;
    }
    let stale_pid = record.active_identity.pid;
    if stale_pid == current_child_pid || stale_pid == std::process::id() {
        return false;
    }
    let guard_path = canonical_daemon_guard_path();
    if !daemon_guard_is_contended(&guard_path) {
        // The flock dies with its holder: an uncontended guard means the
        // recorded daemon is already gone and there is nothing to reclaim.
        return false;
    }
    if !process_matches_identity(&record.active_identity) {
        tracing::warn!(
            stale_pid,
            "recorded app-sidecar owner does not match the live process; refusing to signal"
        );
        return false;
    }
    tracing::warn!(
        stale_pid,
        "orphaned app-sidecar daemon holds the guard; requesting termination"
    );
    if let Err(error) = hypercolor_macos_owner::request_macos_pid_termination(stale_pid) {
        tracing::warn!(stale_pid, %error, "stale app-sidecar termination failed");
        return false;
    }
    let released = tokio::task::spawn_blocking(move || {
        hypercolor_macos_owner::wait_for_macos_guard_release(
            Duration::from_secs(10),
            &guard_path.to_string_lossy(),
        )
    })
    .await;
    match released {
        Ok(Ok(true)) => {
            tracing::info!(stale_pid, "stale app-sidecar guard reclaimed");
            true
        }
        Ok(Ok(false)) => {
            tracing::warn!(
                stale_pid,
                "stale app-sidecar ignored termination; guard still held"
            );
            false
        }
        Ok(Err(error)) => {
            tracing::warn!(stale_pid, %error, "guard release wait failed during reclaim");
            false
        }
        Err(error) => {
            tracing::warn!(stale_pid, %error, "guard release task failed during reclaim");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn process_matches_identity(identity: &hypercolor_macos_owner::MacosOwnerIdentity) -> bool {
    use core_foundation::{base::TCFType, data::CFData};
    use security_framework::os::macos::code_signing::{
        Flags as CodeSigningFlags, GuestAttributes, SecCode,
    };

    let mut audit_token = [0_u8; 32];
    let mut words = identity.audit_token_identity.split(':');
    for index in 0..8 {
        let Some(word) = words.next() else {
            return false;
        };
        if word.len() != 8 || !word.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
        let Ok(value) = u32::from_str_radix(word, 16) else {
            return false;
        };
        if index == 5 && value != identity.pid {
            return false;
        }
        audit_token[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    if words.next().is_some() {
        return false;
    }

    let token_data = CFData::from_buffer(&audit_token);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(token_data.as_concrete_TypeRef());
    let Ok(code) = SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE)
    else {
        return false;
    };
    let Some(executable) = code
        .path(CodeSigningFlags::NONE)
        .ok()
        .and_then(|url| url.to_path())
    else {
        return false;
    };

    match (
        executable.canonicalize(),
        identity.executable_path.canonicalize(),
    ) {
        (Ok(live), Ok(recorded)) => live == recorded,
        _ => false,
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
#[allow(
    clippy::items_after_test_module,
    reason = "state-machine fixtures stay adjacent while platform spawn helpers remain last"
)]
mod tests {
    use super::{
        DaemonStartupObservation, MacosDaemonOwnerOfflineStatus, macos_external_owner_offline,
        select_daemon_startup_observation,
    };
    use hypercolor_macos_owner::{MacosDaemonOwner, MacosExternalOwnerMode, MacosOwnerRemedy};

    #[cfg(target_os = "macos")]
    use hypercolor_types::api::system::{
        MacosCapabilityOwner, MacosDaemonOwnershipStatus, ServerInfo, SystemResource, SystemStatus,
    };
    #[cfg(target_os = "macos")]
    use hypercolor_types::api::{ApiResponse, ResponseMeta};

    #[cfg(target_os = "macos")]
    fn system_response_body(identity: ServerInfo, status: Option<SystemStatus>) -> String {
        serde_json::to_string(&ApiResponse {
            data: SystemResource { identity, status },
            meta: ResponseMeta {
                api_version: "v1".to_owned(),
                request_id: "req_test".to_owned(),
                timestamp: "2026-08-20T00:00:00Z".to_owned(),
            },
        })
        .expect("system response should encode")
    }

    #[cfg(target_os = "macos")]
    fn system_status_body(owner: MacosCapabilityOwner, owner_epoch: u64) -> String {
        system_response_body(
            ServerInfo::default(),
            Some(SystemStatus {
                macos_daemon_ownership: Some(MacosDaemonOwnershipStatus {
                    active_owner: owner,
                    owner_epoch,
                    ..MacosDaemonOwnershipStatus::default()
                }),
                ..SystemStatus::default()
            }),
        )
    }

    #[cfg(target_os = "macos")]
    fn external_session_fixture() -> (
        tempfile::TempDir,
        hypercolor_macos_owner::MacosOwnerStore,
        hypercolor_macos_owner::MacosDaemonGuard,
        hypercolor_macos_owner::MacosDaemonSessionAttestation,
        std::path::PathBuf,
    ) {
        use hypercolor_macos_owner::{
            MacosOwnerIdentity, MacosOwnerStore, try_acquire_macos_daemon_guard,
        };

        let directory = tempfile::tempdir().expect("temporary directory should build");
        let guard_path = directory.path().join("daemon.lock");
        let guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("guard acquisition should succeed")
            .expect("fixture should win the guard");
        let store = MacosOwnerStore::new(directory.path().join("store"));
        let record = store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                MacosOwnerIdentity::new(
                    "audit-launchd",
                    "/usr/local/bin/hypercolor-daemon",
                    "requirement-launchd",
                    std::process::id(),
                )
                .expect("identity should build"),
            )
            .expect("owner should publish");
        let attestation = store
            .publish_daemon_session_attestation(&guard, &record.incarnation())
            .expect("session should publish");
        (directory, store, guard, attestation, guard_path)
    }

    #[cfg(target_os = "macos")]
    async fn system_fixture(body: String) -> (url::Url, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener should bind");
        let address = listener
            .local_addr()
            .expect("fixture address should resolve");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let mut request = [0_u8; 4_096];
            let read = stream
                .read(&mut request)
                .await
                .expect("request should read");
            let request = std::str::from_utf8(&request[..read])
                .expect("request should contain UTF-8 headers");
            assert!(request.starts_with("GET /api/v1/system HTTP/1.1"));
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        });
        (
            url::Url::parse(&format!("http://{address}")).expect("fixture daemon URL should parse"),
            server,
        )
    }

    #[cfg(target_os = "macos")]
    async fn authoritative_probe_fixture(
        status_body: String,
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
            for (expected_path, body) in
                [("/health", "{}"), ("/api/v1/system", status_body.as_str())]
            {
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
    #[tokio::test]
    async fn verified_connection_requires_matching_endpoint_session_and_live_guard() {
        let (_directory, store, guard, attestation, guard_path) = external_session_fixture();
        let body = system_response_body(
            ServerInfo {
                server_session_id: Some(attestation.server_session_id.as_str().to_owned()),
                ..ServerInfo::default()
            },
            None,
        );
        let (base, server) = system_fixture(body).await;
        let state = super::SupervisorState::default();

        let verified = super::verify_macos_daemon_connection(
            &reqwest::Client::new(),
            &base,
            &store,
            &guard_path,
            &state,
            MacosDaemonOwner::DirectLaunchd,
        )
        .await;
        server.await.expect("fixture server should finish");
        let verified = verified.expect("matching live session should verify");
        assert_eq!(
            verified.server_session_id,
            Some(attestation.server_session_id)
        );
        assert_eq!(
            verified.protected_control_credential,
            Some(attestation.protected_control_credential)
        );
        drop(guard);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn wrong_or_missing_endpoint_session_never_exposes_credential() {
        let (_directory, store, _guard, _attestation, guard_path) = external_session_fixture();
        let state = super::SupervisorState::default();
        for body in [
            system_response_body(ServerInfo::default(), None),
            system_response_body(
                ServerInfo {
                    server_session_id: Some(
                        hypercolor_macos_owner::MacosServerSessionId::from_bytes([0x77; 16])
                            .as_str()
                            .to_owned(),
                    ),
                    ..ServerInfo::default()
                },
                None,
            ),
        ] {
            let (base, server) = system_fixture(body).await;
            assert!(
                super::verify_macos_daemon_connection(
                    &reqwest::Client::new(),
                    &base,
                    &store,
                    &guard_path,
                    &state,
                    MacosDaemonOwner::DirectLaunchd,
                )
                .await
                .is_none()
            );
            server.await.expect("fixture server should finish");
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stale_crash_artifacts_and_replayed_session_fail_without_live_guard() {
        let (_directory, store, guard, attestation, guard_path) = external_session_fixture();
        let replayed_session = attestation.server_session_id.clone();
        drop(guard);
        let unreachable =
            url::Url::parse("http://127.0.0.1:9").expect("fixture daemon URL should parse");

        assert_eq!(
            store
                .load_daemon_session_attestation()
                .expect("stale matching artifacts should still load")
                .expect("stale attestation should remain")
                .server_session_id,
            replayed_session
        );
        assert!(
            super::verify_macos_daemon_connection(
                &reqwest::Client::new(),
                &unreachable,
                &store,
                &guard_path,
                &super::SupervisorState::default(),
                MacosDaemonOwner::DirectLaunchd,
            )
            .await
            .is_none()
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn nonloopback_daemon_base_never_enters_session_verification() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let base =
            url::Url::parse("https://attacker.example:9420").expect("fixture URL should parse");
        assert!(
            super::verify_macos_daemon_connection(
                &reqwest::Client::new(),
                &base,
                &hypercolor_macos_owner::MacosOwnerStore::new(directory.path()),
                &directory.path().join("daemon.lock"),
                &super::SupervisorState::default(),
                MacosDaemonOwner::DirectLaunchd,
            )
            .await
            .is_none()
        );
        assert!(!super::daemon_base_is_loopback(&base));
        assert!(super::daemon_base_is_loopback(
            &url::Url::parse("http://[::1]:9420").expect("loopback URL should parse")
        ));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn prebound_unrelated_listener_cannot_become_the_app_sidecar_endpoint() {
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};

        use hypercolor_macos_owner::{
            MacosOwnerIdentity, MacosOwnerStore, try_acquire_macos_daemon_guard,
        };

        let directory = tempfile::tempdir().expect("temporary directory should build");
        let guard_path = directory.path().join("daemon.lock");
        let guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("guard acquisition should succeed")
            .expect("fixture should win the guard");
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fixture child should spawn");
        let pid = child.id();
        let daemon = Arc::new(Mutex::new(super::ManagedDaemon {
            child: Some(child),
            platform_guard: super::PlatformGuard,
        }));
        let store = MacosOwnerStore::new(directory.path().join("store"));
        let record = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                MacosOwnerIdentity::new(
                    "audit-sidecar",
                    "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                    "requirement-sidecar",
                    pid,
                )
                .expect("identity should build"),
            )
            .expect("owner should publish");
        let state = super::SupervisorState::default();
        state
            .register_app_sidecar_child(record.incarnation(), Arc::clone(&daemon))
            .expect("matching child should register");
        assert!(state.app_sidecar_is_live(&record.incarnation()));
        let unrelated_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("unrelated listener should pre-bind a loopback port");
        let unrelated_address = unrelated_listener
            .local_addr()
            .expect("unrelated listener address should resolve");
        let base = url::Url::parse(&format!("http://{unrelated_address}"))
            .expect("unrelated listener URL should parse");

        assert!(
            super::verify_macos_daemon_connection(
                &reqwest::Client::new(),
                &base,
                &store,
                &guard_path,
                &state,
                MacosDaemonOwner::AppSidecar,
            )
            .await
            .is_none()
        );
        assert!(
            store
                .load_daemon_session_attestation()
                .expect("session state should load")
                .is_none()
        );
        drop(unrelated_listener);
        state.clear_child();
        drop(daemon);
        drop(guard);
    }

    #[test]
    fn clearing_verified_state_emits_offline_and_removes_old_credential() {
        use std::sync::{Arc, Mutex, PoisonError};

        let state = super::SupervisorState::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        state.install_verified_connection_emitter(Arc::new(move |connection| {
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(connection);
        }));
        state.replace_verified_connection(Some(super::VerifiedDaemonConnection {
            base_url: "http://127.0.0.1:9420".to_owned(),
            server_session_id: Some(hypercolor_macos_owner::MacosServerSessionId::from_bytes(
                [0x11; 16],
            )),
            protected_control_credential: Some(
                hypercolor_macos_owner::MacosProtectedControlCredential::from_bytes([0x22; 32]),
            ),
        }));
        state.clear_verified_connection();

        assert!(state.verified_daemon_connection().connection.is_none());
        let events = events.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].revision, 1);
        assert!(events[0].connection.is_some());
        assert_eq!(events[1].revision, 2);
        assert!(events[1].connection.is_none());
    }

    #[test]
    fn native_health_proof_enables_base_without_macos_session_authority() {
        let state = super::SupervisorState::default();
        assert!(state.verified_daemon_connection().connection.is_none());

        let base = url::Url::parse("https://daemon.lan:19420/").expect("URL should parse");
        state.replace_verified_connection(Some(super::health_verified_daemon_connection(&base)));
        let connection = state
            .verified_daemon_connection()
            .connection
            .expect("health-proven native daemon should publish its route");
        assert_eq!(connection.base_url, "https://daemon.lan:19420");
        assert!(connection.server_session_id.is_none());
        assert!(connection.protected_control_credential.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn authoritative_system_status_requires_exact_owner_and_newer_epoch() {
        use super::{authoritative_owner_matches, system_url};

        let base = url::Url::parse("http://127.0.0.1:9420").expect("URL should parse");
        assert_eq!(
            system_url(&base).as_str(),
            "http://127.0.0.1:9420/api/v1/system"
        );

        let launchd: ApiResponse<SystemResource> =
            serde_json::from_str(&system_status_body(MacosCapabilityOwner::LaunchdService, 8))
                .expect("launchd status should decode");
        let launchd = launchd
            .data
            .status
            .expect("status should be present")
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

        let missing: ApiResponse<SystemResource> =
            serde_json::from_str(&system_response_body(ServerInfo::default(), None))
                .expect("missing ownership status should decode");
        assert!(missing.data.status.is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn generic_health_does_not_satisfy_authoritative_owner_probe() {
        let launchd = system_status_body(MacosCapabilityOwner::LaunchdService, 8);
        let homebrew = system_status_body(MacosCapabilityOwner::HomebrewService, 9);

        assert!(
            authoritative_probe_fixture(launchd.clone(), MacosDaemonOwner::DirectLaunchd, Some(7))
                .await
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
    fn stale_sidecar_reclaim_requires_the_exact_live_audit_token() {
        use hypercolor_macos_input::current_process_audit_token_identity;
        use hypercolor_macos_owner::MacosOwnerIdentity;

        let executable = std::env::current_exe().expect("test executable should resolve");
        let audit_token = current_process_audit_token_identity()
            .expect("current process audit token should resolve");
        let exact = MacosOwnerIdentity::new(
            audit_token.clone(),
            &executable,
            "requirement-current",
            std::process::id(),
        )
        .expect("current identity should build");
        assert!(super::process_matches_identity(&exact));

        let mut words = audit_token
            .split(':')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        words[7] = format!(
            "{:08x}",
            u32::from_str_radix(&words[7], 16)
                .expect("pid version should parse")
                .wrapping_add(1)
        );
        let stale = MacosOwnerIdentity::new(
            words.join(":"),
            executable,
            "requirement-current",
            std::process::id(),
        )
        .expect("stale identity should build");
        assert!(!super::process_matches_identity(&stale));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn app_exit_never_signals_an_unbound_child_pid() {
        use std::process::{Command, Stdio};

        use super::SupervisorState;

        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fixture child should spawn");
        let state = SupervisorState::default();
        state.replace_child_pid(child.id());

        state.terminate_managed_daemon_for_exit();

        assert!(
            child
                .try_wait()
                .expect("fixture child state should read")
                .is_none(),
            "app exit must not signal a pid without a retained child handle"
        );
        child.kill().expect("fixture child should stop");
        child.wait().expect("fixture child should reap");
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
fn start_systemd_user_service(unit: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("systemctl")
        .args(["--user", "start", unit])
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
        .envs(command.environment.iter().map(|(key, value)| (key, value)))
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
    // Linux: the kernel delivers SIGTERM to the child when this process
    // exits (pdeathsig), so supervisor death is a kernel fact rather than
    // something the daemon has to notice. macOS arms the equivalent on the
    // daemon side with a kqueue EVFILT_PROC watch on the parent pid.
    #[cfg(target_os = "linux")]
    hypercolor_linux_session::arm_parent_death(command, std::process::id());
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the platform helper signature aligned with Windows"
)]
fn attach_platform_guard(_child: &Child) -> Result<PlatformGuard> {
    Ok(PlatformGuard)
}
