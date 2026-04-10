//! `hyper config` -- configuration management (daemon config + CLI profiles).

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::config::{self, Profile};
use crate::output::{OutputContext, OutputFormat, urlencoded};

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
    Get(ConfigGetArgs),
    /// Set a config value by dotted key path.
    Set(ConfigSetArgs),
    /// Reset config to defaults (or a specific key).
    Reset(ConfigResetArgs),
    /// Print the config file path.
    Path,
    /// Manage CLI connection profiles.
    #[command(name = "profile")]
    Profile(ProfileArgs),
}

/// Arguments for `config profile`.
#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

/// Profile management subcommands.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List all saved connection profiles.
    List,
    /// Show a profile's settings (active profile if omitted).
    Show(ProfileShowArgs),
    /// Add a new connection profile.
    Add(ProfileAddArgs),
    /// Update a field in an existing profile.
    Set(ProfileSetArgs),
    /// Remove a saved profile.
    Remove(ProfileRemoveArgs),
    /// Set the default connection profile.
    Default(ProfileDefaultArgs),
}

/// Arguments for `config profile show`.
#[derive(Debug, Args)]
pub struct ProfileShowArgs {
    /// Profile name (shows active profile if omitted).
    pub name: Option<String>,
}

/// Arguments for `config profile add`.
#[derive(Debug, Args)]
pub struct ProfileAddArgs {
    /// Profile name.
    pub name: String,
    /// Daemon host.
    #[arg(long, default_value = "localhost")]
    pub host: String,
    /// Daemon port.
    #[arg(long, default_value_t = 9420)]
    pub port: u16,
    /// API key for authentication.
    #[arg(long)]
    pub api_key: Option<String>,
    /// Human-readable label.
    #[arg(long)]
    pub label: Option<String>,
}

/// Arguments for `config profile set`.
#[derive(Debug, Args)]
pub struct ProfileSetArgs {
    /// Profile name.
    pub name: String,
    /// Field to update (host, port, api_key, label, description).
    pub key: String,
    /// New value.
    pub value: String,
}

/// Arguments for `config profile remove`.
#[derive(Debug, Args)]
pub struct ProfileRemoveArgs {
    /// Profile name to remove.
    pub name: String,
}

/// Arguments for `config profile default`.
#[derive(Debug, Args)]
pub struct ProfileDefaultArgs {
    /// Profile name to set as default.
    pub name: String,
}

/// Arguments for `config get`.
#[derive(Debug, Args)]
pub struct ConfigGetArgs {
    /// Dotted key path (e.g., daemon.fps, audio.gain).
    pub key: String,
}

/// Arguments for `config set`.
#[derive(Debug, Args)]
pub struct ConfigSetArgs {
    /// Dotted key path (e.g., daemon.fps, audio.gain).
    pub key: String,

    /// New value to set.
    pub value: String,

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
        ConfigCommand::Get(get_args) => execute_get(get_args, client, ctx).await,
        ConfigCommand::Set(set_args) => execute_set(set_args, client, ctx).await,
        ConfigCommand::Reset(reset_args) => execute_reset(reset_args, client, ctx).await,
        ConfigCommand::Path => execute_path(ctx),
        ConfigCommand::Profile(profile_args) => execute_profile(profile_args, ctx),
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

async fn execute_get(
    args: &ConfigGetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/config/get?key={}", urlencoded(&args.key));
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

    Ok(())
}

async fn execute_set(
    args: &ConfigSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "key": args.key,
        "value": args.value,
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
            ctx.success(&format!("{}: {}{applied}", args.key, args.value));
        }
    }

    Ok(())
}

async fn execute_reset(
    args: &ConfigResetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if args.key.is_none() && !args.yes {
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

/// Resolve the daemon config file path.
fn config_file_path() -> String {
    if let Ok(path) = std::env::var("HYPERCOLOR_CONFIG") {
        return path;
    }

    dirs::config_dir().map_or_else(
        || "~/.config/hypercolor/hypercolor.toml".to_string(),
        |d| {
            d.join("hypercolor")
                .join("hypercolor.toml")
                .to_string_lossy()
                .into_owned()
        },
    )
}

// ── Profile management ──────────────────────────────────────────────────

fn execute_profile(args: &ProfileArgs, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        ProfileCommand::List => profile_list(ctx),
        ProfileCommand::Show(show_args) => profile_show(show_args, ctx),
        ProfileCommand::Add(add_args) => profile_add(add_args, ctx),
        ProfileCommand::Set(set_args) => profile_set(set_args, ctx),
        ProfileCommand::Remove(remove_args) => profile_remove(remove_args, ctx),
        ProfileCommand::Default(default_args) => profile_default(default_args, ctx),
    }
}

fn profile_list(ctx: &OutputContext) -> Result<()> {
    let cfg = config::load()?;

    match ctx.format {
        OutputFormat::Json => {
            ctx.print_json(&serde_json::to_value(&cfg.profiles)?)?;
        }
        OutputFormat::Plain => {
            for name in cfg.profiles.keys() {
                let marker = if *name == cfg.defaults.profile {
                    " *"
                } else {
                    ""
                };
                println!("{name}{marker}");
            }
        }
        OutputFormat::Table => {
            let rows: Vec<Vec<String>> = cfg
                .profiles
                .iter()
                .map(|(name, p)| {
                    let default_marker = if *name == cfg.defaults.profile {
                        ctx.painter.keyword("*")
                    } else {
                        String::new()
                    };
                    vec![
                        format!("{}{default_marker}", ctx.painter.name(name)),
                        ctx.painter.muted(&format!("{}:{}", p.host, p.port)),
                        if p.api_key.is_some() {
                            ctx.painter.warning("api_key")
                        } else {
                            ctx.painter.muted("none")
                        },
                        p.label.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            ctx.print_table(&["Profile", "Host", "Auth", "Label"], &rows);
        }
    }

    Ok(())
}

fn profile_show(args: &ProfileShowArgs, ctx: &OutputContext) -> Result<()> {
    let cfg = config::load()?;
    let name = args.name.as_deref().unwrap_or(&cfg.defaults.profile);
    let profile = cfg
        .profiles
        .get(name)
        .with_context(|| format!("profile {name:?} not found"))?;

    match ctx.format {
        OutputFormat::Json => {
            ctx.print_json(&serde_json::to_value(profile)?)?;
        }
        OutputFormat::Plain | OutputFormat::Table => {
            println!("  Profile  {}", ctx.painter.name(name));
            println!("  Host     {}:{}", profile.host, profile.port);
            println!(
                "  Auth     {}",
                if profile.api_key.is_some() {
                    "api_key (set)"
                } else {
                    "none"
                }
            );
            if let Some(label) = &profile.label {
                println!("  Label    {label}");
            }
            if let Some(desc) = &profile.description {
                println!("  About    {desc}");
            }
        }
    }

    Ok(())
}

fn profile_add(args: &ProfileAddArgs, ctx: &OutputContext) -> Result<()> {
    let mut cfg = config::load()?;

    if cfg.profiles.contains_key(&args.name) {
        anyhow::bail!("profile {:?} already exists (use `config profile set` to update)", args.name);
    }

    cfg.profiles.insert(
        args.name.clone(),
        Profile {
            host: args.host.clone(),
            port: args.port,
            api_key: args.api_key.clone(),
            label: args.label.clone(),
            description: None,
        },
    );

    config::save(&cfg)?;
    ctx.success(&format!("Profile {:?} added ({}:{})", args.name, args.host, args.port));
    Ok(())
}

fn profile_set(args: &ProfileSetArgs, ctx: &OutputContext) -> Result<()> {
    let mut cfg = config::load()?;
    let profile = cfg
        .profiles
        .get_mut(&args.name)
        .with_context(|| format!("profile {:?} not found", args.name))?;

    match args.key.as_str() {
        "host" => profile.host = args.value.clone(),
        "port" => {
            profile.port = args
                .value
                .parse()
                .with_context(|| format!("invalid port: {:?}", args.value))?;
        }
        "api_key" => profile.api_key = Some(args.value.clone()),
        "label" => profile.label = Some(args.value.clone()),
        "description" => profile.description = Some(args.value.clone()),
        other => anyhow::bail!("unknown profile field: {other:?} (expected: host, port, api_key, label, description)"),
    }

    config::save(&cfg)?;
    ctx.success(&format!(
        "Profile {:?}: {} = {}",
        args.name, args.key, args.value
    ));
    Ok(())
}

fn profile_remove(args: &ProfileRemoveArgs, ctx: &OutputContext) -> Result<()> {
    let mut cfg = config::load()?;

    if cfg.profiles.remove(&args.name).is_none() {
        anyhow::bail!("profile {:?} not found", args.name);
    }

    config::save(&cfg)?;
    ctx.success(&format!("Profile {:?} removed", args.name));
    Ok(())
}

fn profile_default(args: &ProfileDefaultArgs, ctx: &OutputContext) -> Result<()> {
    let mut cfg = config::load()?;

    if !cfg.profiles.contains_key(&args.name) {
        anyhow::bail!("profile {:?} not found", args.name);
    }

    cfg.defaults.profile = args.name.clone();
    config::save(&cfg)?;
    ctx.success(&format!("Default profile set to {:?}", args.name));
    Ok(())
}
