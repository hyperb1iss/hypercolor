//! `hyper layouts` -- spatial layout management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Spatial layout management.
#[derive(Debug, Args)]
pub struct LayoutsArgs {
    #[command(subcommand)]
    pub command: LayoutCommand,
}

/// Layout subcommands.
#[derive(Debug, Subcommand)]
pub enum LayoutCommand {
    /// List configured spatial layouts.
    List,
    /// Show details of a specific layout.
    Show(LayoutShowArgs),
    /// Update a layout configuration.
    Update(LayoutUpdateArgs),
}

/// Arguments for `layouts show`.
#[derive(Debug, Args)]
pub struct LayoutShowArgs {
    /// Layout name or ID.
    pub name: String,
}

/// Arguments for `layouts update`.
#[derive(Debug, Args)]
pub struct LayoutUpdateArgs {
    /// Layout name or ID.
    pub name: String,

    /// JSON payload with layout updates.
    #[arg(long)]
    pub data: String,
}

/// Execute the `layouts` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the layout is not found.
pub async fn execute(args: &LayoutsArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        LayoutCommand::List => execute_list(client, ctx).await,
        LayoutCommand::Show(show_args) => execute_show(show_args, client, ctx).await,
        LayoutCommand::Update(update_args) => execute_update(update_args, client, ctx).await,
    }
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/layouts").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(layouts) = response.as_array() {
                for layout in layouts {
                    if let Some(name) = layout.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(layouts) = response.as_array() {
                let headers = ["Layout", "Devices", "Zones"];
                let rows: Vec<Vec<String>> = layouts
                    .iter()
                    .map(|l| {
                        vec![
                            extract_str(l, "name"),
                            l.get("device_count")
                                .and_then(serde_json::Value::as_u64)
                                .map_or_else(|| "?".to_string(), |c| c.to_string()),
                            l.get("zone_count")
                                .and_then(serde_json::Value::as_u64)
                                .map_or_else(|| "?".to_string(), |c| c.to_string()),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                ctx.info(&format!("{} layouts", layouts.len()));
            }
        }
    }

    Ok(())
}

async fn execute_show(
    args: &LayoutShowArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/layouts/{}", urlencoded(&args.name));
    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "name"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Layout: {}", extract_str(&response, "name")));
            println!();

            if let Some(zones) = response.get("zones").and_then(serde_json::Value::as_array) {
                let headers = ["Zone", "Position", "Size"];
                let rows: Vec<Vec<String>> = zones
                    .iter()
                    .map(|z| {
                        vec![
                            extract_str(z, "name"),
                            extract_str(z, "position"),
                            extract_str(z, "size"),
                        ]
                    })
                    .collect();
                ctx.print_table(&headers, &rows);
            }
            println!();
        }
    }

    Ok(())
}

async fn execute_update(
    args: &LayoutUpdateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/layouts/{}", urlencoded(&args.name));
    let body: serde_json::Value =
        serde_json::from_str(&args.data).map_err(|e| anyhow::anyhow!("Invalid JSON data: {e}"))?;
    let response = client.put(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Layout updated: {}", args.name));
        }
    }

    Ok(())
}

fn extract_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string()
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "%20")
}
