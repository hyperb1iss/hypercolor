//! Daemon supervision primitives for the unified desktop app.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

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
