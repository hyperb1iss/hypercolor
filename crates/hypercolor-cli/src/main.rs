//! Hypercolor CLI -- control your RGB lighting from the terminal.
//!
//! The `hyper` binary is the primary interface for interacting with the
//! Hypercolor daemon. It communicates via HTTP REST to the daemon's API
//! and renders output as styled tables, plain text, or JSON.

mod client;
mod commands;
pub mod config;
mod output;

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use client::DaemonClient;
use output::{OutputContext, OutputFormat};

/// Hypercolor RGB lighting control.
#[derive(Parser)]
#[command(name = "hyper", version, about = "Hypercolor RGB lighting control")]
#[command(propagate_version = true, styles = output::painter::help_styles())]
pub struct Cli {
    /// Output format.
    #[arg(long, global = true, default_value = "table", value_enum)]
    format: OutputFormat,

    /// Daemon host.
    #[arg(long, global = true, default_value = "localhost")]
    host: String,

    /// Daemon port.
    #[arg(long, global = true, default_value_t = 9420)]
    port: u16,

    /// API key used for authenticated daemon requests.
    #[arg(long, global = true, env = "HYPERCOLOR_API_KEY")]
    api_key: Option<String>,

    /// Connection profile name from ~/.config/hypercolor/cli.toml.
    #[arg(long, global = true, env = "HYPERCOLOR_PROFILE")]
    profile: Option<String>,

    /// Color theme name (default: silkcircuit-neon).
    #[arg(long, global = true, env = "HYPERCOLOR_THEME")]
    theme: Option<String>,

    /// JSON output (shorthand for --format json).
    #[arg(long, short = 'j', global = true)]
    json: bool,

    /// Suppress non-essential output.
    #[arg(long, short, global = true)]
    quiet: bool,

    /// Disable colored output.
    #[arg(long, global = true)]
    no_color: bool,

    /// Increase verbosity (-v, -vv, -vvv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

/// Top-level subcommands.
#[derive(Subcommand)]
pub enum Commands {
    /// Show current system state.
    Status(commands::status::StatusArgs),
    /// Device discovery and management.
    Devices(commands::devices::DevicesArgs),
    /// Effect browsing and control.
    Effects(commands::effects::EffectsArgs),
    /// Scene management (automated lighting triggers).
    Scenes(commands::scenes::ScenesArgs),
    /// Profile management (save, apply, delete).
    Profiles(commands::profiles::ProfilesArgs),
    /// Saved effect library (favorites, presets, playlists).
    Library(commands::library::LibraryArgs),
    /// Spatial layout management.
    Layouts(commands::layouts::LayoutsArgs),
    /// Global output brightness control.
    Brightness(commands::brightness::BrightnessArgs),
    /// Audio input device management.
    Audio(commands::audio::AudioArgs),
    /// Daemon identity and health.
    Server(commands::server::ServerArgs),
    /// Configuration management.
    Config(commands::config::ConfigArgs),
    /// Daemon service lifecycle management.
    Service(commands::service::ServiceArgs),
    /// Run system diagnostics and health checks.
    Diagnose(commands::diagnose::DiagnoseArgs),
    /// Discover Hypercolor daemons on the local network.
    Servers(commands::servers::ServersArgs),
    /// Generate shell completion scripts.
    Completions(commands::completions::CompletionsArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_arg_matches(
        &Cli::command()
            .before_help(output::painter::help_banner())
            .get_matches(),
    )?;

    // Initialize tracing based on verbosity
    init_tracing(cli.verbose);

    // Resolve connection profile (flags > env > profile > defaults)
    let conn = config::resolve_connection(
        &cli.host,
        cli.port,
        cli.api_key.as_deref(),
        cli.profile.as_deref(),
    )?;

    // Build shared context
    let ctx = OutputContext::new(
        cli.format,
        cli.json,
        cli.quiet,
        cli.no_color,
        cli.theme.as_deref(),
    );
    let client = DaemonClient::new(&conn.host, conn.port, conn.api_key.as_deref());

    // Dispatch to subcommand handlers
    let result = match &cli.command {
        Commands::Status(args) => commands::status::execute(args, &client, &ctx).await,
        Commands::Devices(args) => commands::devices::execute(args, &client, &ctx).await,
        Commands::Effects(args) => commands::effects::execute(args, &client, &ctx).await,
        Commands::Scenes(args) => commands::scenes::execute(args, &client, &ctx).await,
        Commands::Profiles(args) => commands::profiles::execute(args, &client, &ctx).await,
        Commands::Library(args) => commands::library::execute(args, &client, &ctx).await,
        Commands::Layouts(args) => commands::layouts::execute(args, &client, &ctx).await,
        Commands::Brightness(args) => commands::brightness::execute(args, &client, &ctx).await,
        Commands::Audio(args) => commands::audio::execute(args, &client, &ctx).await,
        Commands::Server(args) => commands::server::execute(args, &client, &ctx).await,
        Commands::Config(args) => commands::config::execute(args, &client, &ctx).await,
        Commands::Service(args) => commands::service::execute(args, &ctx).await,
        Commands::Diagnose(args) => commands::diagnose::execute(args, &client, &ctx).await,
        Commands::Servers(args) => commands::servers::execute(args, &ctx).await,
        Commands::Completions(args) => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            commands::completions::execute(args, &mut cmd);
            Ok(())
        }
    };

    if let Err(e) = result {
        ctx.error(&format!("{e:#}"));
        std::process::exit(1);
    }

    Ok(())
}

/// Initialize `tracing-subscriber` based on CLI verbosity level.
fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
