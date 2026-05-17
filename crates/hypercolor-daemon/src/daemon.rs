//! Foreground daemon runtime shared by console and service entry points.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use hypercolor_types::config::{
    HypercolorConfig, LogLevel, RenderAccelerationMode, ServoGpuImportMode,
};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::api::{self, AppState};
use crate::mdns::MdnsPublisher;
use crate::startup::{DaemonState, load_config};

const MAIN_RUNTIME_WORKERS: usize = 4;
const MAIN_RUNTIME_MAX_BLOCKING_THREADS: usize = 8;
const MAIN_RUNTIME_THREAD_KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(2);
const API_LISTEN_BACKLOG: i32 = 1024;
const API_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Runtime options for one daemon process.
#[derive(Clone, Debug, Default)]
pub struct DaemonRunOptions {
    /// Path to the configuration file.
    pub config: Option<PathBuf>,
    /// Address and port to bind the API server to.
    pub bind: Option<String>,
    /// Host/interface to bind using the configured daemon port.
    pub listen_address: Option<String>,
    /// Bind the API server to every network interface.
    pub listen_all: bool,
    /// Log level override.
    pub log_level: Option<String>,
    /// Compositor acceleration override.
    pub compositor_acceleration_mode: Option<RenderAccelerationMode>,
    /// Servo Linux GPU import override.
    pub servo_gpu_import_mode: Option<ServoGpuImportMode>,
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
pub async fn run(options: DaemonRunOptions, shutdown_rx: watch::Receiver<bool>) -> Result<()> {
    // Load configuration before tracing so we can honor config-driven log
    // levels when the CLI flag is omitted.
    let (mut config, config_path) = load_config(options.config.as_deref()).await?;
    if let Some(mode) = options.compositor_acceleration_mode {
        config.effect_engine.compositor_acceleration_mode = mode;
    }
    if let Some(mode) = options.servo_gpu_import_mode {
        config.rendering.servo_gpu_import.mode = mode;
    }
    let log_level = resolve_log_level(options.log_level.as_deref(), &config);

    // Initialize tracing with the requested log level + SilkCircuit theme.
    // The `RUST_LOG` env var takes precedence if set.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_env_filter(&log_level)));

    crate::startup::logging::install(env_filter);

    let requested_listen_targets = effective_bind_targets(&options, &config);
    let control_api_key_configured = api::security::control_api_key_configured_from_env();
    let (listen_targets, fell_back_to_loopback) = effective_startup_bind_targets(
        &options,
        &config,
        control_api_key_configured,
        config.network.allow_unauthenticated_remote_access,
    );
    if fell_back_to_loopback {
        warn!(
            requested = %requested_listen_targets.join(", "),
            effective = %listen_targets.join(", "),
            "Network listen config requires HYPERCOLOR_API_KEY; falling back to loopback"
        );
    }
    let listen_addr = listen_targets.join(", ");
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

    let binds = resolve_bind_targets(&listen_targets).await?;
    for bind in &binds {
        validate_network_bind_auth(
            *bind,
            control_api_key_configured,
            config.network.allow_unauthenticated_remote_access,
        )?;
    }
    let listeners = bind_api_listeners(&binds)?;
    let advertised_bind = listeners
        .first()
        .context("no API listeners were bound")?
        .local_addr()
        .context("failed to read API listener address")?;

    let mut daemon_state = DaemonState::initialize(&config, config_path)?;
    daemon_state.start().await?;

    let ui_dir = resolve_ui_dir(options.ui_dir);
    let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
    let router = api::build_router(app_state, ui_dir.as_deref());

    let mdns_publisher = MdnsPublisher::new(
        &daemon_state.server_identity,
        advertised_bind,
        config.network.mdns_publish,
        api::security::api_auth_required_from_env(),
    )?;

    if ui_dir.is_some() {
        info!(url = %format!("http://{advertised_bind}/"), "Web UI available");
    }
    info!(binds = %listen_addr, "API server listening");

    notify_ready();
    spawn_watchdog();

    serve_api_listeners(listeners, router, shutdown_rx).await?;

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

    if age.is_some_and(|elapsed| elapsed > std::time::Duration::from_hours(168)) {
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

async fn resolve_bind_targets(targets: &[String]) -> Result<Vec<SocketAddr>> {
    let mut resolved = Vec::new();

    for target in targets {
        let bind = resolve_socket_addr(target).await?;
        if !resolved.contains(&bind) {
            resolved.push(bind);
        }
    }

    Ok(resolved)
}

fn bind_api_listeners(binds: &[SocketAddr]) -> Result<Vec<TcpListener>> {
    let mut listeners = Vec::with_capacity(binds.len());

    for bind in binds {
        let listener = bind_api_listener(*bind)
            .with_context(|| format!("failed to bind API server to {bind}"))?;
        listeners.push(listener);
    }

    Ok(listeners)
}

/// Construct one API TCP listener with the daemon's socket options.
///
/// # Errors
///
/// Returns an error when the socket cannot be created, configured, bound,
/// listened on, or converted into a Tokio listener.
#[doc(hidden)]
pub fn bind_api_listener(bind: SocketAddr) -> Result<TcpListener> {
    let socket = Socket::new(
        if bind.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        },
        Type::STREAM,
        Some(Protocol::TCP),
    )?;

    if bind.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;

    socket.bind(&bind.into())?;
    socket.listen(API_LISTEN_BACKLOG)?;

    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener).context("failed to create async TCP listener")
}

async fn serve_api_listeners(
    listeners: Vec<TcpListener>,
    router: Router,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    serve_api_listeners_with_shutdown_timeout(
        listeners,
        router,
        shutdown_rx,
        API_GRACEFUL_SHUTDOWN_TIMEOUT,
    )
    .await
}

/// Serve pre-bound API listeners with a configurable shutdown drain timeout.
///
/// # Errors
///
/// Returns an error if any listener task fails before shutdown completes.
#[doc(hidden)]
pub async fn serve_api_listeners_with_shutdown_timeout(
    listeners: Vec<TcpListener>,
    router: Router,
    shutdown_rx: watch::Receiver<bool>,
    shutdown_timeout: Duration,
) -> Result<()> {
    let mut servers = JoinSet::new();

    for listener in listeners {
        let bind = listener
            .local_addr()
            .context("failed to read API listener address")?;
        let router = router.clone();
        let shutdown_wait_rx = shutdown_rx.clone();
        let shutdown_deadline_rx = shutdown_rx.clone();

        servers.spawn(async move {
            let server = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(wait_for_api_shutdown_signal(bind, shutdown_wait_rx))
            .into_future();

            tokio::pin!(server);
            tokio::select! {
                result = &mut server => {
                    result.with_context(|| format!("API server error on {bind}"))
                }
                () = api_shutdown_deadline(bind, shutdown_deadline_rx, shutdown_timeout) => {
                    warn!(
                        bind = %bind,
                        timeout_ms = shutdown_timeout.as_millis(),
                        "API graceful shutdown timed out; forcing listener close"
                    );
                    Ok(())
                }
            }
        });
    }

    while let Some(result) = servers.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                servers.abort_all();
                return Err(error);
            }
            Err(error) => {
                servers.abort_all();
                return Err(error).context("API server task failed");
            }
        }
    }

    Ok(())
}

async fn wait_for_api_shutdown_signal(bind: SocketAddr, mut shutdown_rx: watch::Receiver<bool>) {
    if !*shutdown_rx.borrow() {
        let _ = shutdown_rx.changed().await;
    }
    info!(bind = %bind, "Shutdown signal received, stopping API server");
}

async fn api_shutdown_deadline(
    bind: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
    shutdown_timeout: Duration,
) {
    if !*shutdown_rx.borrow() {
        let _ = shutdown_rx.changed().await;
    }
    sleep(shutdown_timeout).await;
    info!(bind = %bind, "API shutdown drain deadline reached");
}

/// Validate that network-reachable binds require control-tier authentication.
///
/// # Errors
///
/// Returns an error when `bind` is non-loopback and no control API key is configured.
pub fn validate_network_bind_auth(
    bind: SocketAddr,
    control_api_key_configured: bool,
    allow_unauthenticated_remote_access: bool,
) -> Result<()> {
    if bind.ip().is_loopback() || control_api_key_configured || allow_unauthenticated_remote_access
    {
        return Ok(());
    }

    bail!(
        "refusing to bind Hypercolor control API to {bind} without HYPERCOLOR_API_KEY; \
         set HYPERCOLOR_API_KEY, bind to a loopback address, or set \
         network.allow_unauthenticated_remote_access = true"
    );
}

#[must_use]
pub fn effective_bind_target(options: &DaemonRunOptions, config: &HypercolorConfig) -> String {
    effective_bind_targets(options, config)
        .into_iter()
        .next()
        .expect("effective bind targets should never be empty")
}

#[must_use]
pub fn effective_bind_targets(
    options: &DaemonRunOptions,
    config: &HypercolorConfig,
) -> Vec<String> {
    if let Some(bind) = options.bind.as_deref() {
        return expand_bind_target(bind);
    }

    let hosts = if options.listen_all {
        all_interface_hosts()
    } else if let Some(host) = options.listen_address.as_deref() {
        expand_listen_host(host)
    } else if config.network.remote_access && is_loopback_host(&config.daemon.listen_address) {
        all_interface_hosts()
    } else {
        expand_listen_host(&config.daemon.listen_address)
    };

    hosts
        .into_iter()
        .map(|host| format_bind_target(&host, config.daemon.port))
        .collect()
}

#[must_use]
pub fn effective_startup_bind_targets(
    options: &DaemonRunOptions,
    config: &HypercolorConfig,
    control_api_key_configured: bool,
    allow_unauthenticated_remote_access: bool,
) -> (Vec<String>, bool) {
    let targets = effective_bind_targets(options, config);
    if control_api_key_configured
        || allow_unauthenticated_remote_access
        || has_explicit_bind_override(options)
    {
        return (targets, false);
    }

    if targets.iter().any(|target| bind_target_needs_auth(target)) {
        return (loopback_bind_targets(config.daemon.port), true);
    }

    (targets, false)
}

fn has_explicit_bind_override(options: &DaemonRunOptions) -> bool {
    options.bind.is_some() || options.listen_address.is_some() || options.listen_all
}

fn bind_target_needs_auth(target: &str) -> bool {
    let normalized = normalize_bind_target(target);
    let Some((host, _)) = split_bind_host_port(&normalized) else {
        return true;
    };

    let host = unbracket_host(host);
    !is_loopback_host(host)
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
    let trimmed = normalize_listen_host(host);
    if trimmed.eq_ignore_ascii_case("localhost") {
        return true;
    }

    trimmed
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn normalize_bind_target(bind: &str) -> String {
    let trimmed = bind.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some((host, suffix)) = rest.split_once(']')
    {
        return format!("[{}]{suffix}", normalize_listen_host(host));
    }

    if let Some((host, port)) = trimmed.rsplit_once(':')
        && !host.contains(':')
    {
        return format!("{}:{port}", normalize_listen_host(host));
    }

    normalize_listen_host(trimmed)
}

fn expand_bind_target(bind: &str) -> Vec<String> {
    let normalized = normalize_bind_target(bind);
    let Some((host, port)) = split_bind_host_port(&normalized) else {
        return vec![normalized];
    };

    let host = unbracket_host(host);
    if host.eq_ignore_ascii_case("localhost") {
        return vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
    }

    let additional_target = if host == "127.0.0.1" {
        Some(format!("[::1]:{port}"))
    } else if host == all_interfaces_host() {
        Some(format!("[::]:{port}"))
    } else {
        None
    };

    if let Some(target) = additional_target {
        vec![normalized, target]
    } else {
        vec![normalized]
    }
}

fn split_bind_host_port(bind: &str) -> Option<(&str, &str)> {
    if let Some(rest) = bind.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']')?;
        let port = suffix.strip_prefix(':')?;
        return Some((host, port));
    }

    bind.rsplit_once(':')
}

fn normalize_listen_host(host: &str) -> String {
    let trimmed = unbracket_host(host.trim());
    let lower = trimmed.to_ascii_lowercase();

    match lower.as_str() {
        "all" | "any" | "*" => all_interfaces_host().to_owned(),
        "local" | "loopback" => "127.0.0.1".to_owned(),
        "all6" | "any6" | "ipv6" => "::".to_owned(),
        "local6" | "loopback6" | "ipv6-loopback" => "::1".to_owned(),
        _ => trimmed.to_owned(),
    }
}

const fn all_interfaces_host() -> &'static str {
    "0.0.0.0"
}

fn all_interface_hosts() -> Vec<String> {
    vec![all_interfaces_host().to_owned(), "::".to_owned()]
}

fn loopback_bind_targets(port: u16) -> Vec<String> {
    vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")]
}

fn expand_listen_host(host: &str) -> Vec<String> {
    let normalized = normalize_listen_host(host);
    if normalized.eq_ignore_ascii_case("localhost") || normalized == "127.0.0.1" {
        return vec!["127.0.0.1".to_owned(), "::1".to_owned()];
    }
    if normalized == all_interfaces_host() {
        return all_interface_hosts();
    }

    vec![normalized]
}

fn format_bind_target(host: &str, port: u16) -> String {
    if host
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_ipv6())
    {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn unbracket_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host)
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
