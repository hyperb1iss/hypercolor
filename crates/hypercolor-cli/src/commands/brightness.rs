//! `hyper brightness` -- global output brightness control.

use anyhow::Result;
use clap::{Args, Subcommand};
use hypercolor_types::api::output::{OutputPatchRequest, OutputResource};

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
    let response: OutputResource = client.get("/output").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            println!("{}", brightness_percent(f64::from(response.brightness)));
        }
    }

    Ok(())
}

async fn execute_set(
    args: &BrightnessSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let percent = args.value.min(100);
    let body = OutputPatchRequest {
        power: None,
        brightness: Some(f32::from(u16::try_from(percent).unwrap_or(100)) / 100.0),
    };
    let response: OutputResource = client.patch("/output", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Brightness set to {}%", args.value.min(100)));
        }
    }

    Ok(())
}

/// Render the wire's `0.0..=1.0` brightness as the 0-100 percentage
/// this command has always spoken.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to the unit interval before scaling"
)]
fn brightness_percent(brightness: f64) -> u8 {
    (brightness.clamp(0.0, 1.0) * 100.0).round() as u8
}
