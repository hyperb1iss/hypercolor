//! `hyper config` -- configuration management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Configuration management.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show the complete current configuration.
    Show,
    /// Get a config value by dotted key path.
    #[command(alias = "get")]
    Set(ConfigSetArgs),
    /// Reset config to defaults (or a specific key).
    Reset(ConfigResetArgs),
    /// Print the config file path.
    Path,
}

/// Arguments for `config set`.
#[derive(Debug, Args)]
pub struct ConfigSetArgs {
    /// Dotted key path (e.g., daemon.fps, audio.gain).
    pub key: String,

    /// New value (omit to get current value).
    pub value: Option<String>,

    /// Apply change to running daemon immediately (hot-reload).
    #[arg(long)]
    pub live: bool,
}

/// Arguments for `config reset`.
#[derive(Debug, Args)]
pub struct ConfigResetArgs {
    /// Reset specific key only (omit for full reset).
    pub key: Option<String>,

    /// Skip confirmation for full reset.
    #[arg(long)]
    pub yes: bool,
}

/// Execute the `config` subcommand tree.
///
/// # Errors
///
/// Returns an error if the config file cannot be read or the daemon is unreachable
/// for live updates.
pub async fn execute(args: &ConfigArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ConfigCommand::Show => execute_show(client, ctx).await,
        ConfigCommand::Set(set_args) => execute_set(set_args, client, ctx).await,
        ConfigCommand::Reset(reset_args) => execute_reset(reset_args, client, ctx).await,
        ConfigCommand::Path => execute_path(ctx),
    }
}

async fn execute_show(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/config").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            // Pretty-print the config as indented JSON (readable for humans too)
            let formatted = serde_json::to_string_pretty(&response)?;
            println!("{formatted}");
        }
    }

    Ok(())
}

async fn execute_set(
    args: &ConfigSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if let Some(value) = &args.value {
        // Set operation
        let body = serde_json::json!({
            "key": args.key,
            "value": value,
            "live": args.live,
        });
        let response = client.post("/config/set", &body).await?;

        match ctx.format {
            OutputFormat::Json => ctx.print_json(&response)?,
            OutputFormat::Plain | OutputFormat::Table => {
                let applied = if args.live {
                    "  (applied to running daemon)"
                } else {
                    ""
                };
                ctx.success(&format!("{}: {value}{applied}", args.key));
            }
        }
    } else {
        // Get operation
        let path = format!("/config/get?key={}", &args.key);
        let response = client.get(&path).await?;

        match ctx.format {
            OutputFormat::Json => ctx.print_json(&response)?,
            OutputFormat::Plain | OutputFormat::Table => {
                if let Some(val) = response.get("value") {
                    match val {
                        serde_json::Value::String(s) => println!("{s}"),
                        other => println!("{other}"),
                    }
                }
            }
        }
    }

    Ok(())
}

async fn execute_reset(
    args: &ConfigResetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if args.key.is_none() && !args.yes && !ctx.quiet {
        ctx.warning("Use --yes to confirm full config reset to defaults");
        return Ok(());
    }

    let body = serde_json::json!({
        "key": args.key,
    });
    let response = client.post("/config/reset", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            if let Some(key) = &args.key {
                ctx.success(&format!("Reset {key} to default"));
            } else {
                ctx.success("Config reset to defaults");
            }
        }
    }

    Ok(())
}

fn execute_path(ctx: &OutputContext) -> Result<()> {
    let config_path = config_file_path();
    match ctx.format {
        OutputFormat::Json => {
            ctx.print_json(&serde_json::json!({ "path": config_path }))?;
        }
        OutputFormat::Plain | OutputFormat::Table => {
            println!("{config_path}");
        }
    }
    Ok(())
}

/// Resolve the config file path using the same logic as the daemon.
fn config_file_path() -> String {
    if let Ok(path) = std::env::var("HYPERCOLOR_CONFIG") {
        return path;
    }

    dirs::config_dir().map_or_else(
        || "~/.config/hypercolor/config.toml".to_string(),
        |d| {
            d.join("hypercolor")
                .join("config.toml")
                .to_string_lossy()
                .into_owned()
        },
    )
}
