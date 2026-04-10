//! `hyper server` -- daemon identity and health.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str};

/// Daemon identity and health commands.
#[derive(Debug, Args)]
pub struct ServerArgs {
    #[command(subcommand)]
    pub command: ServerCommand,
}

/// Server subcommands.
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Show daemon version, identity, and capabilities.
    Info,
    /// Run a quick health check.
    Health,
}

pub async fn execute(args: &ServerArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ServerCommand::Info => execute_info(client, ctx).await,
        ServerCommand::Health => execute_health(client, ctx).await,
    }
}

async fn execute_info(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/server").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "version"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Version    {}", extract_str(&response, "version")));
            ctx.info(&format!("Name       {}", extract_str(&response, "name")));
            if let Some(features) = response
                .get("features")
                .and_then(serde_json::Value::as_array)
            {
                let feature_list: Vec<&str> = features
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect();
                ctx.info(&format!("Features   {}", feature_list.join(", ")));
            }
            println!();
        }
    }

    Ok(())
}

async fn execute_health(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/health").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let status = response
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let styled = ctx.painter.device_state(status);
            ctx.success(&format!("Daemon is {styled}"));
        }
    }

    Ok(())
}
