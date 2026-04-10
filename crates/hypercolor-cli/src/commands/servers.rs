//! `hyper servers` -- discover Hypercolor daemons on the local network.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use hypercolor_core::device::discover_servers;

use crate::config::{self, Profile};
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
    /// Save a discovered server as a connection profile.
    Adopt(AdoptArgs),
}

/// Browse the local network for Hypercolor daemons.
#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// Discovery timeout in seconds.
    #[arg(long, default_value = "3")]
    pub timeout: f64,
}

/// Save a discovered mDNS server as a connection profile.
#[derive(Debug, Args)]
pub struct AdoptArgs {
    /// Instance name from a previous `servers discover` run.
    pub instance: String,

    /// Profile name to save as (defaults to the instance name).
    #[arg(long, alias = "as")]
    pub name: Option<String>,

    /// Discovery timeout in seconds for locating the server.
    #[arg(long, default_value = "5")]
    pub timeout: f64,
}

/// Execute the `servers` subcommand.
pub async fn execute(args: &ServersArgs, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ServersCommand::Discover(discover) => discover_command(discover, ctx).await,
        ServersCommand::Adopt(adopt) => adopt_command(adopt, ctx).await,
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

async fn adopt_command(args: &AdoptArgs, ctx: &OutputContext) -> Result<()> {
    let timeout = Duration::from_secs_f64(args.timeout.max(0.1));
    let servers = discover_servers(timeout).await?;

    let server = servers
        .iter()
        .find(|s| s.identity.instance_name == args.instance)
        .with_context(|| {
            format!(
                "no server named {:?} found (run `hyper servers discover` to list available instances)",
                args.instance
            )
        })?;

    let profile_name = args
        .name
        .clone()
        .unwrap_or_else(|| slug_from_name(&server.identity.instance_name));

    let mut cfg = config::load()?;

    if cfg.profiles.contains_key(&profile_name) {
        anyhow::bail!(
            "profile {profile_name:?} already exists \
             (use `hyper config profile set {profile_name} host ...` to update, \
             or `--name` to adopt under a different name)"
        );
    }

    cfg.profiles.insert(
        profile_name.clone(),
        Profile {
            host: server.host.to_string(),
            port: server.port,
            api_key: None,
            label: Some(format!(
                "{} (v{})",
                server.identity.instance_name, server.identity.version
            )),
            description: None,
        },
    );

    config::save(&cfg)?;
    let host_str = server.host.to_string();
    ctx.success(&format!(
        "Adopted {:?} as profile {:?} ({host_str}:{})",
        server.identity.instance_name, profile_name, server.port
    ));
    Ok(())
}

fn slug_from_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
