//! `hyper brightness` -- global output brightness control.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Global output brightness control.
#[derive(Debug, Args)]
pub struct BrightnessArgs {
    #[command(subcommand)]
    pub command: BrightnessCommand,
}

/// Brightness subcommands.
#[derive(Debug, Subcommand)]
pub enum BrightnessCommand {
    /// Get the current global brightness level.
    Get,
    /// Set the global brightness level (0-100).
    Set(BrightnessSetArgs),
}

/// Arguments for `brightness set`.
#[derive(Debug, Args)]
pub struct BrightnessSetArgs {
    /// Brightness level (0-100).
    pub value: u32,
}

pub async fn execute(
    args: &BrightnessArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        BrightnessCommand::Get => execute_get(client, ctx).await,
        BrightnessCommand::Set(set_args) => execute_set(set_args, client, ctx).await,
    }
}

async fn execute_get(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/settings/brightness").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let value = response
                .get("brightness")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            println!("{value}");
        }
    }

    Ok(())
}

async fn execute_set(
    args: &BrightnessSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({ "brightness": args.value.min(100) });
    let response = client.put("/settings/brightness", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Brightness set to {}%", args.value.min(100)));
        }
    }

    Ok(())
}
