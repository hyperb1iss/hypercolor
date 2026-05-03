//! `hyper service` — daemon service lifecycle management.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};

use crate::output::OutputContext;

// ── Constants ───────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "hypercolor";

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "tech.hyperbliss.hypercolor";

#[cfg(target_os = "macos")]
const PLIST_FILENAME: &str = "tech.hyperbliss.hypercolor.plist";

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
    }
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
                "ActiveEnterTimestamp" => {
                    if !value.is_empty() {
                        since = value.to_string();
                    }
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
    let plist_path = plist_path()?;
    run_launchctl(&["load", &plist_path])?;
    ctx.success("Daemon service started");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_stop(ctx: &OutputContext) -> Result<()> {
    let plist_path = plist_path()?;
    run_launchctl(&["unload", &plist_path])?;
    ctx.success("Daemon service stopped");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_restart(ctx: &OutputContext) -> Result<()> {
    let plist_path = plist_path()?;
    // Ignore errors on stop — service may not be running
    let _ = run_launchctl(&["unload", &plist_path]);
    run_launchctl(&["load", &plist_path])?;
    ctx.success("Daemon service restarted");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_status(ctx: &OutputContext) -> Result<()> {
    let uid = get_uid()?;
    let domain_target = format!("gui/{uid}/{LAUNCHD_LABEL}");

    let output = std::process::Command::new("launchctl")
        .args(["print", &domain_target])
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
    let plist_path = plist_path()?;
    run_launchctl(&["load", &plist_path])?;
    ctx.success("Daemon service enabled for autostart");
    Ok(())
}

#[cfg(target_os = "macos")]
#[expect(clippy::unused_async, reason = "async signature required by dispatch")]
async fn execute_disable(ctx: &OutputContext) -> Result<()> {
    let plist_path = plist_path()?;
    run_launchctl(&["unload", &plist_path])?;
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
fn plist_path() -> Result<String> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let path = home
        .join("Library")
        .join("LaunchAgents")
        .join(PLIST_FILENAME);
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn macos_log_path() -> Result<String> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let path = home
        .join("Library")
        .join("Logs")
        .join("hypercolor")
        .join("hypercolor.log");
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("launchctl")
        .args(args)
        .output()
        .context("Failed to run launchctl")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("launchctl {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn get_uid() -> Result<String> {
    let output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to run `id -u`")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
