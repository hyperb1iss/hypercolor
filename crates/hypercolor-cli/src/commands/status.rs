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
        ctx.warning("Watch mode requires a running daemon with WebSocket support.");
        ctx.info(&format!(
            "Would poll every {:.1}s (not yet implemented)",
            args.interval
        ));
    }

    let response = client.get("/status").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            // Quiet / plain: just the effect name
            if let Some(effect) = response
                .get("effect")
                .and_then(|e| e.get("name"))
                .and_then(serde_json::Value::as_str)
            {
                println!("{effect}");
            } else {
                println!("Off");
            }
        }
        OutputFormat::Table => {
            print_status_table(&response, ctx);
        }
    }

    Ok(())
}

/// Render the human-readable status display.
fn print_status_table(data: &serde_json::Value, ctx: &OutputContext) {
    let daemon_status = data
        .get("daemon")
        .and_then(|d| d.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    let pid = data
        .get("daemon")
        .and_then(|d| d.get("pid"))
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "?".to_string(), |p| p.to_string());

    let fps = data
        .get("engine")
        .and_then(|e| e.get("fps"))
        .and_then(serde_json::Value::as_f64)
        .map_or_else(|| "?".to_string(), |f| format!("{f:.1}"));

    let effect_name = data
        .get("effect")
        .and_then(|e| e.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("None");

    let total_leds = data
        .get("total_leds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    let device_count = data
        .get("devices")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);

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
        "Daemon     {status_dot} {daemon_status:<16} pid {pid}"
    ));
    ctx.info(&format!("FPS        {fps}"));
    ctx.info(&format!("Effect     {effect_name}"));
    ctx.info(&format!(
        "Devices    {device_count} connected ({total_leds} LEDs)"
    ));

    // Device list
    if let Some(devices) = data.get("devices").and_then(serde_json::Value::as_array) {
        println!();
        let headers = ["Device", "Protocol", "LEDs", "Status", "Latency"];
        let rows: Vec<Vec<String>> = devices
            .iter()
            .map(|d| {
                vec![
                    d.get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    d.get("protocol")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    d.get("leds")
                        .and_then(serde_json::Value::as_u64)
                        .map_or_else(|| "?".to_string(), |l| l.to_string()),
                    d.get("status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    d.get("latency_ms")
                        .and_then(serde_json::Value::as_f64)
                        .map_or_else(|| "?".to_string(), |l| format!("{l:.1}ms")),
                ]
            })
            .collect();
        ctx.print_table(&headers, &rows);
    }

    println!();
}
