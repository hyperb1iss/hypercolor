//! `hyper status` -- display current system state.

use anyhow::Result;
use clap::Args;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Show current system state: running effect, devices, FPS, audio capture.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Live-updating status (re-renders on state change).
    #[arg(long)]
    pub watch: bool,

    /// Update interval for --watch mode in seconds.
    #[arg(long, default_value = "1")]
    pub interval: f64,
}

/// Execute the `status` subcommand.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable.
pub async fn execute(args: &StatusArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    if args.watch {
        let interval = args.interval.max(0.2);
        loop {
            let response = client.get("/status").await?;
            render_status(&response, ctx)?;

            let sleep = tokio::time::sleep(std::time::Duration::from_secs_f64(interval));
            tokio::pin!(sleep);
            tokio::select! {
                () = &mut sleep => {}
                _ = tokio::signal::ctrl_c() => {
                    if !ctx.quiet {
                        println!();
                        ctx.info("Stopped status watch.");
                    }
                    break;
                }
            }
        }
        return Ok(());
    }

    let response = client.get("/status").await?;
    render_status(&response, ctx)?;

    Ok(())
}

fn render_status(data: &serde_json::Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(data)?,
        OutputFormat::Plain => {
            let effect = data
                .get("active_effect")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Off");
            println!("{effect}");
        }
        OutputFormat::Table => print_status_table(data, ctx),
    }
    Ok(())
}

/// Render the human-readable status display.
fn print_status_table(data: &serde_json::Value, ctx: &OutputContext) {
    let running = data
        .get("running")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let daemon_status = if running { "running" } else { "stopped" };
    let version = data
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let uptime = data
        .get("uptime_seconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let fps_tier = data
        .get("render_loop")
        .and_then(|r| r.get("fps_tier"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let total_frames = data
        .get("render_loop")
        .and_then(|r| r.get("total_frames"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let effect_name = data
        .get("active_effect")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Off");
    let device_count = data
        .get("device_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let effect_count = data
        .get("effect_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let status_dot = if ctx.color {
        if daemon_status == "running" {
            "\x1b[38;2;80;250;123m\u{25cf}\x1b[0m"
        } else {
            "\x1b[38;2;255;99;99m\u{25cf}\x1b[0m"
        }
    } else if daemon_status == "running" {
        "(*)"
    } else {
        "(x)"
    };

    println!();
    ctx.info(&format!(
        "Daemon     {status_dot} {daemon_status:<16} {version}"
    ));
    ctx.info(&format!("Uptime     {uptime}s"));
    ctx.info(&format!("Render     tier={fps_tier} frames={total_frames}"));
    ctx.info(&format!("Effect     {effect_name}"));
    ctx.info(&format!("Catalog    {effect_count} effects"));
    ctx.info(&format!("Devices    {device_count} tracked"));

    println!();
}
