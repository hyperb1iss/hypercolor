//! `hyper devices` -- device discovery, inspection, and management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

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
    /// Pair a network device and store credentials.
    Pair(DevicePairArgs),
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
    /// Scan specific backends only (repeatable: wled, hid, hue).
    #[arg(long)]
    pub backend: Vec<String>,

    /// Discovery timeout in seconds.
    #[arg(long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `devices pair`.
#[derive(Debug, Args)]
pub struct DevicePairArgs {
    #[command(subcommand)]
    pub backend: DevicePairBackend,
}

/// Pairable network backends.
#[derive(Debug, Subcommand)]
pub enum DevicePairBackend {
    /// Pair a Philips Hue bridge.
    Hue(DevicePairHueArgs),
    /// Pair a Nanoleaf panel controller.
    Nanoleaf(DevicePairNanoleafArgs),
}

/// Arguments for `devices pair hue`.
#[derive(Debug, Args)]
pub struct DevicePairHueArgs {
    /// Hue bridge IP address. Defaults to the first discovered Hue bridge.
    #[arg(long)]
    pub bridge_ip: Option<String>,
}

/// Arguments for `devices pair nanoleaf`.
#[derive(Debug, Args)]
pub struct DevicePairNanoleafArgs {
    /// Nanoleaf device IP address. Defaults to the first discovered Nanoleaf device.
    #[arg(long)]
    pub device_ip: Option<String>,
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
        DeviceCommand::Pair(pair_args) => execute_pair(pair_args, client, ctx).await,
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
        query_parts.push(format!("status={}", urlencoded(status)));
    }
    if let Some(backend) = &args.backend {
        query_parts.push(format!("backend={}", urlencoded(backend)));
    }
    if !query_parts.is_empty() {
        path = format!("{path}?{}", query_parts.join("&"));
    }

    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(devices) = response.get("items").and_then(serde_json::Value::as_array) {
                for device in devices {
                    if let Some(name) = device.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(devices) = response.get("items").and_then(serde_json::Value::as_array) {
                let headers = ["Device", "Backend", "LEDs", "Status", "Firmware"];
                let rows: Vec<Vec<String>> = devices
                    .iter()
                    .map(|d| {
                        vec![
                            extract_str(d, "name"),
                            extract_str(d, "backend"),
                            d.get("total_leds")
                                .and_then(serde_json::Value::as_u64)
                                .map_or_else(|| "?".to_string(), |l| l.to_string()),
                            extract_str(d, "status"),
                            d.get("firmware_version")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("-")
                                .to_string(),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                let total_leds: u64 = devices
                    .iter()
                    .filter_map(|d| d.get("total_leds").and_then(serde_json::Value::as_u64))
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

async fn execute_pair(
    args: &DevicePairArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.backend {
        DevicePairBackend::Hue(hue_args) => execute_pair_hue(hue_args, client, ctx).await,
        DevicePairBackend::Nanoleaf(nanoleaf_args) => {
            execute_pair_nanoleaf(nanoleaf_args, client, ctx).await
        }
    }
}

async fn execute_pair_hue(
    args: &DevicePairHueArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let bridge_ip =
        resolve_pair_ip(client, "hue", args.bridge_ip.as_deref(), "--bridge-ip").await?;
    let response = client
        .post(
            "/devices/pair/hue",
            &serde_json::json!({ "bridge_ip": bridge_ip }),
        )
        .await?;
    render_pair_response("Hue bridge", &response, ctx)?;
    Ok(())
}

async fn execute_pair_nanoleaf(
    args: &DevicePairNanoleafArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let device_ip =
        resolve_pair_ip(client, "nanoleaf", args.device_ip.as_deref(), "--device-ip").await?;
    let response = client
        .post(
            "/devices/pair/nanoleaf",
            &serde_json::json!({ "device_ip": device_ip }),
        )
        .await?;
    render_pair_response("Nanoleaf device", &response, ctx)?;
    Ok(())
}

async fn execute_discover(
    args: &DeviceDiscoverArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "backends": args.backend,
        "timeout_ms": args.timeout.saturating_mul(1000),
    });

    ctx.info("Discovering devices...");
    let response = client.post("/devices/discover", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let scan_id = response
                .get("scan_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("scan_unknown");
            let status = response
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("queued");
            ctx.success(&format!("Discovery {status}: {scan_id}"));
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
                "Backend      {}",
                extract_str(&response, "backend")
            ));
            ctx.info(&format!(
                "LED Count    {}",
                response
                    .get("total_leds")
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
    let body = serde_json::json!({ "duration_ms": args.duration.saturating_mul(1000) });
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
    let path = format!("/devices/{}", urlencoded(&args.device));
    let body = serde_json::json!({ "color": args.color });
    let response = client.put(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Set {} to {}", args.device, args.color));
        }
    }

    Ok(())
}

async fn resolve_pair_ip(
    client: &DaemonClient,
    backend: &str,
    provided_ip: Option<&str>,
    flag_name: &str,
) -> Result<String> {
    if let Some(ip) = provided_ip {
        return Ok(ip.to_owned());
    }

    let response = client
        .get(&format!("/devices?backend={}", urlencoded(backend)))
        .await?;
    let items = response
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Daemon returned an invalid device list"))?;

    if let Some(ip) = items.iter().find_map(|item| {
        item.get("network_ip")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    }) {
        return Ok(ip);
    }

    anyhow::bail!(
        "No discovered {backend} devices expose a network IP. Run `hyper devices discover --backend {backend}` or pass {flag_name}."
    );
}

fn render_pair_response(
    target_label: &str,
    response: &serde_json::Value,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            match response.get("status").and_then(serde_json::Value::as_str) {
                Some("paired") => {
                    let name = response
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(target_label);
                    let device_key = response
                        .get("device_key")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("stored");
                    ctx.success(&format!("Paired {name} ({device_key})"));
                }
                Some("press_button") => {
                    ctx.info("Press the Hue bridge link button, then rerun the pair command.");
                }
                Some("hold_power") => {
                    ctx.info(
                    "Hold the Nanoleaf power button for 5-7 seconds, then rerun the pair command.",
                );
                }
                Some(status) => {
                    ctx.info(&format!("{target_label} pairing status: {status}"));
                }
                None => {
                    ctx.info(&format!("{target_label} pairing request completed."));
                }
            }
        }
    }

    Ok(())
}
