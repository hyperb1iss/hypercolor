//! `hyper drivers` -- driver module inventory and controls.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::client::DaemonClient;
use crate::commands::controls;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

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

    /// Expected surface revision for optimistic concurrency.
    #[arg(long)]
    pub expected_revision: Option<u64>,

    /// Validate the transaction without applying it.
    #[arg(long)]
    pub dry_run: bool,
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
    let response = client.get("/drivers").await?;
    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for driver in response
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                println!("{}", descriptor_field(driver, "id"));
            }
        }
        OutputFormat::Table => {
            let rows = response
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
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
    let response = client
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
    args: &DriverActionArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface_id = driver_control_surface_id(client, &args.driver).await?;
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

fn driver_row(driver: &Value, ctx: &OutputContext) -> Vec<String> {
    vec![
        ctx.painter.name(&presentation_label(driver)),
        ctx.painter.muted(&descriptor_field(driver, "module_kind")),
        ctx.painter.muted(&transport_summary(driver)),
        ctx.painter
            .muted(if driver_enabled(driver) { "yes" } else { "no" }),
        ctx.painter.muted(
            driver
                .get("control_surface_id")
                .and_then(Value::as_str)
                .unwrap_or("-"),
        ),
    ]
}

async fn driver_control_surface_id(client: &DaemonClient, driver: &str) -> Result<String> {
    let surface = client
        .get(&format!("/drivers/{}/controls", urlencoded(driver)))
        .await?;
    Ok(extract_str(&surface, "surface_id"))
}

fn descriptor_field(driver: &Value, key: &str) -> String {
    driver.get("descriptor").map_or_else(
        || "?".to_string(),
        |descriptor| extract_str(descriptor, key),
    )
}

fn presentation_label(driver: &Value) -> String {
    driver
        .get("presentation")
        .and_then(|presentation| presentation.get("label"))
        .and_then(Value::as_str)
        .map_or_else(
            || descriptor_field(driver, "display_name"),
            ToOwned::to_owned,
        )
}

fn transport_summary(driver: &Value) -> String {
    driver
        .get("descriptor")
        .and_then(|descriptor| descriptor.get("transports"))
        .and_then(Value::as_array)
        .map(|transports| {
            transports
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn driver_enabled(driver: &Value) -> bool {
    driver
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
