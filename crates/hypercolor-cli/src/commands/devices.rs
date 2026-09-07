//! `hyper devices` -- device discovery, inspection, and management.

use std::collections::HashMap;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use hypercolor_types::api::controls::{ControlSurfaceListResponse, InvokeControlActionRequest};
use hypercolor_types::api::devices::{
    DeviceListResponse, DeviceSummary, DiscoverRequest, DiscoverResponse, IdentifyDeviceResponse,
    IdentifyRequest, PairDeviceResponse,
};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlSurfaceDocument,
};
use hypercolor_types::device::{DeviceOrigin, DriverTransportKind};
use hypercolor_types::pairing::{PairDeviceRequest, PairDeviceStatus};

use crate::client::DaemonClient;
use crate::commands::controls;
use crate::output::{OutputContext, OutputFormat, urlencoded};

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

    /// Filter by output backend route.
    #[arg(long)]
    pub backend_id: Option<String>,

    /// Filter by owning driver module.
    #[arg(long)]
    pub driver: Option<String>,
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
    if let Some(backend_id) = &args.backend_id {
        query_parts.push(format!("backend_id={}", urlencoded(backend_id)));
    }
    if let Some(driver) = &args.driver {
        query_parts.push(format!("driver={}", urlencoded(driver)));
    }
    if !query_parts.is_empty() {
        path = format!("{path}?{}", query_parts.join("&"));
    }

    let response: DeviceListResponse = client.get_list(&path).await?;
    let devices = &response.items;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for device in devices {
                println!("{}", device.name);
            }
        }
        OutputFormat::Table => {
            let headers = ["Device", "Driver", "Route", "LEDs", "Status", "Firmware"];
            let rows: Vec<Vec<String>> = devices
                .iter()
                .map(|d| {
                    vec![
                        ctx.painter.name(&d.name),
                        ctx.painter.muted(&d.origin.driver_id),
                        ctx.painter.muted(&d.origin.backend_id),
                        ctx.painter.number(&d.total_leds.to_string()),
                        ctx.painter.device_state(&d.status),
                        ctx.painter
                            .muted(d.firmware_version.as_deref().unwrap_or("-")),
                    ]
                })
                .collect();

            ctx.print_table(&headers, &rows);
            println!();
            let total_leds: u64 = devices.iter().map(|d| u64::from(d.total_leds)).sum();
            ctx.info(&format!(
                "{} devices \u{00b7} {} LEDs",
                ctx.painter.number(&devices.len().to_string()),
                ctx.painter.number(&total_leds.to_string()),
            ));
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
    let response: PairDeviceResponse = client
        .post(
            &path,
            &PairDeviceRequest {
                values: HashMap::new(),
                activate_after_pair: !args.no_activate,
            },
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
    let body = DiscoverRequest {
        targets: Some(args.target.clone()),
        timeout_ms: Some(u64::from(args.timeout).saturating_mul(1000)),
        wait: None,
    };

    ctx.info("Discovering devices...");
    let response: DiscoverResponse = client.post("/devices/discover", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let (status, scan_id) = match &response {
                DiscoverResponse::Scanning { scan_id, .. } => ("scanning", scan_id),
                DiscoverResponse::Completed { scan_id, .. } => ("completed", scan_id),
            };
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
    let response: DeviceSummary = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", response.name);
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&response.name);
            println!();
            ctx.info(&format!("Driver       {}", response.origin.driver_id));
            ctx.info(&format!("Route        {}", response.origin.backend_id));
            ctx.info(&format!(
                "Transport    {}",
                transport_label(&response.origin)
            ));
            ctx.info(&format!("LED Count    {}", response.total_leds));
            ctx.info(&format!("Status       {}", response.status));
            if let Some(fw) = &response.firmware_version {
                ctx.info(&format!("Firmware     {fw}"));
            }
            println!();
        }
    }

    Ok(())
}

/// Wire spelling of the transport a device speaks.
fn transport_label(origin: &DeviceOrigin) -> String {
    match &origin.transport {
        DriverTransportKind::Network => "network".to_owned(),
        DriverTransportKind::Usb => "usb".to_owned(),
        DriverTransportKind::Smbus => "smbus".to_owned(),
        DriverTransportKind::Midi => "midi".to_owned(),
        DriverTransportKind::Serial => "serial".to_owned(),
        DriverTransportKind::Virtual => "virtual".to_owned(),
        DriverTransportKind::Bridge => "bridge".to_owned(),
        DriverTransportKind::Custom(kind) => kind.clone(),
    }
}

async fn execute_controls(
    args: &DeviceControlsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let response: ControlSurfaceListResponse = client
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
    let body = PatchControlsRequest {
        values: controls::assignments_to_values(&[assignment])?,
        clear_bindings: Vec::new(),
    };

    let response: ApplyControlChangesResponse = client
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
    let surface_id = surface.surface_id.clone();
    let input = controls::assignments_to_map(&args.input)?;
    let response: ControlActionResult = client
        .post(
            &format!(
                "/control-surfaces/{}/actions/{}",
                urlencoded(&surface_id),
                urlencoded(&args.action)
            ),
            &InvokeControlActionRequest { input },
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
    let body = IdentifyRequest {
        duration_ms: Some(u64::from(args.duration).saturating_mul(1000)),
        color: None,
    };
    let response: IdentifyDeviceResponse = client.post(&path, &body).await?;

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

fn render_pair_response(
    target_label: &str,
    response: &PairDeviceResponse,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            if matches!(
                response.status,
                PairDeviceStatus::Paired | PairDeviceStatus::AlreadyPaired
            ) {
                ctx.success(&response.message);
            } else {
                ctx.info(&format!("{target_label}: {}", response.message));
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
    surfaces
        .iter()
        .find(|surface| surface.fields.iter().any(|item| item.id == field))
        .map(|surface| surface.surface_id.clone())
        .ok_or_else(|| {
            let available = available_field_ids(&surfaces);
            anyhow::anyhow!(
                "Device control field '{field}' was not found on {device}. Available fields: {available}"
            )
        })
}

async fn device_control_surface_for_action(
    client: &DaemonClient,
    device: &str,
    action: &str,
) -> Result<ControlSurfaceDocument> {
    let surfaces = device_control_surfaces(client, device).await?;
    surfaces
        .iter()
        .find(|surface| surface.actions.iter().any(|item| item.id == action))
        .cloned()
        .ok_or_else(|| {
            let available = available_action_ids(&surfaces);
            anyhow::anyhow!(
                "Device control action '{action}' was not found on {device}. Available actions: {available}"
            )
        })
}

async fn device_control_surfaces(
    client: &DaemonClient,
    device: &str,
) -> Result<Vec<ControlSurfaceDocument>> {
    let response: ControlSurfaceListResponse = client
        .get(&format!(
            "/control-surfaces?device_id={}",
            urlencoded(device)
        ))
        .await?;
    if response.surfaces.is_empty() {
        bail!("Device {device} does not expose control surfaces");
    }
    Ok(response.surfaces)
}

fn available_field_ids(surfaces: &[ControlSurfaceDocument]) -> String {
    join_ids(
        surfaces
            .iter()
            .flat_map(|surface| surface.fields.iter().map(|field| field.id.as_str())),
    )
}

fn available_action_ids(surfaces: &[ControlSurfaceDocument]) -> String {
    join_ids(
        surfaces
            .iter()
            .flat_map(|surface| surface.actions.iter().map(|action| action.id.as_str())),
    )
}

fn join_ids<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    let ids = ids.collect::<Vec<_>>();
    if ids.is_empty() {
        "none".to_owned()
    } else {
        ids.join(", ")
    }
}
