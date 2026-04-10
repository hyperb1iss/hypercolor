//! `hyper layouts` -- spatial layout management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

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
    /// Create a new spatial layout.
    Create(LayoutCreateArgs),
    /// Delete a spatial layout.
    Delete(LayoutDeleteArgs),
    /// Show the currently active layout.
    Active,
    /// Apply a layout (make it active).
    Apply(LayoutApplyArgs),
    /// Preview a layout without making it active.
    Preview(LayoutPreviewArgs),
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

/// Arguments for `layouts create`.
#[derive(Debug, Args)]
pub struct LayoutCreateArgs {
    /// Name for the new layout.
    #[arg(long)]
    pub name: String,

    /// JSON file or inline JSON with layout definition.
    #[arg(long)]
    pub data: String,
}

/// Arguments for `layouts delete`.
#[derive(Debug, Args)]
pub struct LayoutDeleteArgs {
    /// Layout name or ID.
    pub name: String,
}

/// Arguments for `layouts apply`.
#[derive(Debug, Args)]
pub struct LayoutApplyArgs {
    /// Layout name or ID.
    pub name: String,
}

/// Arguments for `layouts preview`.
#[derive(Debug, Args)]
pub struct LayoutPreviewArgs {
    /// Layout name or ID.
    pub name: String,
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
        LayoutCommand::Create(create_args) => execute_create(create_args, client, ctx).await,
        LayoutCommand::Delete(delete_args) => execute_delete(delete_args, client, ctx).await,
        LayoutCommand::Active => execute_active(client, ctx).await,
        LayoutCommand::Apply(apply_args) => execute_apply(apply_args, client, ctx).await,
        LayoutCommand::Preview(preview_args) => execute_preview(preview_args, client, ctx).await,
    }
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/layouts").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(layouts) = response.get("items").and_then(serde_json::Value::as_array) {
                for layout in layouts {
                    if let Some(name) = layout.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(layouts) = response.get("items").and_then(serde_json::Value::as_array) {
                let headers = ["ID", "Layout", "Canvas", "Zones"];
                let rows: Vec<Vec<String>> = layouts
                    .iter()
                    .map(|l| {
                        vec![
                            extract_str(l, "id"),
                            extract_str(l, "name"),
                            format!(
                                "{}x{}",
                                l.get("canvas_width")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0),
                                l.get("canvas_height")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0)
                            ),
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
            ctx.info(&format!("ID         {}", extract_str(&response, "id")));
            let width = response
                .get("canvas_width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let height = response
                .get("canvas_height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            ctx.info(&format!("Canvas     {width}x{height}"));
            ctx.info(&format!(
                "Zones      {}",
                response
                    .get("zone_count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            ));
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

async fn execute_create(
    args: &LayoutCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut body: serde_json::Value =
        serde_json::from_str(&args.data).map_err(|e| anyhow::anyhow!("Invalid JSON data: {e}"))?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("name".to_string(), serde_json::Value::String(args.name.clone()));
    }
    let response = client.post("/layouts", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Layout created: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_delete(
    args: &LayoutDeleteArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/layouts/{}", urlencoded(&args.name));
    let response = client.delete(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Layout deleted: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_active(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/layouts/active").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => println!("{}", extract_str(&response, "name")),
        OutputFormat::Table => {
            ctx.info(&format!(
                "Active layout: {}",
                extract_str(&response, "name")
            ));
        }
    }

    Ok(())
}

async fn execute_apply(
    args: &LayoutApplyArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/layouts/{}/apply", urlencoded(&args.name));
    let response = client.post(&path, &serde_json::json!({})).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Layout applied: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_preview(
    args: &LayoutPreviewArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/layouts/{}", urlencoded(&args.name));
    let layout_data = client.get(&path).await?;
    let response = client.put("/layouts/active/preview", &layout_data).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Previewing layout: {}", args.name));
        }
    }

    Ok(())
}
