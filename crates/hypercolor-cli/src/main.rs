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

// ── CLI definition ──────────────────────────────────────────────────────

/// Hypercolor RGB lighting control.
#[derive(Parser)]
#[command(
    name = "hyper",
    version,
    about = "RGB lighting orchestration engine",
    styles = output::painter::help_styles(),
    subcommand_required = true,
    arg_required_else_help = true,
)]
pub struct Cli {
    // ── Connection ──────────────────────────────────────────────────
    /// Daemon hostname or IP
    #[arg(
        long,
        global = true,
        default_value = "localhost",
        help_heading = "Connection"
    )]
    host: String,

    /// Daemon port
    #[arg(
        long,
        global = true,
        default_value_t = 9420,
        help_heading = "Connection"
    )]
    port: u16,

    /// Bearer token for authenticated requests
    #[arg(
        long,
        global = true,
        env = "HYPERCOLOR_API_KEY",
        help_heading = "Connection"
    )]
    api_key: Option<String>,

    /// Named connection profile from cli.toml
    #[arg(
        long,
        global = true,
        env = "HYPERCOLOR_PROFILE",
        help_heading = "Connection"
    )]
    profile: Option<String>,

    // ── Output ──────────────────────────────────────────────────────
    /// Output format: table, json, plain
    #[arg(
        long,
        global = true,
        default_value = "table",
        value_enum,
        hide_possible_values = true,
        help_heading = "Output"
    )]
    format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long, short = 'j', global = true, help_heading = "Output")]
    json: bool,

    /// Suppress non-essential output
    #[arg(long, short, global = true, help_heading = "Output")]
    quiet: bool,

    /// Disable colored output
    #[arg(long, global = true, help_heading = "Output")]
    no_color: bool,

    /// Color theme name
    #[arg(long, global = true, env = "HYPERCOLOR_THEME", help_heading = "Output")]
    theme: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count,
          help_heading = "Output")]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

/// Top-level subcommands.
///
/// `display_order` controls the order in which clap lists them under the
/// "Commands:" heading. Logical grouping: lighting → devices → library →
/// network → system.
#[derive(Subcommand)]
pub enum Commands {
    // ── Lighting ──────────────────────────────────────────────
    /// System state, render loop, and active effect
    #[command(display_order = 1)]
    Status(commands::status::StatusArgs),

    /// Browse, activate, and control lighting effects
    #[command(display_order = 2)]
    Effects(commands::effects::EffectsArgs),

    /// Global output brightness (0-100)
    #[command(display_order = 3)]
    Brightness(commands::brightness::BrightnessArgs),

    /// Automated lighting triggers and schedules
    #[command(display_order = 4)]
    Scenes(commands::scenes::ScenesArgs),

    // ── Devices ───────────────────────────────────────────────
    /// Discovery, pairing, and hardware management
    #[command(display_order = 10)]
    Devices(commands::devices::DevicesArgs),

    /// Spatial LED layout configuration
    #[command(display_order = 11)]
    Layouts(commands::layouts::LayoutsArgs),

    /// Audio input device selection
    #[command(display_order = 12)]
    Audio(commands::audio::AudioArgs),

    // ── Library ───────────────────────────────────────────────
    /// Favorites, presets, and playlists
    #[command(display_order = 20)]
    Library(commands::library::LibraryArgs),

    /// Save and apply full system profiles
    #[command(display_order = 21)]
    Profiles(commands::profiles::ProfilesArgs),

    // ── Network ───────────────────────────────────────────────
    /// Daemon version, identity, and health
    #[command(display_order = 30)]
    Server(commands::server::ServerArgs),

    /// Discover daemons on the local network
    #[command(display_order = 31)]
    Servers(commands::servers::ServersArgs),

    /// Daemon lifecycle (start, stop, restart)
    #[command(display_order = 32)]
    Service(commands::service::ServiceArgs),

    // ── System ────────────────────────────────────────────────
    /// Daemon and CLI configuration
    #[command(display_order = 40)]
    Config(commands::config::ConfigArgs),

    /// Health checks and diagnostic reports
    #[command(display_order = 41)]
    Diagnose(commands::diagnose::DiagnoseArgs),

    /// Generate shell completion scripts
    #[command(display_order = 42)]
    Completions(commands::completions::CompletionsArgs),
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_arg_matches(
        &Cli::command()
            .before_help(output::painter::help_banner())
            .get_matches(),
    )?;

    init_tracing(cli.verbose);

    let conn = config::resolve_connection(
        &cli.host,
        cli.port,
        cli.api_key.as_deref(),
        cli.profile.as_deref(),
    )?;

    let ctx = OutputContext::new(
        cli.format,
        cli.json,
        cli.quiet,
        cli.no_color,
        cli.theme.as_deref(),
    );
    let client = DaemonClient::new(&conn.host, conn.port, conn.api_key.as_deref());

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
