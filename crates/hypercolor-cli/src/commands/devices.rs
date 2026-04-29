//! `hyper devices` -- device discovery, inspection, and management.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::client::DaemonClient;
use crate::commands::controls;
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
    /// Scan for new RGB devices across discovery targets.
    Discover(DeviceDiscoverArgs),
    /// Pair a network device and store credentials.
    Pair(DevicePairArgs),
    /// Show detailed information about a device.
    Info(DeviceInfoArgs),
    /// Flash a test pattern on a device for identification.
    Identify(DeviceIdentifyArgs),
    /// Set a device to a specific color.
    SetColor(DeviceSetColorArgs),
    /// Show one device-level control surface.
    Controls(DeviceControlsArgs),
    /// Apply one device-level control value.
    SetControl(DeviceSetControlArgs),
    /// Invoke one device-level control action.
    Action(DeviceActionArgs),
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
    /// Scan specific discovery targets only (repeatable: wled, usb, hue).
    #[arg(long)]
    pub target: Vec<String>,

    /// Discovery timeout in seconds.
    #[arg(long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `devices pair`.
#[derive(Debug, Args)]
pub struct DevicePairArgs {
    /// Device name or ID.
    pub device: String,

    /// Store credentials but skip immediate activation.
    #[arg(long)]
    pub no_activate: bool,
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

/// Arguments for `devices controls`.
#[derive(Debug, Args)]
pub struct DeviceControlsArgs {
    /// Device name or ID.
    pub device: String,
}

/// Arguments for `devices set-control`.
#[derive(Debug, Args)]
pub struct DeviceSetControlArgs {
    /// Device ID.
    pub device: String,

    /// Field ID.
    pub field: String,

    /// Typed value. Examples: `enum:grb`, `bool:true`, `duration:1500`.
    pub value: String,

    /// Expected surface revision for optimistic concurrency.
    #[arg(long)]
    pub expected_revision: Option<u64>,

    /// Validate the transaction without applying it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for `devices action`.
#[derive(Debug, Args)]
pub struct DeviceActionArgs {
    /// Device ID.
    pub device: String,

    /// Action ID.
    pub action: String,

    /// Action input assignment, repeatable.
    #[arg(long = "input", short = 'i')]
    pub input: Vec<String>,

    /// Confirm actions that declare confirmation metadata.
    #[arg(long)]
    pub yes: bool,
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
        DeviceCommand::Controls(controls_args) => {
            execute_controls(controls_args, client, ctx).await
        }
        DeviceCommand::SetControl(control_args) => {
            execute_set_control(control_args, client, ctx).await
        }
        DeviceCommand::Action(action_args) => execute_action(action_args, client, ctx).await,
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
                            ctx.painter.name(&extract_str(d, "name")),
                            ctx.painter.muted(&extract_str(d, "backend")),
                            ctx.painter.number(
                                &d.get("total_leds")
                                    .and_then(serde_json::Value::as_u64)
                                    .map_or_else(|| "?".to_string(), |l| l.to_string()),
                            ),
                            ctx.painter.device_state(&extract_str(d, "status")),
                            ctx.painter.muted(
                                d.get("firmware_version")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or("-"),
                            ),
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
                    ctx.painter.number(&devices.len().to_string()),
                    ctx.painter.number(&total_leds.to_string()),
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
    let path = format!("/devices/{}/pair", urlencoded(&args.device));
    let response = client
        .post(
            &path,
            &serde_json::json!({ "activate_after_pair": !args.no_activate }),
        )
        .await?;
    render_pair_response(&args.device, &response, ctx)?;
    Ok(())
}

async fn execute_discover(
    args: &DeviceDiscoverArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "targets": args.target,
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

async fn execute_controls(
    args: &DeviceControlsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let response = client
        .get(&format!(
            "/control-surfaces?device_id={}",
            urlencoded(&args.device)
        ))
        .await?;
    controls::render_surface_list(&response, ctx)
}

async fn execute_set_control(
    args: &DeviceSetControlArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface_id = device_control_surface_id_for_field(client, &args.device, &args.field).await?;
    let assignment = format!("{}={}", args.field, args.value);
    let changes = controls::assignments_to_changes(&[assignment])?;
    let mut body = json!({
        "surface_id": surface_id,
        "changes": changes,
        "dry_run": args.dry_run,
    });
    if let Some(revision) = args.expected_revision {
        body["expected_revision"] = json!(revision);
    }

    let response = client
        .patch(
            &format!("/control-surfaces/{}/values", urlencoded(&surface_id)),
            &body,
        )
        .await?;
    controls::render_apply_response(&response, ctx)
}

async fn execute_action(
    args: &DeviceActionArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface = device_control_surface_for_action(client, &args.device, &args.action).await?;
    controls::ensure_action_confirmed(&surface, &args.action, args.yes, ctx)?;
    let surface_id = extract_str(&surface, "surface_id");
    let input = controls::assignments_to_map(&args.input)?;
    let response = client
        .post(
            &format!(
                "/control-surfaces/{}/actions/{}",
                urlencoded(&surface_id),
                urlencoded(&args.action)
            ),
            &json!({ "input": input }),
        )
        .await?;
    controls::render_action_response(&response, ctx)
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

fn render_pair_response(
    target_label: &str,
    response: &serde_json::Value,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let status = response
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let message = response
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Pairing request completed.");

            if matches!(status, "paired" | "already_paired") {
                ctx.success(message);
            } else {
                ctx.info(&format!("{target_label}: {message}"));
            }
        }
    }

    Ok(())
}

async fn device_control_surface_id_for_field(
    client: &DaemonClient,
    device: &str,
    field: &str,
) -> Result<String> {
    let surfaces = device_control_surfaces(client, device).await?;
    find_surface_with_item(&surfaces, "fields", field).ok_or_else(|| {
        let available = available_surface_items(&surfaces, "fields");
        anyhow::anyhow!(
            "Device control field '{field}' was not found on {device}. Available fields: {available}"
        )
    })
}

async fn device_control_surface_for_action(
    client: &DaemonClient,
    device: &str,
    action: &str,
) -> Result<Value> {
    let surfaces = device_control_surfaces(client, device).await?;
    find_surface_value_with_item(&surfaces, "actions", action).ok_or_else(|| {
        let available = available_surface_items(&surfaces, "actions");
        anyhow::anyhow!(
            "Device control action '{action}' was not found on {device}. Available actions: {available}"
        )
    })
}

async fn device_control_surfaces(client: &DaemonClient, device: &str) -> Result<Vec<Value>> {
    let response = client
        .get(&format!(
            "/control-surfaces?device_id={}",
            urlencoded(device)
        ))
        .await?;
    let surfaces = response
        .get("surfaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if surfaces.is_empty() {
        bail!("Device {device} does not expose control surfaces");
    }
    Ok(surfaces)
}

fn find_surface_with_item(surfaces: &[Value], collection: &str, item_id: &str) -> Option<String> {
    find_surface_value_with_item(surfaces, collection, item_id)
        .as_ref()
        .and_then(|surface| surface.get("surface_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn find_surface_value_with_item(
    surfaces: &[Value],
    collection: &str,
    item_id: &str,
) -> Option<Value> {
    surfaces.iter().find_map(|surface| {
        surface
            .get(collection)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|item| item.get("id").and_then(Value::as_str) == Some(item_id))
            .then(|| surface.clone())
    })
}

fn available_surface_items(surfaces: &[Value], collection: &str) -> String {
    let items = surfaces
        .iter()
        .flat_map(|surface| {
            surface
                .get(collection)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}
