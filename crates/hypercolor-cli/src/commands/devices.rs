//! `hyper devices` -- device discovery, inspection, and management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Device discovery and management.
#[derive(Debug, Args)]
pub struct DevicesArgs {
    #[command(subcommand)]
    pub command: DeviceCommand,
}

/// Device subcommands.
#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    /// List connected and discovered devices.
    List(DeviceListArgs),
    /// Scan for new RGB devices across all backends.
    Discover(DeviceDiscoverArgs),
    /// Show detailed information about a device.
    Info(DeviceInfoArgs),
    /// Flash a test pattern on a device for identification.
    Identify(DeviceIdentifyArgs),
    /// Set a device to a specific color.
    SetColor(DeviceSetColorArgs),
}

/// Arguments for `devices list`.
#[derive(Debug, Args)]
pub struct DeviceListArgs {
    /// Filter by connection status.
    #[arg(long)]
    pub status: Option<String>,

    /// Filter by backend/protocol.
    #[arg(long)]
    pub backend: Option<String>,
}

/// Arguments for `devices discover`.
#[derive(Debug, Args)]
pub struct DeviceDiscoverArgs {
    /// Scan specific backends only (repeatable: wled, openrgb, hid, hue).
    #[arg(long)]
    pub backend: Vec<String>,

    /// Discovery timeout in seconds.
    #[arg(long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `devices info`.
#[derive(Debug, Args)]
pub struct DeviceInfoArgs {
    /// Device name or ID.
    pub device: String,
}

/// Arguments for `devices identify`.
#[derive(Debug, Args)]
pub struct DeviceIdentifyArgs {
    /// Device name or ID.
    pub device: String,

    /// Flash duration in seconds.
    #[arg(long, default_value = "5")]
    pub duration: u32,
}

/// Arguments for `devices set-color`.
#[derive(Debug, Args)]
pub struct DeviceSetColorArgs {
    /// Device name or ID.
    pub device: String,

    /// Color to set (hex: #ff00ff or name: cyan).
    pub color: String,
}

/// Execute the `devices` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the requested device
/// is not found.
pub async fn execute(args: &DevicesArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        DeviceCommand::List(list_args) => execute_list(list_args, client, ctx).await,
        DeviceCommand::Discover(discover_args) => {
            execute_discover(discover_args, client, ctx).await
        }
        DeviceCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
        DeviceCommand::Identify(identify_args) => {
            execute_identify(identify_args, client, ctx).await
        }
        DeviceCommand::SetColor(color_args) => execute_set_color(color_args, client, ctx).await,
    }
}

async fn execute_list(
    args: &DeviceListArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut path = "/devices".to_string();
    let mut query_parts = Vec::new();

    if let Some(status) = &args.status {
        query_parts.push(format!("status={status}"));
    }
    if let Some(backend) = &args.backend {
        query_parts.push(format!("backend={backend}"));
    }
    if !query_parts.is_empty() {
        path = format!("{path}?{}", query_parts.join("&"));
    }

    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(devices) = response.as_array() {
                for device in devices {
                    if let Some(name) = device.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(devices) = response.as_array() {
                let headers = ["Device", "Protocol", "LEDs", "Status", "Latency"];
                let rows: Vec<Vec<String>> = devices
                    .iter()
                    .map(|d| {
                        vec![
                            extract_str(d, "name"),
                            extract_str(d, "protocol"),
                            d.get("leds")
                                .and_then(serde_json::Value::as_u64)
                                .map_or_else(|| "?".to_string(), |l| l.to_string()),
                            extract_str(d, "status"),
                            d.get("latency_ms")
                                .and_then(serde_json::Value::as_f64)
                                .map_or_else(|| "?".to_string(), |l| format!("{l:.1}ms")),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                let total_leds: u64 = devices
                    .iter()
                    .filter_map(|d| d.get("leds").and_then(serde_json::Value::as_u64))
                    .sum();
                ctx.info(&format!(
                    "{} devices \u{00b7} {} LEDs",
                    devices.len(),
                    total_leds
                ));
            }
        }
    }

    Ok(())
}

async fn execute_discover(
    args: &DeviceDiscoverArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "backends": args.backend,
        "timeout": args.timeout,
    });

    ctx.info("Discovering devices...");
    let response = client.post("/devices/discover", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            if let Some(found) = response.get("found").and_then(serde_json::Value::as_u64) {
                ctx.success(&format!("{found} devices found"));
            }
        }
    }

    Ok(())
}

async fn execute_info(
    args: &DeviceInfoArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/devices/{}", urlencoded(&args.device));
    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "name"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&extract_str(&response, "name"));
            println!();
            ctx.info(&format!(
                "Protocol     {}",
                extract_str(&response, "protocol")
            ));
            ctx.info(&format!(
                "LED Count    {}",
                response
                    .get("led_count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            ));
            ctx.info(&format!(
                "Status       {}",
                extract_str(&response, "status")
            ));
            if let Some(fw) = response
                .get("firmware_version")
                .and_then(serde_json::Value::as_str)
            {
                ctx.info(&format!("Firmware     {fw}"));
            }
            println!();
        }
    }

    Ok(())
}

async fn execute_identify(
    args: &DeviceIdentifyArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/devices/{}/identify", urlencoded(&args.device));
    let body = serde_json::json!({ "duration": args.duration });
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!(
                "Identifying {} for {}s",
                args.device, args.duration
            ));
        }
    }

    Ok(())
}

async fn execute_set_color(
    args: &DeviceSetColorArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/devices/{}/color", urlencoded(&args.device));
    let body = serde_json::json!({ "color": args.color });
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Set {} to {}", args.device, args.color));
        }
    }

    Ok(())
}

/// Extract a string field from a JSON value, returning "?" if missing.
fn extract_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string()
}

/// Simple percent-encoding for URL path segments.
fn urlencoded(s: &str) -> String {
    s.replace(' ', "%20")
}
