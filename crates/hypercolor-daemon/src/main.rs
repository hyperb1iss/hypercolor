use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use hypercolor_types::config::{HypercolorConfig, LogLevel, RenderAccelerationMode};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::mdns::MdnsPublisher;
use hypercolor_daemon::startup::{DaemonState, install_signal_handlers, load_config};

const MAIN_RUNTIME_WORKERS: usize = 4;
const MAIN_RUNTIME_MAX_BLOCKING_THREADS: usize = 8;
const MAIN_RUNTIME_THREAD_KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(2);

// ── CLI Arguments ───────────────────────────────────────────────────────────

/// Hypercolor lighting daemon — orchestrates RGB devices at up to 60fps.
#[derive(Parser, Debug)]
#[command(name = "hypercolor-daemon", about = "Hypercolor lighting daemon")]
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

    /// Override the configured compositor acceleration mode.
    #[arg(long, alias = "render-acceleration-mode", value_enum)]
    compositor_acceleration_mode: Option<RenderAccelerationModeArg>,

    /// Serve the web UI from this directory (static files with SPA fallback).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RenderAccelerationModeArg {
    Cpu,
    Auto,
    Gpu,
}

impl From<RenderAccelerationModeArg> for RenderAccelerationMode {
    fn from(value: RenderAccelerationModeArg) -> Self {
        match value {
            RenderAccelerationModeArg::Cpu => Self::Cpu,
            RenderAccelerationModeArg::Auto => Self::Auto,
            RenderAccelerationModeArg::Gpu => Self::Gpu,
        }
    }
}

// ── Entry Point ─────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(MAIN_RUNTIME_WORKERS)
        .max_blocking_threads(MAIN_RUNTIME_MAX_BLOCKING_THREADS)
        .thread_keep_alive(MAIN_RUNTIME_THREAD_KEEP_ALIVE)
        .thread_name("hypercolor-main-rt")
        .enable_all()
        .build()
        .context("failed to initialize daemon runtime")?;

    runtime.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let args = DaemonArgs::parse();

    // Load configuration before tracing so we can honor config-driven log
    // levels when the CLI flag is omitted.
    let (mut config, config_path) = load_config(args.config.as_deref()).await?;
    if let Some(mode) = args.compositor_acceleration_mode {
        config.effect_engine.compositor_acceleration_mode = mode.into();
    }
    let log_level = resolve_log_level(args.log_level.as_deref(), &config);

    // 1. Initialize tracing with the requested log level + SilkCircuit theme.
    //    The `RUST_LOG` env var takes precedence if set.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_env_filter(&log_level)));

    hypercolor_daemon::startup::logging::install(env_filter);

    let listen_addr = args.bind.as_deref().map_or_else(
        || format!("{}:{}", config.daemon.listen_address, config.daemon.port),
        String::from,
    );
    hypercolor_daemon::startup::banner::print(
        env!("CARGO_PKG_VERSION"),
        (config.daemon.canvas_width, config.daemon.canvas_height),
        &listen_addr,
    );

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

    let bind = resolve_bind_address(args.bind.as_deref(), &config).await?;

    if !bind.ip().is_loopback() && !api::security::control_api_key_configured_from_env() {
        warn!(
            bind = %bind,
            "Network-accessible without API key — anyone on your network can control your lights"
        );
    }

    // 3. Initialize all subsystems.
    let mut daemon_state = DaemonState::initialize(&config, config_path)?;

    // 4. Start subsystems (render loop, render thread, discovery).
    daemon_state.start().await?;

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

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind API server to {bind}"))?;

    let mdns_publisher = MdnsPublisher::new(
        &daemon_state.server_identity,
        bind,
        config.network.mdns_publish,
        api::security::api_auth_required_from_env(),
    )?;

    if ui_dir.is_some() {
        info!(url = %format!("http://{bind}/"), "Web UI available");
    }
    info!(bind = %bind, "API server listening");

    notify_ready();
    spawn_watchdog();

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

    if let Some(publisher) = mdns_publisher {
        publisher.shutdown().await;
    }

    // 9. Graceful shutdown of daemon subsystems.
    daemon_state.shutdown().await?;

    info!("Hypercolor daemon exited cleanly");
    Ok(())
}

// ── systemd Integration ────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn notify_ready() {
    if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
        tracing::warn!("failed to notify systemd: {e}");
    } else {
        tracing::debug!("notified systemd: READY=1");
    }
}

#[cfg(not(target_os = "linux"))]
fn notify_ready() {}

#[cfg(target_os = "linux")]
fn spawn_watchdog() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn spawn_watchdog() {}

fn default_env_filter(log_level: &str) -> String {
    let normalized = log_level.trim().to_ascii_lowercase();

    // mdns_sd's internal parser logs ERROR for every malformed mDNS response on
    // the network (Apple devices with curly-quote hostnames, truncated packets,
    // etc.). These are unactionable noise; our own mDNS code in
    // hypercolor_core::device::net::mdns still logs normally.
    const MDNS_SQUELCH: &str = "mdns_sd::service_daemon=off";

    if normalized == "debug" {
        return format!(
            "warn,hypercolor=debug,hypercolor_daemon=debug,hypercolor_core=debug,hypercolor_hal=debug,hypercolor_types=debug,{MDNS_SQUELCH}"
        );
    }

    format!("{normalized},{MDNS_SQUELCH}")
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

async fn resolve_bind_address(
    cli_bind: Option<&str>,
    config: &HypercolorConfig,
) -> Result<SocketAddr> {
    if let Some(bind) = cli_bind {
        return resolve_socket_addr(bind).await;
    }

    let host = if config.network.remote_access && is_loopback_host(&config.daemon.listen_address) {
        IpAddr::from([0, 0, 0, 0]).to_string()
    } else {
        config.daemon.listen_address.clone()
    };

    resolve_socket_addr(&format!("{host}:{}", config.daemon.port)).await
}

async fn resolve_socket_addr(bind: &str) -> Result<SocketAddr> {
    let mut addrs = tokio::net::lookup_host(bind)
        .await
        .with_context(|| format!("failed to resolve {bind}"))?;
    addrs
        .next()
        .with_context(|| format!("no socket addresses resolved for {bind}"))
}

fn is_loopback_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.eq_ignore_ascii_case("localhost") {
        return true;
    }

    trimmed
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{DaemonArgs, default_env_filter, resolve_log_level};
    use hypercolor_types::config::{HypercolorConfig, LogLevel, RenderAccelerationMode};

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
        let filter = default_env_filter("debug");
        assert!(filter.starts_with("warn,hypercolor=debug,"));
        assert!(filter.contains("mdns_sd::service_daemon=off"));
    }

    #[test]
    fn default_env_filter_squelches_mdns_at_all_levels() {
        for level in ["info", "warn", "error", "trace"] {
            let filter = default_env_filter(level);
            assert!(
                filter.contains("mdns_sd::service_daemon=off"),
                "level {level} should squelch mdns_sd"
            );
        }
    }

    #[test]
    fn compositor_acceleration_mode_cli_override_updates_config() {
        let args = DaemonArgs::try_parse_from([
            "hypercolor-daemon",
            "--compositor-acceleration-mode",
            "gpu",
        ])
        .expect("CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.compositor_acceleration_mode {
            config.effect_engine.compositor_acceleration_mode = mode.into();
        }

        assert_eq!(
            config.effect_engine.compositor_acceleration_mode,
            RenderAccelerationMode::Gpu
        );
    }

    #[test]
    fn legacy_render_acceleration_mode_cli_alias_updates_config() {
        let args =
            DaemonArgs::try_parse_from(["hypercolor-daemon", "--render-acceleration-mode", "gpu"])
                .expect("legacy CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.compositor_acceleration_mode {
            config.effect_engine.compositor_acceleration_mode = mode.into();
        }

        assert_eq!(
            config.effect_engine.compositor_acceleration_mode,
            RenderAccelerationMode::Gpu
        );
    }
}
