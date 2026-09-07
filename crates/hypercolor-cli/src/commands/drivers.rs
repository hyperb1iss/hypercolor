//! `hyper drivers` -- driver module inventory and controls.

use anyhow::Result;
use clap::{Args, Subcommand};
use hypercolor_types::api::controls::InvokeControlActionRequest;
use hypercolor_types::api::drivers::{DriverListResponse, DriverSummary};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlSurfaceDocument,
};
use hypercolor_types::device::{
    DriverModuleKind, DriverTransportAvailability, DriverTransportDescriptor, DriverTransportKind,
};

use crate::client::DaemonClient;
use crate::commands::controls;
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// Driver module inventory and dynamic controls.
#[derive(Debug, Args)]
pub struct DriversArgs {
    #[command(subcommand)]
    pub command: DriverCommand,
}

/// Driver subcommands.
#[derive(Debug, Subcommand)]
pub enum DriverCommand {
    /// List registered driver modules.
    List,
    /// Show one driver-level control surface.
    Controls(DriverControlsArgs),
    /// Apply one driver-level control value.
    SetControl(DriverSetControlArgs),
    /// Invoke one driver-level control action.
    Action(DriverActionArgs),
}

/// Arguments for `drivers controls`.
#[derive(Debug, Args)]
pub struct DriverControlsArgs {
    /// Driver ID.
    pub driver: String,
}

/// Arguments for `drivers set-control`.
#[derive(Debug, Args)]
pub struct DriverSetControlArgs {
    /// Driver ID.
    pub driver: String,

    /// Field ID.
    pub field: String,

    /// Typed value. Examples: `enum:ddp`, `bool:true`, `ip:10.0.0.2`.
    pub value: String,
}

/// Arguments for `drivers action`.
#[derive(Debug, Args)]
pub struct DriverActionArgs {
    /// Driver ID.
    pub driver: String,

    /// Action ID.
    pub action: String,

    /// Action input assignment, repeatable.
    #[arg(long = "input", short = 'i')]
    pub input: Vec<String>,

    /// Confirm actions that declare confirmation metadata.
    #[arg(long)]
    pub yes: bool,
}

/// Execute the `drivers` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, the driver does not expose
/// the requested control surface, or a typed value cannot be parsed.
pub async fn execute(args: &DriversArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        DriverCommand::List => execute_list(client, ctx).await,
        DriverCommand::Controls(args) => execute_controls(args, client, ctx).await,
        DriverCommand::SetControl(args) => execute_set_control(args, client, ctx).await,
        DriverCommand::Action(args) => execute_action(args, client, ctx).await,
    }
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: DriverListResponse = client.get_list("/drivers").await?;
    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for driver in &response.items {
                println!("{}", driver.descriptor.id);
            }
        }
        OutputFormat::Table => {
            let rows = response
                .items
                .iter()
                .map(|driver| driver_row(driver, ctx))
                .collect::<Vec<_>>();
            ctx.print_table(
                &["Driver", "Kind", "Transports", "Enabled", "Controls"],
                &rows,
            );
        }
    }
    Ok(())
}

async fn execute_controls(
    args: &DriverControlsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let response: ControlSurfaceDocument = client
        .get(&format!("/drivers/{}/controls", urlencoded(&args.driver)))
        .await?;
    controls::render_surface(&response, ctx)
}

async fn execute_set_control(
    args: &DriverSetControlArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface_id = driver_control_surface_id(client, &args.driver).await?;
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
    args: &DriverActionArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface = driver_control_surface(client, &args.driver).await?;
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

fn driver_row(driver: &DriverSummary, ctx: &OutputContext) -> Vec<String> {
    vec![
        ctx.painter.name(&driver.presentation.label),
        ctx.painter
            .muted(module_kind_label(driver.descriptor.module_kind)),
        ctx.painter.muted(&transport_summary(driver)),
        ctx.painter.muted(if driver.enabled { "yes" } else { "no" }),
        ctx.painter
            .muted(driver.control_surface_id.as_deref().unwrap_or("-")),
    ]
}

async fn driver_control_surface_id(client: &DaemonClient, driver: &str) -> Result<String> {
    let surface = driver_control_surface(client, driver).await?;
    Ok(surface.surface_id)
}

async fn driver_control_surface(
    client: &DaemonClient,
    driver: &str,
) -> Result<ControlSurfaceDocument> {
    client
        .get(&format!("/drivers/{}/controls", urlencoded(driver)))
        .await
}

const fn module_kind_label(kind: DriverModuleKind) -> &'static str {
    match kind {
        DriverModuleKind::Network => "network",
        DriverModuleKind::Hal => "hal",
        DriverModuleKind::Bridge => "bridge",
        DriverModuleKind::Host => "host",
        DriverModuleKind::Virtual => "virtual",
    }
}

fn transport_summary(driver: &DriverSummary) -> String {
    let transports = driver
        .descriptor
        .transports
        .iter()
        .map(transport_label)
        .collect::<Vec<_>>();
    if transports.is_empty() {
        "-".to_string()
    } else {
        transports.join(", ")
    }
}

fn transport_label(transport: &DriverTransportDescriptor) -> String {
    let label = match &transport.kind {
        DriverTransportKind::Network => "network".to_owned(),
        DriverTransportKind::Usb => "usb".to_owned(),
        DriverTransportKind::Smbus => "smbus".to_owned(),
        DriverTransportKind::Midi => "midi".to_owned(),
        DriverTransportKind::Serial => "serial".to_owned(),
        DriverTransportKind::Virtual => "virtual".to_owned(),
        DriverTransportKind::Bridge => "bridge".to_owned(),
        DriverTransportKind::Custom(kind) => kind.clone(),
    };

    match &transport.availability {
        DriverTransportAvailability::Available => label,
        DriverTransportAvailability::UnsupportedPlatform { platform } => {
            format!("{label} (not available on {platform})")
        }
    }
}
