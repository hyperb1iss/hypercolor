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
    #[arg(long)]
    bind: Option<String>,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Serve the web UI from this directory (static files with SPA fallback).
    #[arg(long)]
    ui_dir: Option<PathBuf>,

    /// Run the MCP server over stdio instead of serving the REST API.
    #[arg(long, default_value_t = false)]
    mcp_stdio: bool,
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
        bind = ?args.bind,
        "Hypercolor daemon starting"
    );

    // 2. Load configuration.
    let (config, config_path) = load_config(args.config.as_deref()).await?;

    info!(
        schema_version = config.schema_version,
        target_fps = config.daemon.target_fps,
        "Configuration ready"
    );

    let bind = args
        .bind
        .clone()
        .unwrap_or_else(|| format!("{}:{}", config.daemon.listen_address, config.daemon.port));

    // 3. Initialize all subsystems.
    let mut daemon_state = DaemonState::initialize(&config, config_path)?;

    // 4. Start subsystems (render loop, render thread, discovery).
    daemon_state.start().await?;

    if args.mcp_stdio {
        info!("MCP stdio mode enabled");
        let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
        let mut shutdown_rx = install_signal_handlers();

        tokio::select! {
            mcp_result = hypercolor_daemon::mcp::run_stdio_server_with_state(Arc::clone(&app_state)) => {
                mcp_result.context("MCP stdio server error")?;
            }
            _ = shutdown_rx.changed() => {
                info!("Shutdown signal received, stopping MCP server");
            }
        }

        daemon_state.shutdown().await?;
        info!("Hypercolor daemon exited cleanly");
        return Ok(());
    }

    // 5. Build the API server with shared daemon state.
    let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
    let router = api::build_router(app_state, args.ui_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("failed to bind API server to {bind}"))?;

    info!(bind = %bind, "API server listening");

    // 6. Install signal handlers for graceful shutdown.
    let mut shutdown_rx = install_signal_handlers();

    // 7. Serve HTTP with graceful shutdown.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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
