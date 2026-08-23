//! `hyper server` -- daemon identity and health.

use anyhow::Result;
use clap::{Args, Subcommand};

use hypercolor_types::api::system::{HealthResponse, SystemResource};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

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
    let system: SystemResource = client.get("/system").await?;
    let identity = &system.identity;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(identity)?,
        OutputFormat::Plain => {
            println!("{}", identity.version);
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Version    {}", identity.version));
            ctx.info(&format!("Name       {}", identity.instance_name));
            ctx.info(&format!("Devices    {}", identity.device_count));
            ctx.info(&format!(
                "Auth       {}",
                if identity.auth_required {
                    "required"
                } else {
                    "open"
                }
            ));
            if let Some(status) = &system.status
                && !status.capabilities.is_empty()
            {
                ctx.info(&format!("Features   {}", status.capabilities.join(", ")));
            }
            println!();
        }
    }

    Ok(())
}

async fn execute_health(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: HealthResponse = client.get_unversioned("/health").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let styled = ctx.painter.device_state(&response.status);
            ctx.success(&format!("Daemon is {styled}"));
        }
    }

    Ok(())
}
