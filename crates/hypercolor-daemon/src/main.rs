use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use hypercolor_types::config::{HypercolorConfig, LogLevel};
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
    #[arg(long)]
    log_level: Option<String>,

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

    // Load configuration before tracing so we can honor config-driven log
    // levels when the CLI flag is omitted.
    let (config, config_path) = load_config(args.config.as_deref()).await?;
    let log_level = resolve_log_level(args.log_level.as_deref(), &config);

    // 1. Initialize tracing with the requested log level.
    //    The `RUST_LOG` env var takes precedence if set.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_env_filter(&log_level)));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = ?args.bind,
        log_level = %log_level,
        "Hypercolor daemon starting"
    );

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

    // 5. Resolve UI directory — explicit flag or auto-discover from workspace.
    let ui_dir = args.ui_dir.or_else(|| {
        let candidate = PathBuf::from("crates/hypercolor-ui/dist");
        if candidate.join("index.html").exists() {
            info!(path = %candidate.display(), "Auto-discovered web UI");
            Some(candidate)
        } else {
            None
        }
    });

    // 6. Build the API server with shared daemon state.
    let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
    let router = api::build_router(app_state, ui_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("failed to bind API server to {bind}"))?;

    if ui_dir.is_some() {
        info!(url = %format!("http://{bind}/"), "Web UI available");
    }
    info!(bind = %bind, "API server listening");

    // 7. Install signal handlers for graceful shutdown.
    let mut shutdown_rx = install_signal_handlers();

    // 8. Serve HTTP with graceful shutdown.
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

    // 9. Graceful shutdown of daemon subsystems.
    daemon_state.shutdown().await?;

    info!("Hypercolor daemon exited cleanly");
    Ok(())
}

fn default_env_filter(log_level: &str) -> String {
    let normalized = log_level.trim().to_ascii_lowercase();
    if normalized == "debug" {
        // Keep third-party crates quiet in debug mode, while still surfacing
        // detailed logs from Hypercolor crates.
        return "warn,hypercolor=debug,hypercolor_daemon=debug,hypercolor_core=debug,hypercolor_hal=debug,hypercolor_types=debug".to_owned();
    }

    normalized
}

fn resolve_log_level(cli_log_level: Option<&str>, config: &HypercolorConfig) -> String {
    cli_log_level.map_or_else(
        || config_log_level_name(&config.daemon.log_level).to_owned(),
        |value| value.trim().to_ascii_lowercase(),
    )
}

const fn config_log_level_name(level: &LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{default_env_filter, resolve_log_level};
    use hypercolor_types::config::{HypercolorConfig, LogLevel};

    #[test]
    fn resolve_log_level_prefers_cli_flag() {
        let mut config = HypercolorConfig::default();
        config.daemon.log_level = LogLevel::Warn;

        assert_eq!(resolve_log_level(Some("debug"), &config), "debug");
    }

    #[test]
    fn resolve_log_level_falls_back_to_config() {
        let mut config = HypercolorConfig::default();
        config.daemon.log_level = LogLevel::Debug;

        assert_eq!(resolve_log_level(None, &config), "debug");
    }

    #[test]
    fn default_env_filter_scopes_hypercolor_debug_logs() {
        assert_eq!(
            default_env_filter("debug"),
            "warn,hypercolor=debug,hypercolor_daemon=debug,hypercolor_core=debug,hypercolor_hal=debug,hypercolor_types=debug"
        );
    }
}
