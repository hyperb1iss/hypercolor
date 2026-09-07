//! `hyper service` — daemon service lifecycle management.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::{Result, bail};
use clap::{Args, Subcommand, ValueEnum};

use crate::output::OutputContext;

// ── Constants ───────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "hypercolor";

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = hypercolor_macos_owner::MACOS_DIRECT_LAUNCHD_LABEL;

// ── CLI Args ────────────────────────────────────────────────────────────

/// Daemon service lifecycle management.
#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

/// Service subcommands.
#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Start the daemon service.
    Start,
    /// Stop the daemon service.
    Stop,
    /// Restart the daemon service.
    Restart,
    /// Show service status.
    Status,
    /// Enable autostart on login.
    Enable,
    /// Disable autostart on login.
    Disable,
    /// Show daemon logs.
    Logs(LogsArgs),
    /// Select the local macOS daemon owner.
    ChooseOwner(ChooseOwnerArgs),
}

/// Arguments for `service choose-owner`.
#[derive(Debug, Args)]
pub struct ChooseOwnerArgs {
    /// Installed service topology that should own the daemon.
    #[arg(value_enum)]
    pub owner: MacosServiceOwner,
}

/// macOS service topologies selectable without the app UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MacosServiceOwner {
    /// Daemon supervised by the packaged Hypercolor app.
    AppSidecar,
    /// Hypercolor's directly installed per-user launchd service.
    DirectLaunchd,
    /// The Homebrew-managed per-user service.
    Homebrew,
}

impl MacosServiceOwner {
    /// The neutral launcher identity this choice names.
    #[must_use]
    pub fn identity(self) -> hypercolor_types::service::ServiceIdentity {
        use hypercolor_types::service::ServiceIdentity;
        match self {
            Self::AppSidecar => ServiceIdentity::APP_SIDECAR,
            Self::DirectLaunchd => ServiceIdentity::launchd_direct(),
            Self::Homebrew => ServiceIdentity::homebrew(),
        }
    }
}

/// Arguments for `service logs`.
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Follow log output in real time.
    #[arg(long, short)]
    pub follow: bool,

    /// Number of log lines to show.
    #[arg(long, short = 'n', default_value = "50")]
    pub lines: Option<u32>,

    /// Show logs since a given time (e.g., "1h", "2024-01-01", "today").
    #[arg(long)]
    pub since: Option<String>,
}

// ── Dispatch ────────────────────────────────────────────────────────────

/// Execute the `service` subcommand tree.
///
/// Service commands manage the system daemon directly via platform tools
/// (`systemctl` on Linux, `launchctl` on macOS). They do not communicate
/// with the daemon HTTP API.
///
/// # Errors
///
/// Returns an error if the platform is unsupported or the underlying
/// system command fails.
pub async fn execute(args: &ServiceArgs, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ServiceCommand::Start => execute_start(ctx).await,
        ServiceCommand::Stop => execute_stop(ctx).await,
        ServiceCommand::Restart => execute_restart(ctx).await,
        ServiceCommand::Status => execute_status(ctx).await,
        ServiceCommand::Enable => execute_enable(ctx).await,
        ServiceCommand::Disable => execute_disable(ctx).await,
        ServiceCommand::Logs(logs_args) => execute_logs(logs_args, ctx).await,
        ServiceCommand::ChooseOwner(owner_args) => execute_choose_owner(owner_args, ctx).await,
    }
}

#[cfg(target_os = "macos")]
async fn execute_choose_owner(args: &ChooseOwnerArgs, ctx: &OutputContext) -> Result<()> {
    let owner = macos_owner_for_identity(&args.owner.identity())?;
    let outcome = tokio::task::spawn_blocking(move || choose_owner_locally(owner))
        .await
        .context("macOS daemon owner coordinator task failed")??;
    if ctx.format == crate::output::OutputFormat::Json {
        return ctx.print_json(&serde_json::to_value(&outcome)?);
    }
    match outcome {
        hypercolor_macos_owner::MacosOwnerCoordinatorOutcome::Active {
            owner,
            owner_epoch,
        } => ctx.success(&format!(
            "macOS daemon owner is {owner:?} at epoch {owner_epoch}"
        )),
        hypercolor_macos_owner::MacosOwnerCoordinatorOutcome::PendingStandalone {
            remedy: hypercolor_macos_owner::MacosOwnerRemedy::StopStandaloneOwner { pid },
            ..
        } => ctx.warning(&format!(
            "stop_standalone_owner: stop daemon PID {pid}, then repeat the command"
        )),
        hypercolor_macos_owner::MacosOwnerCoordinatorOutcome::PendingStandalone {
            remedy,
            ..
        } => ctx.warning(&format!("macOS daemon owner handover is pending: {remedy:?}")),
        hypercolor_macos_owner::MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner,
            failure,
        } => ctx.warning(&format!(
            "owner handover rolled back to {prior_owner:?}: {failure}"
        )),
        hypercolor_macos_owner::MacosOwnerCoordinatorOutcome::RecoveryRequired {
            requested_owner,
            prior_owner,
            phase,
        } => ctx.warning(&format!(
            "owner recovery remains pending: requested={requested_owner:?} prior={prior_owner:?} phase={phase:?}"
        )),
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_choose_owner(_args: &ChooseOwnerArgs, _ctx: &OutputContext) -> Result<()> {
    bail!("macOS daemon owner selection is unavailable on this platform")
}

// ── Linux (systemctl) ───────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_start(ctx: &OutputContext) -> Result<()> {
    run_systemctl(&["start", SERVICE_NAME])?;
    ctx.success("Daemon service started");
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_stop(ctx: &OutputContext) -> Result<()> {
    run_systemctl(&["stop", SERVICE_NAME])?;
    ctx.success("Daemon service stopped");
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_restart(ctx: &OutputContext) -> Result<()> {
    run_systemctl(&["restart", SERVICE_NAME])?;
    ctx.success("Daemon service restarted");
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_status(ctx: &OutputContext) -> Result<()> {
    let output = run_systemctl_output(&[
        "show",
        SERVICE_NAME,
        "--property=ActiveState,SubState,MainPID,MemoryCurrent,ActiveEnterTimestamp",
    ])?;

    let mut state = String::from("unknown");
    let mut sub_state = String::new();
    let mut pid = String::new();
    let mut memory = String::new();
    let mut since = String::new();

    for line in output.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "ActiveState" => state = value.to_string(),
                "SubState" => sub_state = value.to_string(),
                "MainPID" => pid = value.to_string(),
                "MemoryCurrent" => {
                    if let Ok(bytes) = value.parse::<u64>() {
                        memory = format_bytes(bytes);
                    }
                }
                "ActiveEnterTimestamp" if !value.is_empty() => {
                    since = value.to_string();
                }
                _ => {}
            }
        }
    }

    let state_display = if sub_state.is_empty() {
        state
    } else if pid != "0" && !pid.is_empty() {
        format!("{state} (pid {pid})")
    } else {
        format!("{state} ({sub_state})")
    };

    println!();
    ctx.info(&format!("Service: {SERVICE_NAME}"));
    ctx.info(&format!("State:   {state_display}"));
    if !since.is_empty() {
        ctx.info(&format!("Since:   {since}"));
    }
    if !memory.is_empty() {
        ctx.info(&format!("Memory:  {memory}"));
    }
    println!();

    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_enable(ctx: &OutputContext) -> Result<()> {
    run_systemctl(&["enable", SERVICE_NAME])?;
    ctx.success("Daemon service enabled for autostart");
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_disable(ctx: &OutputContext) -> Result<()> {
    run_systemctl(&["disable", SERVICE_NAME])?;
    ctx.success("Daemon service disabled for autostart");
    Ok(())
}

#[cfg(target_os = "linux")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_logs(args: &LogsArgs, ctx: &OutputContext) -> Result<()> {
    let _ = ctx; // logs stream directly to stdout
    let mut cmd_args = vec!["--user", "-u", SERVICE_NAME, "--no-pager"];

    let lines_str;
    if let Some(n) = args.lines {
        lines_str = format!("{n}");
        cmd_args.extend_from_slice(&["-n", &lines_str]);
    }

    if let Some(since) = &args.since {
        cmd_args.extend_from_slice(&["--since", since]);
    }

    if args.follow {
        cmd_args.push("-f");
    }

    let status = std::process::Command::new("journalctl")
        .args(&cmd_args)
        .status()
        .context("Failed to run journalctl. Is systemd available?")?;

    if !status.success() {
        bail!("journalctl exited with {status}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_systemctl(args: &[&str]) -> Result<()> {
    let mut cmd_args = vec!["--user"];
    cmd_args.extend_from_slice(args);

    let output = std::process::Command::new("systemctl")
        .args(&cmd_args)
        .output()
        .context("Failed to run systemctl. Is systemd available?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("systemctl {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_systemctl_output(args: &[&str]) -> Result<String> {
    let mut cmd_args = vec!["--user"];
    cmd_args.extend_from_slice(args);

    let output = std::process::Command::new("systemctl")
        .args(&cmd_args)
        .output()
        .context("Failed to run systemctl. Is systemd available?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("systemctl {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ── macOS (launchctl) ───────────────────────────────────────────────────

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_start(ctx: &OutputContext) -> Result<()> {
    let (launchd, target) = direct_launchd_target()?;
    launchd.start(&target, &plist_path()?)?;
    ctx.success("Daemon service started");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_stop(ctx: &OutputContext) -> Result<()> {
    let (launchd, target) = direct_launchd_target()?;
    launchd.stop(&target)?;
    ctx.success("Daemon service stopped");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_restart(ctx: &OutputContext) -> Result<()> {
    let (launchd, target) = direct_launchd_target()?;
    if launchd.is_loaded(&target)? {
        launchd.kickstart_restart(&target)?;
    } else {
        launchd.bootstrap(&plist_path()?)?;
    }
    ctx.success("Daemon service restarted");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_status(ctx: &OutputContext) -> Result<()> {
    let (_, target) = direct_launchd_target()?;

    let output = std::process::Command::new(hypercolor_macos_owner::LAUNCHCTL_PATH)
        .args(["print", &target.service()])
        .output()
        .context("Failed to run launchctl")?;

    if !output.status.success() {
        println!();
        ctx.info(&format!("Service: {LAUNCHD_LABEL}"));
        ctx.info("State:   not loaded");
        println!();
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut state = String::from("loaded");
    let mut pid = String::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("state = ") {
            state = trimmed
                .strip_prefix("state = ")
                .unwrap_or("unknown")
                .to_string();
        } else if trimmed.starts_with("pid = ") {
            pid = trimmed.strip_prefix("pid = ").unwrap_or("").to_string();
        }
    }

    let state_display = if pid.is_empty() {
        state
    } else {
        format!("{state} (pid {pid})")
    };

    println!();
    ctx.info(&format!("Service: {LAUNCHD_LABEL}"));
    ctx.info(&format!("State:   {state_display}"));
    println!();

    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_enable(ctx: &OutputContext) -> Result<()> {
    let (launchd, target) = direct_launchd_target()?;
    launchd.set_autostart(&target, true)?;
    if !launchd.is_loaded(&target)? {
        launchd.bootstrap(&plist_path()?)?;
    }
    ctx.success("Daemon service enabled for autostart");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_disable(ctx: &OutputContext) -> Result<()> {
    let (launchd, target) = direct_launchd_target()?;
    launchd.set_autostart(&target, false)?;
    launchd.bootout(&target)?;
    ctx.success("Daemon service disabled for autostart");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_logs(args: &LogsArgs, ctx: &OutputContext) -> Result<()> {
    let _ = ctx; // logs stream directly to stdout
    let log_path = macos_log_path()?;

    if args.since.is_some() {
        bail!("--since is not supported on macOS; use --lines or --follow");
    }

    if !std::path::Path::new(&log_path).exists() {
        bail!("Log file not found at {log_path}. Has the daemon been started?");
    }

    let mut cmd_args: Vec<String> = Vec::new();

    if args.follow {
        cmd_args.push("-f".to_string());
    }

    if let Some(n) = args.lines {
        cmd_args.push("-n".to_string());
        cmd_args.push(n.to_string());
    }

    cmd_args.push(log_path);

    let status = std::process::Command::new("tail")
        .args(&cmd_args)
        .status()
        .context("Failed to run tail")?;

    if !status.success() {
        bail!("tail exited with {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_path() -> Result<std::path::PathBuf> {
    let launch_agents = hypercolor_core::config::paths::macos_launch_agents_dir()
        .context("Could not determine home directory")?;
    let (_, target) = direct_launchd_target()?;
    Ok(hypercolor_macos_owner::launch_agent_plist(
        &launch_agents,
        &target,
    ))
}

#[cfg(target_os = "macos")]
fn direct_launchd_target() -> Result<(
    hypercolor_macos_owner::LaunchdAdapter,
    hypercolor_macos_owner::LaunchdTarget,
)> {
    let launchd = hypercolor_macos_owner::LaunchdAdapter::for_current_user()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let target = launchd.target(LAUNCHD_LABEL);
    Ok((launchd, target))
}

#[cfg(target_os = "macos")]
fn macos_log_path() -> Result<String> {
    let path = hypercolor_core::config::paths::macos_user_log_dir()
        .context("Could not determine home directory")?
        .join("hypercolor.log");
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn choose_owner_locally(
    requested_owner: hypercolor_macos_owner::MacosDaemonOwner,
) -> Result<hypercolor_macos_owner::MacosOwnerCoordinatorOutcome> {
    use hypercolor_core::config::paths::data_dir;
    use hypercolor_macos_owner::{
        MacosHandoverTransactionId, MacosOwnerStore, choose_daemon_owner,
    };

    let store = MacosOwnerStore::new(data_dir());
    let mut executor = CliOwnerExecutor::new(store.clone())?;
    let transaction_id = MacosHandoverTransactionId::new(format!(
        "cli-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))?;
    choose_daemon_owner(&store, &mut executor, requested_owner, transaction_id)
        .map_err(anyhow::Error::from)
}

#[cfg(target_os = "macos")]
struct CliOwnerExecutor {
    store: hypercolor_macos_owner::MacosOwnerStore,
    launchd: hypercolor_macos_owner::LaunchdAdapter,
    launch_agents: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
impl CliOwnerExecutor {
    fn new(store: hypercolor_macos_owner::MacosOwnerStore) -> Result<Self> {
        let launchd = hypercolor_macos_owner::LaunchdAdapter::for_current_user()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let launch_agents = hypercolor_core::config::paths::macos_launch_agents_dir()
            .context("failed to resolve the user home directory")?;
        Ok(Self {
            store,
            launchd,
            launch_agents,
        })
    }

    fn label(
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> Result<&'static str, hypercolor_macos_owner::MacosOwnerExecutionError> {
        use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerExecutionError};

        match owner {
            MacosDaemonOwner::AppSidecar => Ok(hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME),
            MacosDaemonOwner::DirectLaunchd => Ok(LAUNCHD_LABEL),
            MacosDaemonOwner::Homebrew => Ok(hypercolor_types::service::HOMEBREW_UNIT),
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no service label",
            )),
        }
    }

    fn plist(
        &self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> Result<std::path::PathBuf, hypercolor_macos_owner::MacosOwnerExecutionError> {
        let file_name = if owner == hypercolor_macos_owner::MacosDaemonOwner::AppSidecar {
            hypercolor_macos_owner::MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME.to_owned()
        } else {
            format!("{}.plist", Self::label(owner)?)
        };
        Ok(self.launch_agents.join(file_name))
    }

    fn target(
        &self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> Result<
        hypercolor_macos_owner::LaunchdTarget,
        hypercolor_macos_owner::MacosOwnerExecutionError,
    > {
        Ok(self.launchd.target(Self::label(owner)?))
    }

    /// Who may stop an owner, derived from its neutral identity; the same
    /// rule the app executor applies.
    fn stop_authority(
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> hypercolor_types::service::StopAuthority {
        use hypercolor_macos_owner::MacosDaemonOwner;
        use hypercolor_types::service::{ServiceIdentity, StopAuthority};

        StopAuthority::for_identity(&match owner {
            MacosDaemonOwner::AppSidecar => ServiceIdentity::APP_SIDECAR,
            MacosDaemonOwner::DirectLaunchd => ServiceIdentity::launchd_direct(),
            MacosDaemonOwner::Homebrew => ServiceIdentity::homebrew(),
            MacosDaemonOwner::Standalone => ServiceIdentity::STANDALONE,
        })
    }
}

#[cfg(target_os = "macos")]
impl hypercolor_macos_owner::MacosOwnerExecutor for CliOwnerExecutor {
    fn autostart_enabled(
        &mut self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> Result<bool, hypercolor_macos_owner::MacosOwnerExecutionError> {
        let plist = self.plist(owner)?;
        if !plist.is_file() {
            return Ok(false);
        }
        self.launchd.autostart_enabled(&self.target(owner)?)
    }

    fn set_autostart(
        &mut self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
        enabled: bool,
    ) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
        let app_plist = (owner == hypercolor_macos_owner::MacosDaemonOwner::AppSidecar)
            .then(|| self.plist(owner))
            .transpose()?;
        if enabled
            && let Some(path) = app_plist.as_ref()
            && !path.is_file()
        {
            install_app_sidecar_launch_agent(path)?;
        }
        self.launchd.set_autostart(&self.target(owner)?, enabled)?;
        if !enabled
            && let Some(path) = app_plist
            && path.is_file()
        {
            std::fs::remove_file(&path).map_err(|error| {
                hypercolor_macos_owner::MacosOwnerExecutionError::new(error.to_string())
            })?;
            std::fs::File::open(&self.launch_agents)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    hypercolor_macos_owner::MacosOwnerExecutionError::new(error.to_string())
                })?;
        }
        Ok(())
    }

    fn preflight_stop_authority(
        &mut self,
        incarnation: &hypercolor_macos_owner::MacosOwnerIncarnation,
    ) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
        use hypercolor_macos_owner::MacosOwnerExecutionError;

        use hypercolor_types::service::StopAuthority;

        match Self::stop_authority(incarnation.owner) {
            StopAuthority::SupervisedChild => Err(MacosOwnerExecutionError::new(
                "app-sidecar termination requires the app supervisor's retained child handle",
            )),
            StopAuthority::ServiceManager(_) => Ok(()),
            StopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
                "standalone termination requires its terminal user",
            )),
        }
    }

    fn flush_and_stop(
        &mut self,
        incarnation: &hypercolor_macos_owner::MacosOwnerIncarnation,
    ) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
        use hypercolor_macos_owner::MacosOwnerExecutionError;

        use hypercolor_types::service::{ServiceManager, StopAuthority};

        match Self::stop_authority(incarnation.owner) {
            StopAuthority::SupervisedChild => Err(MacosOwnerExecutionError::new(
                "app-sidecar termination requires the app supervisor's retained child handle",
            )),
            StopAuthority::ServiceManager(identity) => match identity.manager {
                Some(ServiceManager::Launchd) => {
                    self.launchd.stop(&self.target(incarnation.owner)?)
                }
                Some(ServiceManager::Homebrew) => {
                    let brew = homebrew_binary()?;
                    owner_run_command(&brew.to_string_lossy(), &["services", "stop", "hypercolor"])
                }
                Some(ServiceManager::Systemd | ServiceManager::WindowsScm) | None => {
                    Err(MacosOwnerExecutionError::new(format!(
                        "{identity} is not a macOS service manager"
                    )))
                }
            },
            StopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
                "standalone termination requires its terminal user",
            )),
        }
    }

    fn start(
        &mut self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
    ) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
        use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerExecutionError};

        match owner {
            MacosDaemonOwner::AppSidecar => owner_run_command(
                "/usr/bin/open",
                &["-a", "Hypercolor", "--args", "--minimized"],
            ),
            MacosDaemonOwner::DirectLaunchd => {
                let target = self.target(owner)?;
                let plist = self.plist(owner)?;
                self.launchd.start(&target, &plist)
            }
            MacosDaemonOwner::Homebrew => {
                let brew = homebrew_binary()?;
                owner_run_command(
                    &brew.to_string_lossy(),
                    &["services", "start", "hypercolor"],
                )
            }
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone cannot be selected by the CLI coordinator",
            )),
        }
    }

    fn wait_for_guard_release(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<bool, hypercolor_macos_owner::MacosOwnerExecutionError> {
        hypercolor_macos_owner::wait_for_macos_guard_release(
            timeout,
            &std::env::temp_dir()
                .join("hypercolor-daemon.lock")
                .to_string_lossy(),
        )
    }

    fn wait_for_owner(
        &mut self,
        owner: hypercolor_macos_owner::MacosDaemonOwner,
        after_epoch: u64,
        timeout: std::time::Duration,
    ) -> Result<bool, hypercolor_macos_owner::MacosOwnerExecutionError> {
        hypercolor_macos_owner::wait_for_owner_publication(&self.store, owner, after_epoch, timeout)
    }
}

#[cfg(target_os = "macos")]
fn homebrew_binary() -> Result<std::path::PathBuf, hypercolor_macos_owner::MacosOwnerExecutionError>
{
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| {
            hypercolor_macos_owner::MacosOwnerExecutionError::new(
                "Homebrew executable is unavailable",
            )
        })
}

#[cfg(target_os = "macos")]
fn install_app_sidecar_launch_agent(
    path: &std::path::Path,
) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let bundle = [
        std::path::PathBuf::from("/Applications/Hypercolor.app"),
        hypercolor_core::config::paths::macos_user_applications_dir()
            .unwrap_or_default()
            .join("Hypercolor.app"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
    .ok_or_else(|| {
        hypercolor_macos_owner::MacosOwnerExecutionError::new(
            "Hypercolor.app is not installed in a standard Applications directory",
        )
    })?;
    let executable = bundle.join(hypercolor_macos_owner::MACOS_APP_BUNDLE_EXECUTABLE_RELATIVE_PATH);
    if !bundle.is_absolute()
        || bundle
            .extension()
            .is_none_or(|extension| extension != "app")
    {
        return Err(hypercolor_macos_owner::MacosOwnerExecutionError::new(
            "Hypercolor.app resolved to an invalid bundle path",
        ));
    }
    if !executable.is_file() {
        return Err(hypercolor_macos_owner::MacosOwnerExecutionError::new(
            "Hypercolor.app does not contain its expected executable",
        ));
    }
    let executable = xml_escape(&executable.to_string_lossy());
    let contents = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{}</string>\n\
         <key>ProgramArguments</key><array><string>{executable}</string>\
         <string>--minimized</string></array>\n\
         <key>RunAtLoad</key><true/>\n\
         </dict></plist>\n",
        hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME,
    );
    let parent = path.parent().ok_or_else(|| {
        hypercolor_macos_owner::MacosOwnerExecutionError::new(
            "app-sidecar LaunchAgent has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        hypercolor_macos_owner::MacosOwnerExecutionError::new(error.to_string())
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        hypercolor_macos_owner::MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME,
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| {
            hypercolor_macos_owner::MacosOwnerExecutionError::new(error.to_string())
        })?;
    let result = (|| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(hypercolor_macos_owner::MacosOwnerExecutionError::new(
            error.to_string(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
fn owner_command_output(
    program: &str,
    args: &[&str],
) -> Result<std::process::Output, hypercolor_macos_owner::MacosOwnerExecutionError> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| hypercolor_macos_owner::MacosOwnerExecutionError::new(error.to_string()))
}

#[cfg(target_os = "macos")]
fn owner_run_command(
    program: &str,
    args: &[&str],
) -> Result<(), hypercolor_macos_owner::MacosOwnerExecutionError> {
    let output = owner_command_output(program, args)?;
    if output.status.success() {
        return Ok(());
    }
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr.truncate(4_096);
    Err(hypercolor_macos_owner::MacosOwnerExecutionError::new(
        format!("{program} failed with {}: {}", output.status, stderr.trim()),
    ))
}

// ── Unsupported platforms ───────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_start(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_stop(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_restart(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_status(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_enable(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_disable(_ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_logs(_args: &LogsArgs, _ctx: &OutputContext) -> Result<()> {
    bail!(
        "Service management is not supported on this platform. Supported: Linux (systemd), macOS (launchd)."
    )
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Format a byte count as a human-readable string (e.g., "45.2 MB").
#[cfg(target_os = "linux")]
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "byte counts in practical use are well within f64 precision"
)]
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// The macOS owner a neutral identity names on this host.
#[cfg(target_os = "macos")]
fn macos_owner_for_identity(
    identity: &hypercolor_types::service::ServiceIdentity,
) -> Result<hypercolor_macos_owner::MacosDaemonOwner> {
    use hypercolor_macos_owner::MacosDaemonOwner;
    use hypercolor_types::service::{DaemonRunMode, ServiceManager};

    match (identity.run_mode, identity.manager) {
        (DaemonRunMode::SupervisedChild, None) => Ok(MacosDaemonOwner::AppSidecar),
        (DaemonRunMode::Standalone, None) => Ok(MacosDaemonOwner::Standalone),
        (DaemonRunMode::UserService, Some(ServiceManager::Launchd)) => {
            Ok(MacosDaemonOwner::DirectLaunchd)
        }
        (DaemonRunMode::UserService, Some(ServiceManager::Homebrew)) => {
            Ok(MacosDaemonOwner::Homebrew)
        }
        _ => bail!("{identity} is not a macOS daemon topology"),
    }
}

#[cfg(test)]
mod tests {
    use clap::{Parser, ValueEnum};

    use super::MacosServiceOwner;
    use hypercolor_types::service::ServiceIdentity;

    #[test]
    fn macos_owner_values_use_stable_local_cli_names() {
        assert_eq!(
            MacosServiceOwner::from_str("app-sidecar", false),
            Ok(MacosServiceOwner::AppSidecar)
        );
        assert_eq!(
            MacosServiceOwner::from_str("direct-launchd", false),
            Ok(MacosServiceOwner::DirectLaunchd)
        );
        assert_eq!(
            MacosServiceOwner::from_str("homebrew", false),
            Ok(MacosServiceOwner::Homebrew)
        );
    }

    #[test]
    fn owner_choices_name_the_neutral_identities() {
        assert_eq!(
            MacosServiceOwner::AppSidecar.identity(),
            ServiceIdentity::APP_SIDECAR
        );
        assert_eq!(
            MacosServiceOwner::DirectLaunchd.identity(),
            ServiceIdentity::launchd_direct()
        );
        assert_eq!(
            MacosServiceOwner::Homebrew.identity(),
            ServiceIdentity::homebrew()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn foreign_identities_are_rejected_on_macos() {
        assert!(super::macos_owner_for_identity(&ServiceIdentity::systemd_user()).is_err());
        assert_eq!(
            super::macos_owner_for_identity(&ServiceIdentity::STANDALONE).expect("standalone maps"),
            hypercolor_macos_owner::MacosDaemonOwner::Standalone
        );
    }

    #[test]
    fn local_owner_choice_is_wired_into_the_service_command_tree() {
        let cli =
            crate::Cli::try_parse_from(["hypercolor", "service", "choose-owner", "direct-launchd"])
                .expect("local owner command should parse");
        assert!(matches!(
            cli.command,
            crate::Commands::Service(super::ServiceArgs {
                command: super::ServiceCommand::ChooseOwner(super::ChooseOwnerArgs {
                    owner: MacosServiceOwner::DirectLaunchd,
                }),
            })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_sidecar_service_identity_matches_tauri_artifacts() {
        use hypercolor_macos_owner::{
            MacosDaemonOwner, MacosOwnerExecutor, MacosOwnerIdentity, MacosOwnerStore,
        };

        let directory = tempfile::tempdir().expect("temporary directory should build");
        let executor = super::CliOwnerExecutor {
            store: MacosOwnerStore::new(directory.path()),
            launchd: hypercolor_macos_owner::LaunchdAdapter::with_uid("501"),
            launch_agents: directory.path().to_path_buf(),
        };
        assert_eq!(
            super::CliOwnerExecutor::label(MacosDaemonOwner::AppSidecar)
                .expect("app label should resolve"),
            hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME
        );
        assert_eq!(
            executor
                .plist(MacosDaemonOwner::AppSidecar)
                .expect("app plist should resolve")
                .file_name()
                .and_then(std::ffi::OsStr::to_str),
            Some(hypercolor_macos_owner::MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME)
        );
        assert_eq!(
            super::CliOwnerExecutor::stop_authority(MacosDaemonOwner::AppSidecar),
            hypercolor_types::service::StopAuthority::SupervisedChild
        );

        let record = executor
            .store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                MacosOwnerIdentity::new(
                    "audit-sidecar",
                    "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                    "requirement-sidecar",
                    4_242,
                )
                .expect("identity should build"),
            )
            .expect("owner should publish");
        let mut executor = executor;
        let preflight_error = executor
            .preflight_stop_authority(&record.incarnation())
            .expect_err("CLI must reject app-owned stop authority before handover");
        assert!(
            preflight_error
                .to_string()
                .contains("retained child handle")
        );
        let error = executor
            .flush_and_stop(&record.incarnation())
            .expect_err("CLI must not stop an app-owned child");
        assert!(error.to_string().contains("retained child handle"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cli_external_owner_stops_use_only_exact_launcher_identities() {
        use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerStore};

        use hypercolor_types::service::{ServiceIdentity, StopAuthority};

        let directory = tempfile::tempdir().expect("temporary directory should build");
        let executor = super::CliOwnerExecutor {
            store: MacosOwnerStore::new(directory.path()),
            launchd: hypercolor_macos_owner::LaunchdAdapter::with_uid("501"),
            launch_agents: directory.path().to_path_buf(),
        };
        assert_eq!(
            executor
                .target(MacosDaemonOwner::DirectLaunchd)
                .expect("launchd target should resolve")
                .service(),
            "gui/501/tech.hyperbliss.hypercolor"
        );
        assert_eq!(
            super::CliOwnerExecutor::stop_authority(MacosDaemonOwner::DirectLaunchd),
            StopAuthority::ServiceManager(ServiceIdentity::launchd_direct())
        );
        assert_eq!(
            super::CliOwnerExecutor::stop_authority(MacosDaemonOwner::Homebrew),
            StopAuthority::ServiceManager(ServiceIdentity::homebrew())
        );
        assert_eq!(
            super::CliOwnerExecutor::stop_authority(MacosDaemonOwner::Standalone),
            StopAuthority::UserDirected
        );
        let _ = executor.store;
    }
}
