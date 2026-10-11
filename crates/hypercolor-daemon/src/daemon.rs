//! Foreground daemon runtime shared by console and service entry points.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use hypercolor_core::config::{BootConfig, ConfigManager, LoadedConfig};
use hypercolor_core::session::SessionMonitor;
use hypercolor_types::api::system::DaemonStartupPhase;
use hypercolor_types::config::{
    HypercolorConfig, LogLevel, NetworkAccessMode, RenderAccelerationMode, ServoGpuImportMode,
};
use hypercolor_types::service::ServiceStatus;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::api;
use crate::api::security::{CredentialAuthority, CredentialTier, RemoteClientFamilies};
use crate::app_state::AppState;
use crate::macos_owner::{MacosDaemonOwner, MacosDaemonSessionAttestation, MacosOwnerSnapshot};
use crate::mdns::MdnsPublisher;
use crate::startup::{DaemonState, StartupProgress, config_sources};

const MAIN_RUNTIME_WORKERS: usize = 4;
const MAIN_RUNTIME_MAX_BLOCKING_THREADS: usize = 8;
const MAIN_RUNTIME_THREAD_KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(2);
const API_LISTEN_BACKLOG: i32 = 1024;
const API_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Runtime options for one daemon process.
#[derive(Default)]
pub struct DaemonRunOptions {
    /// Path to the configuration file.
    pub config: Option<PathBuf>,
    /// Address and port to bind the API server to.
    pub bind: Option<String>,
    /// Port that replaces `daemon.port` for this launch.
    ///
    /// It never picks interfaces. Unless `listen_address` or `listen_all`
    /// does, the configured network mode picks them, and loopback stays
    /// reachable on this port so a local launcher can always find the
    /// daemon: a configured network address that cannot be resolved or
    /// bound is dropped with a warning instead of failing startup. An
    /// explicit [`bind`](Self::bind) wins over it.
    pub port: Option<u16>,
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
    /// Bundled effects directory, overriding the install layout.
    pub effects_dir: Option<PathBuf>,
    /// Explicit macOS daemon topology supplied by the local launcher.
    pub macos_owner: Option<MacosDaemonOwner>,
    /// Durable ownership snapshot published before input source construction.
    pub macos_owner_snapshot: Option<MacosOwnerSnapshot>,
    /// Exact private process session derived from canonical macOS ownership.
    pub macos_daemon_session_attestation: Option<MacosDaemonSessionAttestation>,
    /// Corroborated launcher identity resolved by the process host before
    /// runtime startup. On macOS the durable owner snapshot supersedes it.
    pub service_status: Option<ServiceStatus>,
    /// Platform session monitors supplied by the process host.
    pub session_monitors: Option<Vec<Box<dyn SessionMonitor>>>,
    /// API credentials beyond the environment keys, supplied by a
    /// downstream build. It is needed before any listener is bound,
    /// because it decides whether a network bind is allowed.
    pub credential_authority: Option<Arc<dyn CredentialAuthority>>,
}

/// Ownership handle for the exact sockets bound during daemon preparation.
///
/// Each handle is a duplicate descriptor for the same listening socket used
/// by Tokio. Keeping the lease alive prevents another process from binding the
/// API address after serving stops and before process-level authority is
/// invalidated.
#[doc(hidden)]
pub struct ApiListenerLease {
    _listeners: Vec<std::net::TcpListener>,
}

/// Daemon startup state whose final API sockets are already bound.
#[doc(hidden)]
pub struct PreparedDaemon {
    options: DaemonRunOptions,
    config: BootConfig,
    config_manager: Arc<ConfigManager>,
    listen_addr: String,
    listeners: Vec<TcpListener>,
    listener_lease: Option<ApiListenerLease>,
    advertised_bind: SocketAddr,
}

impl PreparedDaemon {
    /// Resume a prepared daemon using its already-bound API listeners.
    ///
    /// # Errors
    ///
    /// Returns an error when subsystem startup, serving, or shutdown fails.
    pub async fn run(self, shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        Box::pin(self.run_with_extensions(shutdown_rx, &[])).await
    }

    /// Return the primary address owned by this prepared daemon.
    #[must_use]
    pub const fn advertised_bind(&self) -> SocketAddr {
        self.advertised_bind
    }

    /// The address each prepared API listener bound.
    ///
    /// # Errors
    ///
    /// Returns an error when a listener cannot report its address.
    #[doc(hidden)]
    pub fn api_listen_addresses(&self) -> Result<Vec<SocketAddr>> {
        listener_addresses(&self.listeners)
    }

    /// Attach the exact macOS process session published after socket binding.
    pub fn install_macos_daemon_session_attestation(
        &mut self,
        attestation: MacosDaemonSessionAttestation,
    ) {
        self.options.macos_daemon_session_attestation = Some(attestation);
    }

    /// Transfer the socket lifetime lease to the process-level owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the lease was already transferred.
    pub fn take_api_listener_lease(&mut self) -> Result<ApiListenerLease> {
        self.listener_lease
            .take()
            .context("prepared API listener lease was already transferred")
    }

    pub(crate) async fn run_with_extensions(
        mut self,
        shutdown_rx: watch::Receiver<bool>,
        extension_installers: &[&dyn DaemonExtensionInstaller],
    ) -> Result<()> {
        let macos_daemon_session_attestation =
            self.options.macos_daemon_session_attestation.clone();
        let credential_authority = self.options.credential_authority.clone();
        let listeners = std::mem::take(&mut self.listeners);
        let api_listen_addresses = listener_addresses(&listeners)?;

        // Answer `/health` from the first moment of startup, so a
        // supervisor sees a starting daemon make progress instead of a
        // socket that never responds. The full router replaces this
        // surface on the same listeners once startup finishes.
        let progress = StartupProgress::default();
        let handoff =
            api::startup::ApiHandoff::starting(progress.clone(), env!("CARGO_PKG_VERSION"));
        let server = ApiServerTask::spawn(serve_api_handoff_with_shutdown_timeout(
            listeners,
            handoff.clone(),
            shutdown_rx,
            API_GRACEFUL_SHUTDOWN_TIMEOUT,
        ));
        info!(binds = %self.listen_addr, "API listeners answering startup probes");

        // Boot values are frozen into the subsystems that need them by this
        // call, which consumes the config; anything read past this point
        // reads live (Spec 76 §3.2).
        let mut daemon_state = DaemonState::initialize_with_progress(
            self.config,
            self.config_manager,
            self.options.macos_owner_snapshot,
            self.options.service_status.take(),
            progress.clone(),
        )?;
        let ui_dir = resolve_ui_dir(self.options.ui_dir.clone());
        daemon_state.session_monitors = self.options.session_monitors.take();
        record_api_binding(
            &mut daemon_state,
            api_listen_addresses,
            has_explicit_bind_override(&self.options),
        );
        install_extensions(&mut daemon_state, ui_dir.clone(), extension_installers)?;
        Box::pin(daemon_state.start()).await?;

        progress.enter(DaemonStartupPhase::PreparingApi);
        let app_state = Arc::new(api::build_state(
            &daemon_state,
            macos_daemon_session_attestation.as_ref(),
            credential_authority,
        ));
        let api_auth_required = app_state.security_state.security_enabled();
        daemon_state.domains.display.sync_connected_surfaces().await;
        daemon_state
            .domains
            .display
            .sync_preference_overlays()
            .await;
        if let Err(error) = notify_api_ready_extensions(&daemon_state, &app_state, &progress).await
        {
            if let Err(shutdown_error) = daemon_state.shutdown().await {
                warn!(%shutdown_error, "Failed to roll back daemon after API-ready hook failure");
            }
            return Err(error);
        }
        // Serve the full API before advertising it, so a LAN client that
        // reacts to the mDNS announcement never lands on the startup surface.
        handoff.install(api::build_router(app_state, ui_dir.as_deref()));

        let mdns_publish = daemon_state.config_manager.live().network.mdns_publish;
        let mdns_publisher = MdnsPublisher::new(
            &daemon_state.server_identity,
            self.advertised_bind,
            mdns_publish,
            api_auth_required,
        )?;

        if ui_dir.is_some() {
            info!(url = %format!("http://{}/", self.advertised_bind), "Web UI available");
        }
        info!(
            binds = %self.listen_addr,
            startup_ms = u64::try_from(progress.elapsed().as_millis()).unwrap_or(u64::MAX),
            "API server listening"
        );

        hypercolor_linux_session::notify_ready();
        hypercolor_linux_session::spawn_watchdog();

        server.join().await?;

        if let Some(publisher) = mdns_publisher {
            publisher.shutdown().await;
        }

        daemon_state.shutdown().await?;

        info!("Hypercolor daemon exited cleanly");
        Ok(())
    }
}

/// Publish where the API listens, before installers read it.
fn record_api_binding(
    daemon: &mut DaemonState,
    addresses: Vec<SocketAddr>,
    overridden_at_launch: bool,
) {
    daemon.api_listen_addresses = addresses;
    daemon.api_bind_overridden_at_launch = overridden_at_launch;
}

fn install_extensions(
    daemon: &mut DaemonState,
    ui_dir: Option<PathBuf>,
    extension_installers: &[&dyn DaemonExtensionInstaller],
) -> Result<()> {
    daemon.ui_dir = ui_dir;
    for installer in extension_installers {
        installer.install(daemon)?;
    }
    Ok(())
}

pub trait DaemonExtensionInstaller: Send + Sync {
    /// Install extension state, API routes, and lifecycle hooks before startup.
    ///
    /// # Errors
    ///
    /// Returns an error when the extension cannot register itself.
    fn install(&self, daemon: &mut DaemonState) -> Result<()>;

    /// The credential authority this extension supplies, if any.
    ///
    /// Read before any listener is bound, ahead of [`install`](Self::install),
    /// so it must be cheap and perform no I/O: return the authority object
    /// here and load its state in `install`. No request reaches the
    /// authority before the full router is served.
    fn credential_authority(&self) -> Option<Arc<dyn CredentialAuthority>> {
        None
    }
}

/// Adopt the credential authority an installer supplies.
///
/// Options that already carry an authority keep it and the installers are
/// not consulted. Every process path calls this before
/// [`prepare`](crate::daemon::prepare), which decides the bind.
///
/// # Errors
///
/// Returns an error when more than one installer supplies an authority.
pub fn adopt_credential_authority(
    options: &mut DaemonRunOptions,
    extension_installers: &[&dyn DaemonExtensionInstaller],
) -> Result<()> {
    if options.credential_authority.is_some() {
        return Ok(());
    }
    let mut supplied = extension_installers
        .iter()
        .filter_map(|installer| installer.credential_authority());
    let authority = supplied.next();
    if supplied.next().is_some() {
        bail!("more than one daemon extension supplies a credential authority");
    }
    options.credential_authority = authority;
    Ok(())
}

/// Whether the run options carry a credential authority that can grant
/// control, which satisfies a network bind the way `HYPERCOLOR_API_KEY`
/// does.
#[must_use]
pub fn credential_authority_grants_control(options: &DaemonRunOptions) -> bool {
    options
        .credential_authority
        .as_ref()
        .is_some_and(|authority| authority.ceiling() == CredentialTier::Control)
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
    Box::pin(run_with_extensions(options, shutdown_rx, &[])).await
}

/// Run the daemon with downstream extension installers.
///
/// # Errors
///
/// Returns an error when startup, extension installation, serving, or graceful
/// shutdown fails.
pub async fn run_with_extensions(
    mut options: DaemonRunOptions,
    shutdown_rx: watch::Receiver<bool>,
    extension_installers: &[&dyn DaemonExtensionInstaller],
) -> Result<()> {
    adopt_credential_authority(&mut options, extension_installers)?;
    let prepared = prepare(options).await?;
    Box::pin(prepared.run_with_extensions(shutdown_rx, extension_installers)).await
}

/// Load configuration and bind every final API listener without starting the
/// daemon subsystems or accepting connections.
///
/// # Errors
///
/// Returns an error when configuration, address resolution, authentication
/// validation, or any final listener bind fails.
#[doc(hidden)]
pub async fn prepare(options: DaemonRunOptions) -> Result<PreparedDaemon> {
    // Must land before any registry scan, which resolves the bundled catalog
    // the first time it enumerates effects.
    if options.effects_dir.is_some() {
        hypercolor_core::effect::set_bundled_effects_root(options.effects_dir.clone());
    }

    // Load configuration before tracing so we can honor config-driven log
    // levels when the CLI flag is omitted.
    let LoadedConfig {
        boot: config,
        manager,
        ..
    } = ConfigManager::load_with_sources(config_sources(
        options.config.clone(),
        options.compositor_acceleration_mode,
        options.servo_gpu_import_mode,
    ))?;
    let config_manager = Arc::new(manager);
    info!(path = %config_manager.path().display(), "Resolved config path");
    let log_level = resolve_log_level(options.log_level.as_deref(), &config);

    // Initialize tracing with the requested log level + SilkCircuit theme.
    // The `RUST_LOG` env var takes precedence if set.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_env_filter(&log_level)));

    crate::startup::logging::install(env_filter);

    let requested_listen_targets = effective_bind_targets(&options, &config);
    let control_credentials_configured = api::security::control_api_key_configured_from_env()
        || credential_authority_grants_control(&options);
    let (listen_targets, fell_back_to_loopback) = effective_startup_bind_targets(
        &options,
        &config,
        control_credentials_configured,
        config.network.unauthenticated_remote_access_allowed(),
    );
    if fell_back_to_loopback {
        warn!(
            requested = %requested_listen_targets.join(", "),
            effective = %listen_targets.join(", "),
            "Network listen config requires control credentials (HYPERCOLOR_API_KEY or a \
             credential authority); falling back to loopback"
        );
    }
    info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = ?options.bind,
        port = ?options.port,
        log_level = %log_level,
        "Hypercolor daemon starting"
    );

    info!(
        schema_version = config.schema_version,
        target_fps = config.daemon.target_fps,
        "Configuration ready"
    );

    let bound = bind_startup_listeners(
        &listen_targets,
        loopback_fallback_port(&options, &config),
        |bind| {
            validate_network_bind_auth(
                bind,
                control_credentials_configured,
                config.network.unauthenticated_remote_access_allowed(),
            )
        },
    )
    .await?;
    for dropped in &bound.dropped {
        warn!(
            target = %dropped.target,
            error = %dropped.error,
            "Configured network listen address is unavailable; serving without it so \
             loopback stays reachable, and skipping mDNS for this run"
        );
    }
    let BoundApiListeners {
        listeners,
        lease: listener_lease,
        addresses,
        dropped,
    } = bound;
    // After a degraded bind, advertise loopback, which keeps mDNS off: a
    // surviving wildcard of one family would otherwise announce host
    // addresses of the other family this daemon does not serve.
    let advertised_bind =
        advertised_bind(&addresses, !dropped.is_empty()).context("no API listeners were bound")?;
    let listen_addr = addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    crate::startup::banner::print(
        env!("CARGO_PKG_VERSION"),
        (config.daemon.canvas_width, config.daemon.canvas_height),
        &listen_addr,
    );
    let credentials_required =
        api::security::api_auth_required_from_env() || options.credential_authority.is_some();
    let keyless = keyless_network_listeners(
        &addresses,
        if credentials_required {
            RemoteClientFamilies::NONE
        } else {
            api::security::network_policy_remote_client_families(&config.network)
        },
    );
    if !keyless.is_empty() {
        warn!(
            listen = %keyless
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            "API is listening on the network without an API key: devices on the local \
             network can control this daemon. Set HYPERCOLOR_API_KEY, or switch \
             network.access_mode to lan_protected, to require one"
        );
    }

    Ok(PreparedDaemon {
        options,
        config,
        config_manager,
        listen_addr,
        listeners,
        listener_lease: Some(listener_lease),
        advertised_bind,
    })
}

async fn notify_api_ready_extensions(
    daemon: &DaemonState,
    state: &Arc<AppState>,
    progress: &StartupProgress,
) -> Result<()> {
    for extension in daemon.lifecycle_extensions.clone() {
        info!(
            extension = extension.name(),
            "Starting API-ready daemon extension hook"
        );
        progress
            .step_async(
                &format!("daemon extension {} API hook", extension.name()),
                extension.api_ready(daemon, Arc::clone(state)),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to start API-ready hook for daemon extension {}",
                    extension.name()
                )
            })?;
    }
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
        // Reproducible package stores (Nix, Guix) normalize every mtime to
        // one second past the Unix epoch, which would read as decades stale.
        // Treat anything that early as an unknown build time rather than a
        // rebuild nag on every boot.
        .filter(|modified| *modified > std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
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

/// A listen target a `--port` launch dropped because it could not be
/// resolved or bound.
#[derive(Debug)]
struct DroppedListenTarget {
    target: String,
    error: String,
}

/// The API listeners one launch bound.
struct BoundApiListeners {
    listeners: Vec<TcpListener>,
    lease: ApiListenerLease,
    /// The address each listener in `listeners` bound, in the same order.
    addresses: Vec<SocketAddr>,
    dropped: Vec<DroppedListenTarget>,
}

/// The loopback port a launch falls back to when a config-chosen network
/// target is unavailable: the `--port` launch port, when config picks the
/// interfaces. Every other launch fails on an unavailable target.
fn loopback_fallback_port(options: &DaemonRunOptions, config: &HypercolorConfig) -> Option<u16> {
    (options.port.is_some() && !has_explicit_bind_override(options))
        .then(|| effective_port(options, config))
}

/// The bound API addresses another host can control without presenting any
/// credential: the non-loopback addresses of a family in which
/// `open_families` admits a remote client. Pass
/// [`RemoteClientFamilies::NONE`] when an API key or credential authority
/// makes the API require a credential.
fn keyless_network_listeners(
    addresses: &[SocketAddr],
    open_families: RemoteClientFamilies,
) -> Vec<SocketAddr> {
    addresses
        .iter()
        .filter(|address| {
            !address.ip().is_loopback() && open_families.admits_family_of(address.ip())
        })
        .copied()
        .collect()
}

/// The address the daemon advertises: the first listener, or after a
/// degraded bind the first loopback listener, which mDNS never publishes.
fn advertised_bind(addresses: &[SocketAddr], degraded: bool) -> Option<SocketAddr> {
    if degraded {
        addresses
            .iter()
            .find(|address| address.ip().is_loopback())
            .or_else(|| addresses.first())
            .copied()
    } else {
        addresses.first().copied()
    }
}

/// The address each listener actually bound, in order.
fn listener_addresses(listeners: &[TcpListener]) -> Result<Vec<SocketAddr>> {
    listeners
        .iter()
        .map(TcpListener::local_addr)
        .collect::<std::io::Result<Vec<_>>>()
        .context("failed to read API listener address")
}

/// Resolve, authorize, and bind the API listeners for `targets`.
///
/// With a `loopback_fallback` port, a network target that fails to resolve
/// or bind (a stale interface address, one DHCP has not assigned yet, a
/// hostname that does not resolve) is dropped instead of aborting startup,
/// and loopback on that port is bound wherever no remaining listener serves
/// it. A local launcher can then still reach the daemon, including the
/// settings that would fix the address. Loopback failures, `authorize`
/// refusals, and every failure without a fallback port still abort.
async fn bind_startup_listeners(
    targets: &[String],
    loopback_fallback: Option<u16>,
    authorize: impl Fn(SocketAddr) -> Result<()>,
) -> Result<BoundApiListeners> {
    let mut dropped = Vec::new();
    let mut resolved = Vec::new();
    for target in targets {
        match resolve_socket_addr(target).await {
            Ok(bind) => {
                if !resolved.contains(&bind) {
                    resolved.push(bind);
                }
            }
            Err(error) if loopback_fallback.is_some() && bind_target_needs_auth(target) => {
                dropped.push(DroppedListenTarget {
                    target: target.clone(),
                    error: format!("{error:#}"),
                });
            }
            Err(error) => return Err(error),
        }
    }
    for bind in &resolved {
        authorize(*bind)?;
    }

    let mut listeners = Vec::with_capacity(resolved.len());
    let mut leases = Vec::with_capacity(resolved.len());
    for bind in resolved {
        match bind_api_listener_with_lease(bind) {
            Ok((listener, lease)) => {
                listeners.push(listener);
                leases.push(lease);
            }
            Err(error) if loopback_fallback.is_some() && !bind.ip().is_loopback() => {
                dropped.push(DroppedListenTarget {
                    target: bind.to_string(),
                    error: format!("{error:#}"),
                });
            }
            Err(error) => {
                return Err(error.context(format!("failed to bind API server to {bind}")));
            }
        }
    }

    if let Some(port) = loopback_fallback {
        let served = listener_addresses(&listeners)?
            .iter()
            .map(SocketAddr::ip)
            .collect::<Vec<_>>();
        for bind in unserved_loopback(&served, port) {
            let (listener, lease) = bind_api_listener_with_lease(bind)
                .with_context(|| format!("failed to bind API server to {bind}"))?;
            listeners.push(listener);
            leases.push(lease);
        }
    }

    let addresses = listener_addresses(&listeners)?;
    Ok(BoundApiListeners {
        listeners,
        lease: ApiListenerLease { _listeners: leases },
        addresses,
        dropped,
    })
}

/// Construct one API TCP listener with the daemon's socket options.
///
/// # Errors
///
/// Returns an error when the socket cannot be created, configured, bound,
/// listened on, or converted into a Tokio listener.
#[doc(hidden)]
pub fn bind_api_listener(bind: SocketAddr) -> Result<TcpListener> {
    bind_api_listener_with_lease(bind).map(|(listener, _lease)| listener)
}

fn bind_api_listener_with_lease(bind: SocketAddr) -> Result<(TcpListener, std::net::TcpListener)> {
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
    // SO_REUSEADDR means fast restart through TIME_WAIT on Unix, but on
    // Windows it lets a second socket bind over a live listener (port
    // hijack). Windows rebinds a closed listener without it.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;

    socket.bind(&bind.into())?;
    socket.listen(API_LISTEN_BACKLOG)?;

    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    let lease = listener
        .try_clone()
        .context("failed to duplicate API listener ownership handle")?;
    let listener =
        TcpListener::from_std(listener).context("failed to create async TCP listener")?;
    Ok((listener, lease))
}

/// The API serving task spawned at the start of startup.
///
/// Aborted when dropped, so a startup failure that returns early stops the
/// listeners instead of leaving them answering "starting" forever.
struct ApiServerTask(Option<tokio::task::JoinHandle<Result<()>>>);

impl ApiServerTask {
    fn spawn(serve: impl Future<Output = Result<()>> + Send + 'static) -> Self {
        Self(Some(tokio::spawn(serve)))
    }

    async fn join(mut self) -> Result<()> {
        let Some(handle) = self.0.take() else {
            return Ok(());
        };
        handle.await.context("API server task failed")?
    }
}

impl Drop for ApiServerTask {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
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
    serve_api_handoff_with_shutdown_timeout(
        listeners,
        api::startup::ApiHandoff::ready(router),
        shutdown_rx,
        shutdown_timeout,
    )
    .await
}

/// Serve pre-bound API listeners through a startup-to-ready handoff.
///
/// The listeners answer from the startup surface until the full router is
/// installed on `handoff`, then from the full router, without rebinding.
///
/// # Errors
///
/// Returns an error if any listener task fails before shutdown completes.
#[doc(hidden)]
pub async fn serve_api_handoff_with_shutdown_timeout(
    listeners: Vec<TcpListener>,
    handoff: api::startup::ApiHandoff,
    shutdown_rx: watch::Receiver<bool>,
    shutdown_timeout: Duration,
) -> Result<()> {
    let mut servers = JoinSet::new();

    for listener in listeners {
        let bind = listener
            .local_addr()
            .context("failed to read API listener address")?;
        let make_service = handoff.make_service();
        let shutdown_wait_rx = shutdown_rx.clone();
        let shutdown_deadline_rx = shutdown_rx.clone();

        servers.spawn(async move {
            let server = axum::serve(listener, make_service)
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
/// `control_credentials_configured` is true when `HYPERCOLOR_API_KEY` is
/// set or an installed credential authority can grant control.
///
/// # Errors
///
/// Returns an error when `bind` is non-loopback and no control credentials are configured.
pub fn validate_network_bind_auth(
    bind: SocketAddr,
    control_credentials_configured: bool,
    allow_unauthenticated_remote_access: bool,
) -> Result<()> {
    if bind.ip().is_loopback()
        || control_credentials_configured
        || allow_unauthenticated_remote_access
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

    let port = effective_port(options, config);
    let hosts = if options.listen_all {
        all_interface_hosts()
    } else if let Some(host) = options.listen_address.as_deref() {
        expand_listen_host(host)
    } else if config.network.access_mode == NetworkAccessMode::LocalOnly
        && !config.network.remote_access
    {
        return loopback_bind_targets(port);
    } else if config.network.remote_access_enabled()
        && is_loopback_host(&config.daemon.listen_address)
    {
        all_interface_hosts()
    } else {
        expand_listen_host(&config.daemon.listen_address)
    };

    let targets = hosts
        .into_iter()
        .map(|host| format_bind_target(&host, port))
        .collect();
    if options.port.is_some() && !has_explicit_bind_override(options) {
        // A launcher that passes only a port reaches the daemon over
        // loopback, so a config that names one specific interface must
        // not strand it.
        with_loopback_reachable(targets, port)
    } else {
        targets
    }
}

/// The port the API binds when no explicit `--bind` names one: the launch
/// override when present, otherwise `daemon.port`.
fn effective_port(options: &DaemonRunOptions, config: &HypercolorConfig) -> u16 {
    options.port.unwrap_or(config.daemon.port)
}

#[must_use]
pub fn effective_startup_bind_targets(
    options: &DaemonRunOptions,
    config: &HypercolorConfig,
    control_credentials_configured: bool,
    allow_unauthenticated_remote_access: bool,
) -> (Vec<String>, bool) {
    let targets = effective_bind_targets(options, config);
    if control_credentials_configured
        || allow_unauthenticated_remote_access
        || has_explicit_bind_override(options)
    {
        return (targets, false);
    }

    if targets.iter().any(|target| bind_target_needs_auth(target)) {
        return (loopback_bind_targets(effective_port(options, config)), true);
    }

    (targets, false)
}

/// Whether a launch flag picked the API interfaces instead of the
/// configured network mode. `--port` alone does not: it only moves the
/// port.
fn has_explicit_bind_override(options: &DaemonRunOptions) -> bool {
    options.bind.is_some() || options.listen_address.is_some() || options.listen_all
}

/// Append the loopback targets on `port` that `targets` does not already
/// serve.
fn with_loopback_reachable(mut targets: Vec<String>, port: u16) -> Vec<String> {
    let served: Vec<IpAddr> = targets
        .iter()
        .filter_map(|target| {
            let (host, _) = split_bind_host_port(target)?;
            unbracket_host(host).parse().ok()
        })
        .collect();
    targets.extend(
        unserved_loopback(&served, port)
            .iter()
            .map(ToString::to_string),
    );
    targets
}

/// The loopback addresses on `port` that no address in `served` answers. A
/// wildcard answers the loopback address of its own family.
fn unserved_loopback(served: &[IpAddr], port: u16) -> Vec<SocketAddr> {
    [
        (
            IpAddr::from(Ipv4Addr::LOCALHOST),
            IpAddr::from(Ipv4Addr::UNSPECIFIED),
        ),
        (
            IpAddr::from(Ipv6Addr::LOCALHOST),
            IpAddr::from(Ipv6Addr::UNSPECIFIED),
        ),
    ]
    .into_iter()
    .filter(|(loopback, wildcard)| {
        !served
            .iter()
            .any(|address| address == loopback || address == wildcard)
    })
    .map(|(loopback, _)| SocketAddr::new(loopback, port))
    .collect()
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
    use std::sync::{Arc, Mutex};

    use hypercolor_core::config::{BootConfig, ConfigManager};
    use hypercolor_types::config::{HypercolorConfig, LogLevel, RenderAccelerationMode};

    use super::{
        DaemonExtensionInstaller, DaemonRunOptions, RemoteClientFamilies, advertised_bind,
        bind_api_listener, bind_api_listener_with_lease, bind_startup_listeners,
        default_env_filter, has_explicit_bind_override, install_extensions,
        keyless_network_listeners, loopback_fallback_port, notify_api_ready_extensions,
        record_api_binding, resolve_log_level, serve_api_listeners_with_shutdown_timeout,
    };
    use crate::app_state::AppState;
    use crate::extensions::DaemonLifecycleExtension;
    use crate::startup::{DaemonState, default_config};

    /// Serializes the tests here that relocate the process-wide directories.
    static PATH_OVERRIDES: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    /// Relocates the config and data directories, and with them the state
    /// directory, into one fixture root for the life of the guard.
    struct PathOverrides {
        _lock: tokio::sync::MutexGuard<'static, ()>,
    }

    impl PathOverrides {
        async fn install(root: &std::path::Path) -> Self {
            let lock = PATH_OVERRIDES.lock().await;
            ConfigManager::set_config_dir_override(Some(root.join("config")));
            ConfigManager::set_data_dir_override(Some(root.join("data")));
            Self { _lock: lock }
        }
    }

    impl Drop for PathOverrides {
        fn drop(&mut self) {
            ConfigManager::set_data_dir_override(None);
            ConfigManager::set_config_dir_override(None);
        }
    }

    struct ApiReadyProbe {
        name: &'static str,
        expected_state: Arc<AppState>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct UiDirProbe(Arc<Mutex<Option<std::path::PathBuf>>>);

    type ObservedBinding = Option<(Vec<std::net::SocketAddr>, bool)>;

    struct BindingProbe(Arc<Mutex<ObservedBinding>>);

    impl DaemonExtensionInstaller for BindingProbe {
        fn install(&self, daemon: &mut DaemonState) -> anyhow::Result<()> {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
                daemon.api_listen_addresses().to_vec(),
                daemon.api_bind_overridden_at_launch(),
            ));
            Ok(())
        }
    }

    impl DaemonExtensionInstaller for UiDirProbe {
        fn install(&self, daemon: &mut DaemonState) -> anyhow::Result<()> {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                daemon.ui_dir().map(std::path::Path::to_path_buf);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DaemonLifecycleExtension for ApiReadyProbe {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn api_ready(
            &self,
            _daemon: &DaemonState,
            state: Arc<AppState>,
        ) -> anyhow::Result<()> {
            assert!(Arc::ptr_eq(&state, &self.expected_state));
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(self.name);
            Ok(())
        }
    }

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

    #[tokio::test]
    async fn exact_prebound_listener_is_served_and_leased_until_explicit_release() {
        let (listener, lease) = bind_api_listener_with_lease(
            "127.0.0.1:0"
                .parse()
                .expect("ephemeral loopback address should parse"),
        )
        .expect("listener and lease should bind together");
        let address = listener
            .local_addr()
            .expect("prepared listener address should resolve");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let router = axum::Router::new().route(
            "/listener-identity",
            axum::routing::get(|| async { "prepared-listener" }),
        );
        let server = tokio::spawn(serve_api_listeners_with_shutdown_timeout(
            vec![listener],
            router,
            shutdown_rx,
            tokio::time::Duration::from_secs(1),
        ));

        let response = reqwest::get(format!("http://{address}/listener-identity"))
            .await
            .expect("request should reach the prepared listener");
        assert_eq!(
            response.text().await.expect("response body should read"),
            "prepared-listener"
        );
        shutdown_tx
            .send(true)
            .expect("shutdown signal should reach the listener");
        server
            .await
            .expect("listener task should join")
            .expect("listener shutdown should succeed");

        bind_api_listener(address).expect_err("lease must keep the exact socket unavailable");
        drop(lease);
        let rebound =
            bind_api_listener(address).expect("dropping the lease should release the port");
        drop(rebound);
    }

    #[tokio::test]
    async fn api_ready_hooks_receive_the_serving_state_in_registration_order() {
        let directory = tempfile::tempdir().expect("daemon test directory should be created");
        let _paths = PathOverrides::install(directory.path()).await;
        let mut config = default_config();
        config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
        let config_manager = Arc::new(ConfigManager::from_config_unchecked(
            directory.path().join("hypercolor.toml"),
            config.clone(),
        ));
        let mut daemon =
            DaemonState::initialize(BootConfig::from_config_unchecked(config), config_manager)
                .expect("daemon test state should initialize");
        let state = Arc::new(AppState::new_with_data_dir(directory.path().join("api")));
        let calls = Arc::new(Mutex::new(Vec::new()));
        for name in ["first", "second"] {
            daemon.register_lifecycle_extension(Arc::new(ApiReadyProbe {
                name,
                expected_state: Arc::clone(&state),
                calls: Arc::clone(&calls),
            }));
        }

        let progress = crate::startup::StartupProgress::default();
        let before = progress.snapshot().sequence;
        notify_api_ready_extensions(&daemon, &state, &progress)
            .await
            .expect("API-ready hooks should succeed");
        assert_eq!(
            progress.snapshot().sequence - before,
            2,
            "each completed API-ready hook is one startup step"
        );

        assert_eq!(
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["first", "second"]
        );
    }

    #[tokio::test]
    async fn extension_installers_observe_the_same_resolved_ui_directory_as_the_router() {
        let directory = tempfile::tempdir().expect("daemon test directory should be created");
        let _paths = PathOverrides::install(directory.path()).await;
        let mut config = default_config();
        config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
        let config_manager = Arc::new(ConfigManager::from_config_unchecked(
            directory.path().join("hypercolor.toml"),
            config.clone(),
        ));
        let mut daemon =
            DaemonState::initialize(BootConfig::from_config_unchecked(config), config_manager)
                .expect("daemon test state should initialize");
        let observed = Arc::new(Mutex::new(None));
        let probe = UiDirProbe(Arc::clone(&observed));
        let ui_dir = directory.path().join("ui");

        install_extensions(&mut daemon, Some(ui_dir.clone()), &[&probe])
            .expect("extension installation should succeed");

        assert_eq!(daemon.ui_dir(), Some(ui_dir.as_path()));
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
            Some(ui_dir.as_path())
        );
    }

    #[test]
    fn port_override_alone_leaves_the_bind_to_config() {
        let port_only = DaemonRunOptions {
            port: Some(9555),
            ..DaemonRunOptions::default()
        };
        assert!(!has_explicit_bind_override(&port_only));

        for explicit in [
            DaemonRunOptions {
                bind: Some("127.0.0.1:9555".to_owned()),
                port: Some(9555),
                ..DaemonRunOptions::default()
            },
            DaemonRunOptions {
                listen_address: Some("192.168.1.42".to_owned()),
                port: Some(9555),
                ..DaemonRunOptions::default()
            },
            DaemonRunOptions {
                listen_all: true,
                port: Some(9555),
                ..DaemonRunOptions::default()
            },
        ] {
            assert!(has_explicit_bind_override(&explicit));
        }
    }

    /// A loopback port that was free a moment ago on both families.
    fn free_loopback_port() -> u16 {
        for _ in 0..16 {
            let v4 = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("an ephemeral IPv4 loopback port should be available");
            let port = v4
                .local_addr()
                .expect("listener address should resolve")
                .port();
            match std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)) {
                Ok(_) => return port,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(error) => panic!("IPv6 loopback is unavailable: {error}"),
            }
        }
        panic!("no port was free on both loopback families after 16 tries");
    }

    fn loopback_pair(port: u16) -> Vec<std::net::SocketAddr> {
        vec![
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
        ]
    }

    /// TEST-NET-1 (RFC 5737): never assigned to a real interface, so a bind
    /// fails the way a stale configured address does.
    const STALE_ADDRESS: &str = "192.0.2.1";

    #[tokio::test]
    async fn port_launch_drops_an_unavailable_configured_address_and_keeps_loopback() {
        let port = free_loopback_port();
        let stale = format!("{STALE_ADDRESS}:{port}");
        let targets = vec![
            stale.clone(),
            format!("127.0.0.1:{port}"),
            format!("[::1]:{port}"),
        ];

        let bound = bind_startup_listeners(&targets, Some(port), |_| Ok(()))
            .await
            .expect("a --port launch should survive an unavailable configured address");

        assert_eq!(bound.addresses, loopback_pair(port));
        assert_eq!(
            bound
                .dropped
                .iter()
                .map(|dropped| dropped.target.as_str())
                .collect::<Vec<_>>(),
            vec![stale.as_str()]
        );
    }

    #[tokio::test]
    async fn port_launch_binds_loopback_when_every_configured_address_is_unavailable() {
        let port = free_loopback_port();
        let targets = vec![
            format!("{STALE_ADDRESS}:{port}"),
            format!("hypercolor-stale.invalid:{port}"),
        ];

        let bound = bind_startup_listeners(&targets, Some(port), |_| Ok(()))
            .await
            .expect("a --port launch should fall back to loopback");

        assert_eq!(bound.addresses, loopback_pair(port));
        assert_eq!(bound.dropped.len(), 2);
    }

    #[tokio::test]
    async fn launches_without_a_fallback_port_still_fail_on_an_unavailable_address() {
        let port = free_loopback_port();
        let targets = vec![
            format!("{STALE_ADDRESS}:{port}"),
            format!("127.0.0.1:{port}"),
        ];

        let error = bind_startup_listeners(&targets, None, |_| Ok(()))
            .await
            .err()
            .expect("a launch without --port must not hide an unavailable address");

        assert!(
            format!("{error:#}").contains(STALE_ADDRESS),
            "the error should name the address, got {error:#}"
        );
    }

    #[tokio::test]
    async fn port_launch_never_tolerates_a_refused_network_bind() {
        let port = free_loopback_port();
        let targets = vec![format!("{STALE_ADDRESS}:{port}")];

        bind_startup_listeners(&targets, Some(port), |_| {
            anyhow::bail!("refusing to bind without a key")
        })
        .await
        .err()
        .expect("an authorization refusal must still abort startup");
    }

    #[test]
    fn keyless_network_listeners_are_the_non_loopback_ones_in_open_families() {
        let both = RemoteClientFamilies {
            ipv4: true,
            ipv6: true,
        };
        let ipv4_only = RemoteClientFamilies {
            ipv4: true,
            ipv6: false,
        };
        let ipv6_only = RemoteClientFamilies {
            ipv4: false,
            ipv6: true,
        };
        let loopback = loopback_pair(9555);
        let wildcard: Vec<std::net::SocketAddr> = vec![
            "0.0.0.0:9555"
                .parse()
                .expect("fixture address should parse"),
            "[::]:9555".parse().expect("fixture address should parse"),
        ];
        let mixed: Vec<std::net::SocketAddr> = vec![
            "192.168.1.42:9555"
                .parse()
                .expect("fixture address should parse"),
            loopback[0],
        ];

        assert!(keyless_network_listeners(&loopback, both).is_empty());
        assert_eq!(keyless_network_listeners(&wildcard, both), wildcard);
        assert_eq!(keyless_network_listeners(&mixed, both), vec![mixed[0]]);
        assert!(keyless_network_listeners(&wildcard, RemoteClientFamilies::NONE).is_empty());
        assert!(keyless_network_listeners(&mixed, RemoteClientFamilies::NONE).is_empty());
        // A listener only accepts clients of its own family.
        assert_eq!(
            keyless_network_listeners(&wildcard, ipv4_only),
            vec![wildcard[0]]
        );
        assert_eq!(
            keyless_network_listeners(&wildcard, ipv6_only),
            vec![wildcard[1]]
        );
        assert!(keyless_network_listeners(&wildcard[1..], ipv4_only).is_empty());
        assert!(keyless_network_listeners(&mixed, ipv6_only).is_empty());
    }

    #[test]
    fn a_degraded_bind_advertises_loopback() {
        let surviving_wildcard: Vec<std::net::SocketAddr> = vec![
            "[::]:9555".parse().expect("fixture address should parse"),
            "127.0.0.1:9555"
                .parse()
                .expect("fixture address should parse"),
        ];

        assert_eq!(
            advertised_bind(&surviving_wildcard, false),
            Some(surviving_wildcard[0])
        );
        assert_eq!(
            advertised_bind(&surviving_wildcard, true),
            Some(surviving_wildcard[1]),
            "a partial bind must not hand mDNS a wildcard"
        );
        assert_eq!(advertised_bind(&[], true), None);
    }

    #[test]
    fn only_a_port_only_launch_falls_back_to_loopback() {
        let config = default_config();
        let with_port = |options: DaemonRunOptions| DaemonRunOptions {
            port: Some(9555),
            ..options
        };

        assert_eq!(
            loopback_fallback_port(&with_port(DaemonRunOptions::default()), &config),
            Some(9555)
        );
        assert_eq!(
            loopback_fallback_port(&DaemonRunOptions::default(), &config),
            None,
            "a launch without --port keeps failing on an unavailable address"
        );
        for explicit in [
            DaemonRunOptions {
                bind: Some("127.0.0.1:9555".to_owned()),
                ..DaemonRunOptions::default()
            },
            with_port(DaemonRunOptions {
                bind: Some("192.0.2.1:9555".to_owned()),
                ..DaemonRunOptions::default()
            }),
            with_port(DaemonRunOptions {
                listen_address: Some("192.0.2.1".to_owned()),
                ..DaemonRunOptions::default()
            }),
            with_port(DaemonRunOptions {
                listen_all: true,
                ..DaemonRunOptions::default()
            }),
        ] {
            assert_eq!(
                loopback_fallback_port(&explicit, &config),
                None,
                "an explicit interface flag keeps failing loudly"
            );
        }
    }

    #[tokio::test]
    async fn extension_installers_observe_the_bound_api_addresses() {
        let directory = tempfile::tempdir().expect("daemon test directory should be created");
        let _paths = PathOverrides::install(directory.path()).await;
        let mut config = default_config();
        config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
        let config_manager = Arc::new(ConfigManager::from_config_unchecked(
            directory.path().join("hypercolor.toml"),
            config.clone(),
        ));
        let mut daemon =
            DaemonState::initialize(BootConfig::from_config_unchecked(config), config_manager)
                .expect("daemon test state should initialize");
        assert!(daemon.api_listen_addresses().is_empty());
        assert!(!daemon.api_bind_overridden_at_launch());

        let observed = Arc::new(Mutex::new(None));
        let probe = BindingProbe(Arc::clone(&observed));
        let address: std::net::SocketAddr = "0.0.0.0:9420"
            .parse()
            .expect("fixture address should parse");
        record_api_binding(&mut daemon, vec![address], true);
        install_extensions(&mut daemon, None, &[&probe])
            .expect("extension installation should succeed");

        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Some((vec![address], true))
        );
    }
}
