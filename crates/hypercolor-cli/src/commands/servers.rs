//! `hyper servers` -- discover Hypercolor daemons on the local network.

use std::time::Duration;

use anyhow::Result;
use clap::{Args, Subcommand};
use hypercolor_core::device::discover_servers;

use crate::output::{OutputContext, OutputFormat};

/// Network server discovery commands.
#[derive(Debug, Args)]
pub struct ServersArgs {
    #[command(subcommand)]
    pub command: ServersCommand,
}

/// `hyper servers` subcommands.
#[derive(Debug, Subcommand)]
pub enum ServersCommand {
    /// Discover Hypercolor daemons advertised via mDNS.
    Discover(DiscoverArgs),
}

/// Browse the local network for Hypercolor daemons.
#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// Discovery timeout in seconds.
    #[arg(long, default_value = "3")]
    pub timeout: f64,
}

/// Execute the `servers` subcommand.
pub async fn execute(args: &ServersArgs, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ServersCommand::Discover(discover) => discover_command(discover, ctx).await,
    }
}

async fn discover_command(args: &DiscoverArgs, ctx: &OutputContext) -> Result<()> {
    let timeout = Duration::from_secs_f64(args.timeout.max(0.1));
    let servers = discover_servers(timeout).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&serde_json::to_value(&servers)?)?,
        OutputFormat::Plain => {
            for server in &servers {
                println!(
                    "{}\t{}:{}",
                    server.identity.instance_name, server.host, server.port
                );
            }
        }
        OutputFormat::Table => {
            let rows = servers
                .iter()
                .map(|server| {
                    vec![
                        server.identity.instance_name.clone(),
                        format!("{}:{}", server.host, server.port),
                        server.identity.version.clone(),
                        server
                            .device_count
                            .map_or_else(|| "?".to_owned(), |count| count.to_string()),
                        if server.auth_required {
                            "api_key".to_owned()
                        } else {
                            "none".to_owned()
                        },
                    ]
                })
                .collect::<Vec<_>>();
            ctx.print_table(&["Name", "Host", "Version", "Devices", "Auth"], &rows);
        }
    }

    Ok(())
}
