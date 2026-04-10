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

// ── Help template ───────────────────────────────────────────────────────
//
// We render the command list ourselves (with group headings) and let clap
// render options (with help_heading groups). The template stitches them
// together: banner → about → usage → [our command groups] → [clap options].

const HELP_TEMPLATE: &str = "\
{before-help}
{about-with-newline}
{usage-heading} {usage}
{after-help}
{all-args}";

// ── CLI definition ──────────────────────────────────────────────────────

/// Hypercolor RGB lighting control.
#[derive(Parser)]
#[command(
    name = "hyper",
    version,
    about = "RGB lighting orchestration engine",
    help_template = HELP_TEMPLATE,
    styles = output::painter::help_styles(),
    subcommand_required = true,
    arg_required_else_help = true,
)]
pub struct Cli {
    // ── Connection ──────────────────────────────────────────────────

    /// Daemon hostname or IP
    #[arg(long, global = true, default_value = "localhost",
          help_heading = "Connection")]
    host: String,

    /// Daemon port
    #[arg(long, global = true, default_value_t = 9420,
          help_heading = "Connection")]
    port: u16,

    /// Bearer token for authenticated requests
    #[arg(long, global = true, env = "HYPERCOLOR_API_KEY",
          help_heading = "Connection")]
    api_key: Option<String>,

    /// Named connection profile from cli.toml
    #[arg(long, global = true, env = "HYPERCOLOR_PROFILE",
          help_heading = "Connection")]
    profile: Option<String>,

    // ── Output ──────────────────────────────────────────────────────

    /// Output format: table, json, plain
    #[arg(long, global = true, default_value = "table", value_enum,
          hide_possible_values = true,
          help_heading = "Output")]
    format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long, short = 'j', global = true,
          help_heading = "Output")]
    json: bool,

    /// Suppress non-essential output
    #[arg(long, short, global = true,
          help_heading = "Output")]
    quiet: bool,

    /// Disable colored output
    #[arg(long, global = true,
          help_heading = "Output")]
    no_color: bool,

    /// Color theme name
    #[arg(long, global = true, env = "HYPERCOLOR_THEME",
          help_heading = "Output")]
    theme: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count,
          help_heading = "Output")]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

/// Top-level subcommands (hidden from default help — rendered by help_commands).
#[derive(Subcommand)]
pub enum Commands {
    #[command(hide = true)]
    /// System state, render loop, and active effect
    Status(commands::status::StatusArgs),

    #[command(hide = true)]
    /// Discovery, pairing, and hardware management
    Devices(commands::devices::DevicesArgs),

    #[command(hide = true)]
    /// Browse, activate, and control lighting effects
    Effects(commands::effects::EffectsArgs),

    #[command(hide = true)]
    /// Automated lighting triggers and schedules
    Scenes(commands::scenes::ScenesArgs),

    #[command(hide = true)]
    /// Save and apply full system profiles
    Profiles(commands::profiles::ProfilesArgs),

    #[command(hide = true)]
    /// Favorites, presets, and playlists
    Library(commands::library::LibraryArgs),

    #[command(hide = true)]
    /// Spatial LED layout configuration
    Layouts(commands::layouts::LayoutsArgs),

    #[command(hide = true)]
    /// Global output brightness (0\u{2013}100)
    Brightness(commands::brightness::BrightnessArgs),

    #[command(hide = true)]
    /// Audio input device selection
    Audio(commands::audio::AudioArgs),

    #[command(hide = true)]
    /// Daemon version, identity, and health
    Server(commands::server::ServerArgs),

    #[command(hide = true)]
    /// Daemon and CLI configuration
    Config(commands::config::ConfigArgs),

    #[command(hide = true)]
    /// Daemon lifecycle (start, stop, restart)
    Service(commands::service::ServiceArgs),

    #[command(hide = true)]
    /// Health checks and diagnostic reports
    Diagnose(commands::diagnose::DiagnoseArgs),

    #[command(hide = true)]
    /// Discover daemons on the local network
    Servers(commands::servers::ServersArgs),

    #[command(hide = true)]
    /// Generate shell completion scripts
    Completions(commands::completions::CompletionsArgs),
}

// ── Main ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_arg_matches(
        &Cli::command()
            .before_help(output::painter::help_banner())
            .after_help(output::painter::help_commands())
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
