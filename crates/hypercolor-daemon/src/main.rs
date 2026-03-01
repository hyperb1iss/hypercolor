use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use hypercolor_daemon::api::{self, AppState};
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
    let daemon_state = DaemonState::initialize(&config, config_path)?;

    // 4. Start subsystems (render loop, discovery, etc.).
    daemon_state.start().await?;

    // 5. Build the API server with shared daemon state.
    let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
    let router = api::build_router(app_state);

    let listener = tokio::net::TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("failed to bind API server to {}", args.bind))?;

    info!(bind = %args.bind, "API server listening");

    // 6. Install signal handlers for graceful shutdown.
    let mut shutdown_rx = install_signal_handlers();

    // 7. Serve HTTP with graceful shutdown.
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
            info!("Shutdown signal received, stopping API server");
        })
        .await
        .context("API server error")?;

    // 8. Graceful shutdown of daemon subsystems.
    daemon_state.shutdown().await?;

    info!("Hypercolor daemon exited cleanly");
    Ok(())
}
