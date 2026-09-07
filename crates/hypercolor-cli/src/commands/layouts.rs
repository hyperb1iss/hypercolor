//! `hyper layouts` -- spatial layout management.

use anyhow::Result;
use clap::{Args, Subcommand};

use hypercolor_types::api::layouts::{
    ApplyLayoutResponse, DeleteLayoutResponse, LayoutListResponse, LayoutSummary,
    PreviewLayoutResponse,
};
use hypercolor_types::spatial::SpatialLayout;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, urlencoded};

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
    let response: LayoutListResponse = client.get_list("/layouts").await?;
    let layouts = &response.items;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for layout in layouts {
                println!("{}", layout.name);
            }
        }
        OutputFormat::Table => {
            let headers = ["ID", "Layout", "Canvas", "Zones"];
            let rows: Vec<Vec<String>> = layouts
                .iter()
                .map(|l| {
                    vec![
                        ctx.painter.id(&l.id),
                        ctx.painter.name(&l.name),
                        ctx.painter
                            .number(&format!("{}x{}", l.canvas_width, l.canvas_height)),
                        ctx.painter.number(&l.zone_count.to_string()),
                    ]
                })
                .collect();

            ctx.print_table(&headers, &rows);
            println!();
            ctx.info(&format!(
                "{} layouts",
                ctx.painter.number(&layouts.len().to_string())
            ));
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
    let response: SpatialLayout = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", response.name);
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Layout: {}", response.name));
            println!();
            ctx.info(&format!("ID         {}", response.id));
            ctx.info(&format!(
                "Canvas     {}x{}",
                response.canvas_width, response.canvas_height
            ));
            ctx.info(&format!("Zones      {}", response.zones.len()));
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
    let response: LayoutSummary = client.put(&path, &body).await?;

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
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(args.name.clone()),
        );
    }
    let response: LayoutSummary = client.post("/layouts", &body).await?;

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
    let response: DeleteLayoutResponse = client.delete(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Layout deleted: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_active(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: SpatialLayout = client.get("/layouts/active").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => println!("{}", response.name),
        OutputFormat::Table => {
            ctx.info(&format!("Active layout: {}", response.name));
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
    let response: ApplyLayoutResponse = client.post(&path, &serde_json::json!({})).await?;

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
    let layout: SpatialLayout = client.get(&path).await?;
    let response: PreviewLayoutResponse = client.put("/layouts/active/preview", &layout).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Previewing layout: {}", args.name));
        }
    }

    Ok(())
}
