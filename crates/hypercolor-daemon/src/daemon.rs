//! Foreground daemon runtime shared by console and service entry points.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_types::config::{HypercolorConfig, LogLevel, RenderAccelerationMode};
use tokio::sync::watch;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::api::{self, AppState};
use crate::mdns::MdnsPublisher;
use crate::startup::{DaemonState, load_config};

const MAIN_RUNTIME_WORKERS: usize = 4;
const MAIN_RUNTIME_MAX_BLOCKING_THREADS: usize = 8;
const MAIN_RUNTIME_THREAD_KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(2);

/// Runtime options for one daemon process.
#[derive(Clone, Debug, Default)]
pub struct DaemonRunOptions {
    /// Path to the configuration file.
    pub config: Option<PathBuf>,
    /// Address and port to bind the API server to.
    pub bind: Option<String>,
    /// Log level override.
    pub log_level: Option<String>,
    /// Compositor acceleration override.
    pub compositor_acceleration_mode: Option<RenderAccelerationMode>,
    /// Static web UI directory.
    pub ui_dir: Option<PathBuf>,
}

/// Build the daemon's main Tokio runtime.
///
/// # Errors
///
/// Returns an error when Tokio cannot initialize the runtime.
pub fn build_main_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(MAIN_RUNTIME_WORKERS)
        .max_blocking_threads(MAIN_RUNTIME_MAX_BLOCKING_THREADS)
        .thread_keep_alive(MAIN_RUNTIME_THREAD_KEEP_ALIVE)
        .thread_name("hypercolor-main-rt")
        .enable_all()
        .build()
        .context("failed to initialize daemon runtime")
}

/// Run the daemon until the shutdown receiver flips to `true`.
///
/// # Errors
///
/// Returns an error when startup, serving, or graceful shutdown fails.
pub async fn run(options: DaemonRunOptions, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
    // Load configuration before tracing so we can honor config-driven log
    // levels when the CLI flag is omitted.
    let (mut config, config_path) = load_config(options.config.as_deref()).await?;
    if let Some(mode) = options.compositor_acceleration_mode {
        config.effect_engine.compositor_acceleration_mode = mode;
    }
    let log_level = resolve_log_level(options.log_level.as_deref(), &config);

    // Initialize tracing with the requested log level + SilkCircuit theme.
    // The `RUST_LOG` env var takes precedence if set.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_env_filter(&log_level)));

    crate::startup::logging::install(env_filter);

    let listen_addr = options.bind.as_deref().map_or_else(
        || format!("{}:{}", config.daemon.listen_address, config.daemon.port),
        String::from,
    );
    crate::startup::banner::print(
        env!("CARGO_PKG_VERSION"),
        (config.daemon.canvas_width, config.daemon.canvas_height),
        &listen_addr,
    );

    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = ?options.bind,
        log_level = %log_level,
        "Hypercolor daemon starting"
    );

    info!(
        schema_version = config.schema_version,
        target_fps = config.daemon.target_fps,
        "Configuration ready"
    );

    let bind = resolve_bind_address(options.bind.as_deref(), &config).await?;

    if !bind.ip().is_loopback() && !api::security::control_api_key_configured_from_env() {
        warn!(
            bind = %bind,
            "Network-accessible without API key — anyone on your network can control your lights"
        );
    }

    let mut daemon_state = DaemonState::initialize(&config, config_path)?;
    daemon_state.start().await?;

    let ui_dir = resolve_ui_dir(options.ui_dir);
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

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        if !*shutdown_rx.borrow() {
            let _ = shutdown_rx.changed().await;
        }
        info!("Shutdown signal received, stopping API server");
    })
    .await
    .context("API server error")?;

    if let Some(publisher) = mdns_publisher {
        publisher.shutdown().await;
    }

    daemon_state.shutdown().await?;

    info!("Hypercolor daemon exited cleanly");
    Ok(())
}

fn resolve_ui_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    let explicit_provided = explicit.is_some();
    let path = explicit.or_else(|| {
        let candidate = PathBuf::from("crates/hypercolor-ui/dist");
        candidate.join("index.html").exists().then_some(candidate)
    })?;

    let index = path.join("index.html");
    let age = index
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.elapsed().ok());

    let age_label = match age {
        Some(elapsed) => format_age(elapsed),
        None => "unknown age".to_string(),
    };

    let source = if explicit_provided {
        "configured"
    } else {
        "auto-discovered"
    };

    if age.is_some_and(|elapsed| elapsed > std::time::Duration::from_secs(7 * 24 * 60 * 60)) {
        warn!(
            path = %path.display(),
            built = %age_label,
            "Serving stale web UI ({source}); rebuild with `just ui-build`"
        );
    } else {
        info!(
            path = %path.display(),
            built = %age_label,
            "Serving web UI ({source})"
        );
    }

    Some(path)
}

fn format_age(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(target_os = "linux")]
fn notify_ready() {
    if let Err(error) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
        tracing::warn!("failed to notify systemd: {error}");
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
    // Hypercolor's mDNS wrapper still logs normally.
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
}
