use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use hypercolor_daemon::startup::{DaemonState, install_signal_handlers, load_config};

// ── CLI Arguments ───────────────────────────────────────────────────────────

/// Hypercolor lighting daemon — orchestrates RGB devices at up to 60fps.
#[derive(Parser, Debug)]
#[command(name = "hypercolor", about = "Hypercolor lighting daemon")]
struct DaemonArgs {
    /// Path to the configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Address and port to bind the API server to.
    #[arg(long, default_value = "127.0.0.1:9420")]
    bind: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,
}

// ── Entry Point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = DaemonArgs::parse();

    // 1. Initialize tracing with the requested log level.
    //    The `RUST_LOG` env var takes precedence if set.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %args.bind,
        "Hypercolor daemon starting"
    );

    // 2. Load configuration.
    let (config, config_path) = load_config(args.config.as_deref()).await?;

    info!(
        schema_version = config.schema_version,
        target_fps = config.daemon.target_fps,
        "Configuration ready"
    );

    // 3. Initialize all subsystems.
    let state = DaemonState::initialize(&config, config_path)?;

    // 4. Start subsystems (render loop, discovery, etc.).
    state.start().await?;

    // 5. Install signal handlers for graceful shutdown.
    let mut shutdown_rx = install_signal_handlers();

    // 6. Wait for shutdown signal.
    shutdown_rx
        .changed()
        .await
        .expect("shutdown signal channel closed unexpectedly");

    // 7. Graceful shutdown.
    state.shutdown().await?;

    info!("Hypercolor daemon exited cleanly");
    Ok(())
}
