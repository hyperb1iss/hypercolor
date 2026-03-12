//! Hypercolor TUI — a live terminal instrument for controlling light.

use anyhow::Result;
use clap::Parser;

/// Terminal UI for the Hypercolor RGB lighting engine.
#[derive(Parser, Debug)]
#[command(name = "hypercolor-tui")]
#[command(about = "Terminal UI for controlling Hypercolor lighting")]
#[command(version)]
struct Args {
    /// Daemon host address.
    #[arg(long, default_value = "127.0.0.1", env = "HYPERCOLOR_HOST")]
    host: String,

    /// Daemon API port.
    #[arg(long, default_value_t = 9420, env = "HYPERCOLOR_PORT")]
    port: u16,

    /// Opaline theme name (default: silkcircuit-neon).
    #[arg(long, env = "HYPERCOLOR_THEME")]
    theme: Option<String>,

    /// Log level (error, warn, info, debug, trace).
    #[arg(long, default_value = "warn", env = "HYPERCOLOR_LOG")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing (logs to file so they don't clobber the TUI)
    tracing_subscriber::fmt()
        .with_env_filter(&args.log_level)
        .with_writer(std::io::stderr)
        .init();

    // Initialize theme
    hypercolor_tui::theme::initialize(args.theme.as_deref());

    // Install panic hook that restores the terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        ratatui::restore();
        original_hook(panic_info);
    }));

    // Run the app
    let mut app = hypercolor_tui::app::App::new(args.host, args.port);
    app.run().await
}
