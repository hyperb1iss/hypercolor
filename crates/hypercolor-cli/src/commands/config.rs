//! `hyper config` -- configuration management (daemon config + CLI profiles).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::config::{self, Profile};
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// File name the daemon reads its configuration from.
const DAEMON_CONFIG_FILE_NAME: &str = "hypercolor.toml";

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
    /// Field to update (host, port, `api_key`, label, description).
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

    /// Apply the change to the running daemon immediately. This is the
    /// daemon's default for every key it can re-apply live.
    #[arg(long, conflicts_with = "no_live")]
    pub live: bool,

    /// Persist the change without touching the running daemon.
    #[arg(long)]
    pub no_live: bool,
}

impl ConfigSetArgs {
    /// The live-apply request, or `None` to take the daemon's default.
    const fn live_request(&self) -> Option<bool> {
        match (self.live, self.no_live) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        }
    }
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
    let response: serde_json::Value = client.get("/config").await?;

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
    let response: serde_json::Value = client.get(&config_key_path(&args.key)).await?;

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

/// Address one config key as a resource.
fn config_key_path(key: &str) -> String {
    format!("/config/keys/{}", urlencoded(key))
}

/// Address one config key, carrying an explicit live-apply request.
fn config_key_path_with_live(key: &str, live: Option<bool>) -> String {
    let path = config_key_path(key);
    match live {
        Some(live) => format!("{path}?live={live}"),
        None => path,
    }
}

/// Read a command-line value as JSON, falling back to a JSON string.
///
/// The daemon used to do this coercion on a stringified body; the
/// resource route takes the value as JSON, so the parse lives where the
/// human-typed text does: `9420` is a number, `microphone` is a string,
/// and `["10.0.0.1"]` is an array.
fn parse_cli_value(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

async fn execute_set(
    args: &ConfigSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = config_key_path_with_live(&args.key, args.live_request());
    let response: serde_json::Value = client.put(&path, &parse_cli_value(&args.value)).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            // The daemon reports what it actually did, so the note
            // follows the response rather than the requested flag.
            let applied = if response["live"] == serde_json::Value::Bool(true) {
                "  (applied to running daemon)"
            } else if response["requires_restart"] == serde_json::Value::Bool(true) {
                "  (restart the daemon to activate)"
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

    let response = match &args.key {
        Some(key) => {
            client
                .delete::<serde_json::Value>(&config_key_path(key))
                .await?
        }
        None => {
            client
                .post("/config/reset", &serde_json::Value::Null)
                .await?
        }
    };

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
///
/// Uses the daemon's own directory resolution so `hypercolor config path`
/// reports the file the daemon actually reads.
fn config_file_path() -> String {
    if let Ok(path) = std::env::var("HYPERCOLOR_CONFIG") {
        return path;
    }

    resolve_daemon_config_path(Some(hypercolor_core::config::paths::config_dir()))
        .expect("a resolved config directory always yields a config file path")
        .to_string_lossy()
        .into_owned()
}

/// Place the daemon config file inside a resolved config directory.
///
/// Split out from [`config_file_path`] so the unresolvable case is reachable
/// from a test: without a directory this must yield nothing rather than
/// fabricate a relative path. The environment half cannot be driven directly
/// because edition 2024 makes `std::env::set_var` unsafe and this crate
/// forbids it.
fn resolve_daemon_config_path(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join(DAEMON_CONFIG_FILE_NAME))
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
                        if p.api_key.as_ref().is_some_and(|k| !k.is_empty()) {
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
                if profile.api_key.as_ref().is_some_and(|k| !k.is_empty()) {
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
        anyhow::bail!(
            "profile {:?} already exists (use `config profile set` to update)",
            args.name
        );
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
    ctx.success(&format!(
        "Profile {:?} added ({}:{})",
        args.name, args.host, args.port
    ));
    Ok(())
}

fn profile_set(args: &ProfileSetArgs, ctx: &OutputContext) -> Result<()> {
    let mut cfg = config::load()?;
    let profile = cfg
        .profiles
        .get_mut(&args.name)
        .with_context(|| format!("profile {:?} not found", args.name))?;

    match args.key.as_str() {
        "host" => profile.host.clone_from(&args.value),
        "port" => {
            profile.port = args
                .value
                .parse()
                .with_context(|| format!("invalid port: {:?}", args.value))?;
        }
        "api_key" => profile.api_key = Some(args.value.clone()),
        "label" => profile.label = Some(args.value.clone()),
        "description" => profile.description = Some(args.value.clone()),
        other => anyhow::bail!(
            "unknown profile field: {other:?} (expected: host, port, api_key, label, description)"
        ),
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

    cfg.defaults.profile.clone_from(&args.name);
    config::save(&cfg)?;
    ctx.success(&format!("Default profile set to {:?}", args.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ConfigSetArgs, DAEMON_CONFIG_FILE_NAME, config_file_path, config_key_path,
        config_key_path_with_live, parse_cli_value, resolve_daemon_config_path,
    };

    #[test]
    fn config_keys_address_one_path_segment() {
        assert_eq!(
            config_key_path("daemon.target_fps"),
            "/config/keys/daemon.target_fps"
        );
        assert_eq!(
            config_key_path("drivers.wled/../hue"),
            "/config/keys/drivers.wled%2F..%2Fhue"
        );
    }

    #[test]
    fn live_flags_map_onto_the_query_the_daemon_reads() {
        let set = |live: bool, no_live: bool| ConfigSetArgs {
            key: "audio.device".to_owned(),
            value: "microphone".to_owned(),
            live,
            no_live,
        };

        assert_eq!(set(false, false).live_request(), None);
        assert_eq!(set(true, false).live_request(), Some(true));
        assert_eq!(set(false, true).live_request(), Some(false));

        assert_eq!(
            config_key_path_with_live("audio.device", None),
            "/config/keys/audio.device"
        );
        assert_eq!(
            config_key_path_with_live("audio.device", Some(false)),
            "/config/keys/audio.device?live=false"
        );
    }

    #[test]
    fn command_line_values_reach_the_wire_as_json() {
        assert_eq!(parse_cli_value("9420"), serde_json::json!(9420));
        assert_eq!(parse_cli_value("true"), serde_json::json!(true));
        assert_eq!(parse_cli_value("2.5"), serde_json::json!(2.5));
        assert_eq!(
            parse_cli_value(r#"["10.0.0.1"]"#),
            serde_json::json!(["10.0.0.1"])
        );
        assert_eq!(
            parse_cli_value("microphone"),
            serde_json::json!("microphone")
        );
    }

    #[test]
    fn unresolvable_config_dir_yields_no_path() {
        assert_eq!(resolve_daemon_config_path(None), None);
    }

    #[test]
    fn daemon_config_path_is_absolute_and_tilde_free() {
        // The env override is caller-owned and cannot be cleared from a test:
        // edition 2024 makes `std::env::set_var` unsafe and this crate forbids
        // unsafe code.
        if std::env::var_os("HYPERCOLOR_CONFIG").is_some() {
            return;
        }
        let reported = config_file_path();
        let path = Path::new(&reported);
        assert!(path.is_absolute(), "reported {reported} is not absolute");
        assert!(
            !path.components().any(|part| part.as_os_str() == "~"),
            "reported {reported} contains a literal tilde component"
        );
        assert_eq!(
            path,
            hypercolor_core::config::paths::config_dir().join(DAEMON_CONFIG_FILE_NAME)
        );
    }
}
