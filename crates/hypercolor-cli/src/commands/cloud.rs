//! `hyper cloud` -- Hypercolor Cloud account and daemon-link controls.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str};

#[derive(Debug, Args)]
pub struct CloudArgs {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Debug, Subcommand)]
pub enum CloudCommand {
    /// Show daemon cloud feature/configuration status.
    Status,
    /// Create or show this daemon's cloud identity.
    Identity,
}

pub async fn execute(args: &CloudArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        CloudCommand::Status => execute_status(client, ctx).await,
        CloudCommand::Identity => execute_identity(client, ctx).await,
    }
}

async fn execute_status(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/cloud/status").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            let enabled = response
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!("{}", if enabled { "enabled" } else { "disabled" });
        }
        OutputFormat::Table => {
            let enabled = response
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let state = if enabled {
                ctx.painter.success("enabled")
            } else {
                ctx.painter.muted("disabled")
            };
            println!();
            ctx.info(&format!("Cloud       {state}"));
            ctx.info(&format!(
                "API         {}",
                ctx.painter.keyword(&extract_str(&response, "base_url"))
            ));
            ctx.info(&format!(
                "Auth        {}",
                ctx.painter
                    .keyword(&extract_str(&response, "auth_base_url"))
            ));
            ctx.info(&format!(
                "App         {}",
                ctx.painter.keyword(&extract_str(&response, "app_base_url"))
            ));
            ctx.info(&format!(
                "Identity    {}",
                extract_str(&response, "identity_storage")
            ));
            println!();
        }
    }

    Ok(())
}

async fn execute_identity(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client
        .post("/cloud/identity", &serde_json::json!({}))
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "daemon_id"));
        }
        OutputFormat::Table => {
            println!();
            ctx.success("Cloud identity ready");
            ctx.info(&format!(
                "Daemon ID   {}",
                ctx.painter.id(&extract_str(&response, "daemon_id"))
            ));
            ctx.info(&format!(
                "Public key  {}",
                extract_str(&response, "identity_pubkey")
            ));
            println!();
        }
    }

    Ok(())
}
