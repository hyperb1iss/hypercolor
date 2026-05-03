//! Daemon supervision primitives for the unified desktop app.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;
use tauri::{AppHandle, Manager, Runtime};
use url::Url;

/// Default daemon bind address used by the app-spawned daemon.
pub const DEFAULT_DAEMON_BIND: &str = "127.0.0.1:9420";

/// Timeout for one lightweight health probe.
pub const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Platform-neutral command description for spawning the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCommand {
    /// Daemon executable path.
    pub program: PathBuf,
    /// Daemon command-line arguments.
    pub args: Vec<String>,
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
        "hypercolor-daemon"
    }
}

/// Resolve the daemon path next to the app executable.
#[must_use]
pub fn sibling_daemon_path(current_exe: &Path) -> Option<PathBuf> {
    current_exe
        .parent()
        .map(|install_dir| install_dir.join(daemon_executable_name()))
}

/// Resolve the installed web UI directory next to the app executable.
#[must_use]
pub fn sibling_ui_dir(current_exe: &Path) -> Option<PathBuf> {
    current_exe
        .parent()
        .map(|install_dir| install_dir.join("ui"))
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

/// Start supervising the daemon in the background.
///
/// # Errors
///
/// Returns an error if the app executable path or daemon URL cannot be resolved.
pub fn start<R: Runtime>(app: &AppHandle<R>, daemon_url: Url) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to resolve app executable path")?;
    let daemon_path = sibling_daemon_path(&current_exe)
        .context("failed to resolve daemon path from app executable")?;
    let ui_dir = sibling_ui_dir(&current_exe).filter(|path| path.join("index.html").exists());
    let bind = bind_from_daemon_url(&daemon_url).unwrap_or_else(|| DEFAULT_DAEMON_BIND.to_owned());
    let state = app.state::<SupervisorState>().inner().clone();

    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        if probe_health(&client, &daemon_url, HEALTH_PROBE_TIMEOUT).await {
            tracing::info!(url = %daemon_url, "daemon already running; reusing existing instance");
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
            }
            Err(error) => {
                tracing::warn!(%error, "failed to spawn app-owned daemon");
            }
        }
    });

    Ok(())
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
fn attach_platform_guard(_child: &Child) -> Result<PlatformGuard> {
    Ok(PlatformGuard)
}
