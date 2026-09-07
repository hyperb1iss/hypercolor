//! Canonical daemon process bootstrap shared by public and downstream binaries.

use crate::daemon::{self, DaemonExtensionInstaller, DaemonRunOptions};
#[cfg(target_os = "macos")]
use crate::macos_owner::{
    MacosDaemonGuard, MacosDaemonOwner, MacosDaemonSessionAttestation,
    MacosOwnerCoordinatorOutcome, MacosOwnerIdentity, MacosOwnerRecord, MacosOwnerRecoveryRequired,
    MacosOwnerStore, MacosOwnerStoreError, acquire_macos_daemon_guard,
    recover_incoming_daemon_owner, try_acquire_macos_daemon_guard,
};
use crate::startup::{ParentLifetime, install_signal_handlers};
use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
use clap::{Parser, ValueEnum};
#[cfg(target_os = "macos")]
use hypercolor_core::config::ConfigManager;
#[cfg(target_os = "macos")]
use hypercolor_macos_input::current_process_audit_token_identity;
use hypercolor_types::config::{RenderAccelerationMode, ServoGpuImportMode};
#[cfg(target_os = "macos")]
use hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE;
use hypercolor_types::service::SERVICE_IDENTITY_ENV;
#[cfg(not(target_os = "macos"))]
use hypercolor_types::service::ServiceStatus;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "macos"))]
use single_instance::SingleInstance;
#[cfg(target_os = "macos")]
use std::fmt::Write as _;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "linux")]
#[path = "linux_launcher_authority.rs"]
mod linux_launcher_authority;

#[cfg(target_os = "macos")]
#[path = "macos_launcher_authority.rs"]
mod macos_launcher_authority;

#[cfg(target_os = "windows")]
#[path = "windows_launcher_authority.rs"]
mod windows_launcher_authority;

#[cfg(target_os = "macos")]
const MACOS_OWNER_ARBITRATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(target_os = "windows")]
#[path = "windows_service.rs"]
mod windows_service;

/// Hypercolor lighting daemon — orchestrates RGB devices at up to 60fps.
#[derive(Parser, Debug)]
#[command(name = "hypercolor-daemon", about = "Hypercolor lighting daemon")]
struct DaemonArgs {
    /// Arm one signed macOS TCC canary row for the next matching daemon owner.
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    #[arg(
        long,
        hide = true,
        value_name = "REQUEST_JSON",
        conflicts_with = "macos_tcc_canary_validate"
    )]
    macos_tcc_canary_arm: Option<PathBuf>,

    /// Validate a directory of signed macOS TCC canary receipts.
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    #[arg(
        long,
        hide = true,
        value_name = "RECEIPT_DIR",
        conflicts_with = "macos_tcc_canary_arm"
    )]
    macos_tcc_canary_validate: Option<PathBuf>,

    /// Validate one macOS TCC canary request without arming it.
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    #[arg(
        long,
        hide = true,
        value_name = "REQUEST_JSON",
        conflicts_with_all = ["macos_tcc_canary_arm", "macos_tcc_canary_validate"]
    )]
    macos_tcc_canary_check_request: Option<PathBuf>,

    /// Atomically publish one bounded macOS TCC canary artifact.
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    #[arg(
        long,
        hide = true,
        value_names = ["CANARY_ROOT", "SOURCE", "DESTINATION"],
        num_args = 3,
        conflicts_with_all = [
            "macos_tcc_canary_arm",
            "macos_tcc_canary_validate",
            "macos_tcc_canary_check_request"
        ]
    )]
    macos_tcc_canary_publish: Option<Vec<PathBuf>>,

    /// Path to the configuration file.
    #[arg(short, long, env = "HYPERCOLOR_CONFIG")]
    config: Option<PathBuf>,

    /// Address and port to bind the API server to.
    #[arg(long)]
    bind: Option<String>,

    /// Host/interface to bind using the configured daemon port.
    #[arg(long, conflicts_with = "bind")]
    listen: Option<String>,

    /// Listen on every IPv4 and IPv6 network interface.
    #[arg(long, conflicts_with_all = ["bind", "listen"])]
    listen_all: bool,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long)]
    log_level: Option<String>,

    /// Override the configured compositor acceleration mode.
    #[arg(long, value_enum)]
    compositor_acceleration_mode: Option<RenderAccelerationModeArg>,

    /// Override the configured Servo GPU import mode.
    #[arg(long, value_enum)]
    servo_gpu_import_mode: Option<ServoGpuImportModeArg>,

    /// Serve the web UI from this directory (static files with SPA fallback).
    #[arg(long)]
    ui_dir: Option<PathBuf>,

    /// Load bundled effects from this directory instead of the install layout.
    #[arg(long, env = hypercolor_core::effect::EFFECTS_DIR_ENV)]
    effects_dir: Option<PathBuf>,

    /// Local macOS daemon topology selected by the process launcher.
    #[cfg(target_os = "macos")]
    #[arg(long, hide = true, value_enum, default_value_t = MacosDaemonOwnerArg::Standalone)]
    macos_owner: MacosDaemonOwnerArg,

    /// Run under the Windows Service Control Manager.
    #[cfg(target_os = "windows")]
    #[arg(long, hide = true)]
    windows_service: bool,
}

impl DaemonArgs {
    fn into_run_options(self) -> DaemonRunOptions {
        DaemonRunOptions {
            config: self.config,
            bind: self.bind,
            listen_address: self.listen,
            listen_all: self.listen_all,
            log_level: self.log_level,
            compositor_acceleration_mode: self.compositor_acceleration_mode.map(Into::into),
            servo_gpu_import_mode: self.servo_gpu_import_mode.map(Into::into),
            ui_dir: self.ui_dir,
            effects_dir: self.effects_dir,
            #[cfg(target_os = "macos")]
            macos_owner: Some(self.macos_owner.into()),
            #[cfg(not(target_os = "macos"))]
            macos_owner: None,
            macos_owner_snapshot: None,
            macos_daemon_session_attestation: None,
            service_status: None,
            session_monitors: None,
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum MacosDaemonOwnerArg {
    AppSidecar,
    DirectLaunchd,
    Homebrew,
    #[default]
    Standalone,
}

#[cfg(target_os = "macos")]
impl From<MacosDaemonOwnerArg> for MacosDaemonOwner {
    fn from(value: MacosDaemonOwnerArg) -> Self {
        match value {
            MacosDaemonOwnerArg::AppSidecar => Self::AppSidecar,
            MacosDaemonOwnerArg::DirectLaunchd => Self::DirectLaunchd,
            MacosDaemonOwnerArg::Homebrew => Self::Homebrew,
            MacosDaemonOwnerArg::Standalone => Self::Standalone,
        }
    }
}

#[cfg(target_os = "macos")]
impl From<MacosDaemonOwner> for MacosDaemonOwnerArg {
    fn from(value: MacosDaemonOwner) -> Self {
        match value {
            MacosDaemonOwner::AppSidecar => Self::AppSidecar,
            MacosDaemonOwner::DirectLaunchd => Self::DirectLaunchd,
            MacosDaemonOwner::Homebrew => Self::Homebrew,
            MacosDaemonOwner::Standalone => Self::Standalone,
        }
    }
}

#[cfg(target_os = "macos")]
impl MacosDaemonOwnerArg {
    const fn is_app_sidecar(self) -> bool {
        matches!(self, Self::AppSidecar)
    }
}

#[cfg(target_os = "macos")]
fn configure_macos_activation_policy(owner: MacosDaemonOwnerArg) -> Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    if !owner.is_app_sidecar() {
        return Ok(());
    }

    let main_thread =
        MainThreadMarker::new().context("daemon entrypoint is not on the main thread")?;
    let application = NSApplication::sharedApplication(main_thread);
    anyhow::ensure!(
        application.setActivationPolicy(NSApplicationActivationPolicy::Prohibited),
        "failed to suppress the app-sidecar daemon Dock icon"
    );
    Ok(())
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ServoGpuImportModeArg {
    Off,
    Auto,
    On,
}

impl From<ServoGpuImportModeArg> for ServoGpuImportMode {
    fn from(value: ServoGpuImportModeArg) -> Self {
        match value {
            ServoGpuImportModeArg::Off => Self::Off,
            ServoGpuImportModeArg::Auto => Self::Auto,
            ServoGpuImportModeArg::On => Self::On,
        }
    }
}

/// Run the canonical daemon process with downstream extension installers.
///
/// Installers must live for the process lifetime because the Windows Service
/// Control Manager invokes the daemon through a static callback.
///
/// # Errors
///
/// Returns an error when process bootstrap, daemon startup, serving, or
/// graceful shutdown fails.
pub fn run(extension_installers: &'static [&'static dyn DaemonExtensionInstaller]) -> Result<()> {
    #[cfg(target_os = "macos")]
    let (mut args, macos_owner_argument) = {
        let matches = DaemonArgs::command().get_matches();
        let argument_was_supplied =
            matches.value_source("macos_owner") == Some(ValueSource::CommandLine);
        let args = DaemonArgs::from_arg_matches(&matches)
            .context("failed to parse daemon command-line arguments")?;
        let argument = argument_was_supplied.then_some(args.macos_owner.into());
        (args, argument)
    };
    #[cfg(not(target_os = "macos"))]
    let args = DaemonArgs::parse();
    #[cfg(target_os = "linux")]
    let service_status = {
        let claim = crate::launcher_claim::read_service_identity_claim(
            std::env::var_os(SERVICE_IDENTITY_ENV).as_deref(),
        )?;
        let evidence = linux_launcher_authority::inspect_linux_launcher_authority()?;
        let identity =
            crate::launcher_claim::resolve_linux_launcher_identity(claim.as_ref(), evidence)?;
        ServiceStatus::new(identity, 0)
    };
    #[cfg(target_os = "windows")]
    let service_status = {
        let claim = crate::launcher_claim::read_service_identity_claim(
            std::env::var_os(SERVICE_IDENTITY_ENV).as_deref(),
        )?;
        let attested = windows_launcher_authority::attested_windows_launchers(args.windows_service);
        let identity = crate::launcher_claim::resolve_launcher_identity(claim.as_ref(), &attested)?;
        ServiceStatus::new(identity, 0)
    };
    #[cfg(target_os = "macos")]
    let macos_daemon_executable =
        std::env::current_exe().context("failed to resolve the current daemon executable")?;
    #[cfg(target_os = "macos")]
    let macos_daemon_requirement = designated_requirement(&macos_daemon_executable)?;
    #[cfg(target_os = "macos")]
    {
        let evidence = macos_launcher_authority::inspect_macos_launcher_authority(
            &macos_daemon_executable,
            &macos_daemon_requirement,
        )?;
        let owner = macos_launcher_authority::resolve_macos_launcher_owner(
            std::env::var_os(SERVICE_IDENTITY_ENV).as_deref(),
            std::env::var_os(macos_launcher_authority::MACOS_OWNER_ENV).as_deref(),
            macos_owner_argument,
            evidence,
        )?;
        args.macos_owner = owner.into();
    }
    #[cfg(target_os = "macos")]
    configure_macos_activation_policy(args.macos_owner)?;
    #[cfg(target_os = "macos")]
    let macos_owner = args.macos_owner.into();
    #[cfg(target_os = "macos")]
    let macos_owner_store = MacosOwnerStore::new(ConfigManager::data_dir());
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(request_path) = args.macos_tcc_canary_arm.as_deref() {
        let path = crate::macos_tcc_canary::arm_macos_tcc_canary(
            &ConfigManager::data_dir(),
            request_path,
        )?;
        println!("macos_tcc_canary_armed={}", path.display());
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(receipt_dir) = args.macos_tcc_canary_validate.as_deref() {
        let validation = crate::macos_tcc_canary::validate_macos_tcc_canary_receipts(receipt_dir)?;
        println!("{}", serde_json::to_string_pretty(&validation)?);
        if !validation.preferred_topology_eligible {
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(request_path) = args.macos_tcc_canary_check_request.as_deref() {
        crate::macos_tcc_canary::validate_macos_tcc_canary_request(request_path)?;
        println!("macos_tcc_canary_request_valid={}", request_path.display());
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(paths) = args.macos_tcc_canary_publish.as_deref() {
        let [canary_root, source, destination] = paths else {
            anyhow::bail!("macOS TCC canary artifact publication requires exactly three paths");
        };
        crate::macos_tcc_canary::publish_macos_tcc_canary_artifact(
            canary_root,
            source,
            destination,
        )?;
        println!("macos_tcc_canary_artifact={}", destination.display());
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let macos_owner_identity =
        current_macos_owner_identity(&macos_daemon_executable, &macos_daemon_requirement)?;
    #[cfg(target_os = "macos")]
    let macos_instance_guard = match try_acquire_macos_daemon_guard(&daemon_instance_name())
        .map_err(anyhow::Error::msg)
        .context("failed to acquire daemon single-instance guard")?
    {
        Some(guard) => guard,
        None => match arbitrate_macos_owner_contention(
            &macos_owner_store,
            macos_owner,
            &macos_owner_identity,
        )? {
            MacosOwnerContention::GuardHeld => {
                let exit_code = macos_contender_exit_code(args.macos_owner);
                if exit_code == 0 {
                    return Ok(());
                }
                std::process::exit(exit_code);
            }
            MacosOwnerContention::Reacquired(guard) => guard,
        },
    };
    #[cfg(not(target_os = "macos"))]
    let instance = SingleInstance::new(&daemon_instance_name())
        .context("failed to acquire daemon single-instance guard")?;
    #[cfg(not(target_os = "macos"))]
    if !instance.is_single() {
        eprintln!("hypercolor-daemon is already running; exiting");
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    let _instance_guard = instance;

    #[cfg(target_os = "macos")]
    let macos_owner_record = publish_macos_owner(
        &macos_owner_store,
        &macos_instance_guard,
        macos_owner,
        macos_owner_identity,
    )?;
    #[cfg(target_os = "macos")]
    let mut owner_snapshot = macos_owner_record.snapshot();
    #[cfg(target_os = "macos")]
    if let Some(MacosOwnerCoordinatorOutcome::RecoveryRequired {
        requested_owner,
        prior_owner,
        phase,
    }) = recover_incoming_daemon_owner(&macos_owner_store, macos_owner)
        .context("failed to recover the macOS daemon owner journal before runtime startup")?
    {
        owner_snapshot = owner_snapshot.with_recovery_required(Some(MacosOwnerRecoveryRequired {
            requested_owner,
            prior_owner,
            phase,
        }));
        eprintln!(
            "macos_daemon_owner_recovery_required: requested={requested_owner:?} prior={prior_owner:?} phase={phase:?}"
        );
    }

    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if crate::macos_tcc_canary::run_armed_macos_tcc_canary(&ConfigManager::data_dir(), macos_owner)?
    {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    if args.windows_service {
        let mut options = args.into_run_options();
        options.service_status = Some(service_status);
        return windows_service::run(options, extension_installers);
    }

    let options = args.into_run_options();
    #[cfg(not(target_os = "macos"))]
    let options = {
        let mut options = options;
        options.service_status = Some(service_status);
        options
    };
    #[cfg(target_os = "macos")]
    let options = {
        let mut options = options;
        options.macos_owner_snapshot = Some(owner_snapshot);
        options
    };
    #[cfg(target_os = "macos")]
    {
        let runtime = daemon::build_main_runtime()?;
        let (prepared, authority) = prepare_macos_daemon_with_session(
            &runtime,
            options,
            macos_owner_store,
            macos_owner_record,
            macos_instance_guard,
        )?;
        let result = run_prepared_macos_daemon(runtime, prepared, extension_installers);
        finish_macos_daemon_run(result, authority)
    }
    #[cfg(not(target_os = "macos"))]
    {
        run_daemon(options, extension_installers)
    }
}

#[cfg(target_os = "macos")]
fn prepare_macos_daemon_with_session(
    runtime: &tokio::runtime::Runtime,
    options: DaemonRunOptions,
    store: MacosOwnerStore,
    owner_record: MacosOwnerRecord,
    instance_guard: MacosDaemonGuard,
) -> Result<(daemon::PreparedDaemon, MacosDaemonRuntimeAuthority)> {
    let mut prepared = runtime.block_on(daemon::prepare(options))?;
    let listener_lease = prepared.take_api_listener_lease()?;
    let mut authority =
        MacosDaemonRuntimeAuthority::new(store, owner_record, instance_guard, listener_lease);
    let attestation = authority
        .publish_session()
        .context("failed to publish the private macOS daemon session")?;
    prepared.install_macos_daemon_session_attestation(attestation.clone());
    Ok((prepared, authority))
}

#[cfg(all(target_os = "macos", test))]
fn prepare_then_publish<Prepared, Published>(
    prepare: impl FnOnce() -> Result<Prepared>,
    publish: impl FnOnce() -> Result<Published>,
) -> Result<(Prepared, Published)> {
    let prepared = prepare()?;
    let published = publish()?;
    Ok((prepared, published))
}

#[cfg(target_os = "macos")]
struct MacosDaemonRuntimeAuthority {
    store: MacosOwnerStore,
    owner_record: MacosOwnerRecord,
    attestation: Option<MacosDaemonSessionAttestation>,
    instance_guard: Option<MacosDaemonGuard>,
    listener_lease: Option<daemon::ApiListenerLease>,
    session_clear_finished: bool,
}

#[cfg(target_os = "macos")]
impl MacosDaemonRuntimeAuthority {
    fn new(
        store: MacosOwnerStore,
        owner_record: MacosOwnerRecord,
        instance_guard: MacosDaemonGuard,
        listener_lease: daemon::ApiListenerLease,
    ) -> Self {
        Self {
            store,
            owner_record,
            attestation: None,
            instance_guard: Some(instance_guard),
            listener_lease: Some(listener_lease),
            session_clear_finished: false,
        }
    }

    fn publish_session(&mut self) -> Result<MacosDaemonSessionAttestation, MacosOwnerStoreError> {
        let attestation = self.store.publish_daemon_session_attestation(
            self.instance_guard
                .as_ref()
                .expect("runtime authority must retain its canonical guard"),
            &self.owner_record.incarnation(),
        )?;
        self.attestation = Some(attestation.clone());
        Ok(attestation)
    }

    fn clear_session(&mut self) -> Result<bool, MacosOwnerStoreError> {
        let incarnation = self.owner_record.incarnation();
        let attestation = match self.attestation.clone() {
            Some(attestation) => Some(attestation),
            None => self
                .store
                .load_daemon_session_attestation()?
                .filter(|attestation| attestation.owner_incarnation() == incarnation),
        };
        let result = attestation.map_or(Ok(false), |attestation| {
            self.store
                .clear_daemon_session_attestation(&incarnation, &attestation.server_session_id)
        });
        if result.is_ok() {
            self.session_clear_finished = true;
        }
        result
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacosDaemonRuntimeAuthority {
    fn drop(&mut self) {
        if !self.session_clear_finished
            && let Err(error) = self.clear_session()
        {
            eprintln!(
                "failed to clear the private macOS daemon session during authority release: {error}"
            );
        }
        drop(self.instance_guard.take());
        drop(self.listener_lease.take());
    }
}

#[cfg(target_os = "macos")]
fn finish_macos_daemon_run(
    daemon_result: Result<()>,
    mut authority: MacosDaemonRuntimeAuthority,
) -> Result<()> {
    let cleanup_result = authority.clear_session();
    combine_macos_daemon_result(daemon_result, cleanup_result)
}

#[cfg(target_os = "macos")]
fn combine_macos_daemon_result(
    daemon_result: Result<()>,
    cleanup_result: Result<bool, MacosOwnerStoreError>,
) -> Result<()> {
    match (daemon_result, cleanup_result) {
        (Err(daemon_error), Err(cleanup_error)) => {
            eprintln!(
                "failed to clear the private macOS daemon session after daemon failure: {cleanup_error}"
            );
            Err(daemon_error)
        }
        (Err(daemon_error), Ok(_)) => Err(daemon_error),
        (Ok(()), Ok(_)) => Ok(()),
        (Ok(()), Err(cleanup_error)) => {
            Err(cleanup_error).context("failed to clear the private macOS daemon session")
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
enum MacosOwnerContention {
    GuardHeld,
    Reacquired(MacosDaemonGuard),
}

#[cfg(target_os = "macos")]
fn arbitrate_macos_owner_contention(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    identity: &MacosOwnerIdentity,
) -> Result<MacosOwnerContention> {
    arbitrate_macos_owner_contention_with(
        store,
        owner,
        identity,
        &daemon_instance_name(),
        MACOS_OWNER_ARBITRATION_TIMEOUT,
    )
}

#[cfg(target_os = "macos")]
fn arbitrate_macos_owner_contention_with(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    identity: &MacosOwnerIdentity,
    instance_name: &str,
    timeout: std::time::Duration,
) -> Result<MacosOwnerContention> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;

    if try_record_macos_owner_conflict(store, owner, identity) {
        return resolve_macos_guard_state(instance_name);
    }

    let owner_path = store.owner_record_path();
    let directory = owner_path
        .parent()
        .context("macOS owner record has no parent directory")?
        .to_path_buf();
    let directory_ready = std::fs::create_dir_all(&directory).is_ok();
    enum ArbitrationSignal {
        OwnerRecordChanged,
        GuardAcquired(Result<MacosDaemonGuard, String>),
    }

    let (signal_tx, signal_rx) = mpsc::sync_channel(2);
    let watched_path = owner_path.clone();
    let owner_signal_tx = signal_tx.clone();
    let mut watcher = directory_ready
        .then(|| {
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| event.paths.iter().any(|path| path == &watched_path)) {
                    let _ = owner_signal_tx.try_send(ArbitrationSignal::OwnerRecordChanged);
                }
            })
        })
        .transpose()
        .ok()
        .flatten();
    if let Some(active_watcher) = watcher.as_mut() {
        let _ = active_watcher.watch(&directory, RecursiveMode::NonRecursive);
    }

    if try_record_macos_owner_conflict(store, owner, identity) {
        return resolve_macos_guard_state(instance_name);
    }

    let guard_signal_tx = signal_tx;
    let guard_instance_name = instance_name.to_owned();
    std::thread::Builder::new()
        .name("hypercolor-macos-owner-arbitration".to_owned())
        .spawn(move || {
            let result =
                acquire_macos_daemon_guard(&guard_instance_name).map_err(|error| error.to_string());
            let _ = guard_signal_tx.send(ArbitrationSignal::GuardAcquired(result));
        })
        .context("failed to start the macOS owner guard waiter")?;

    let started = std::time::Instant::now();
    while let Some(remaining) = timeout.checked_sub(started.elapsed()) {
        match signal_rx.recv_timeout(remaining) {
            Ok(ArbitrationSignal::OwnerRecordChanged) => {
                if try_record_macos_owner_conflict(store, owner, identity) {
                    return resolve_macos_guard_state(instance_name);
                }
            }
            Ok(ArbitrationSignal::GuardAcquired(Ok(guard))) => {
                return Ok(MacosOwnerContention::Reacquired(guard));
            }
            Ok(ArbitrationSignal::GuardAcquired(Err(error))) => {
                anyhow::bail!("failed to reacquire the daemon single-instance guard: {error}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("macOS owner arbitration watch disconnected")
            }
        }
    }

    resolve_macos_guard_state(instance_name)
}

#[cfg(target_os = "macos")]
fn try_record_macos_owner_conflict(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    identity: &MacosOwnerIdentity,
) -> bool {
    match record_macos_owner_conflict(store, owner, identity.clone()) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("macos_daemon_owner_diagnostic_unavailable: {error:#}");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn resolve_macos_guard_state(instance_name: &str) -> Result<MacosOwnerContention> {
    match try_acquire_macos_daemon_guard(instance_name)
        .map_err(anyhow::Error::msg)
        .context("failed to inspect the authoritative daemon guard")?
    {
        Some(guard) => Ok(MacosOwnerContention::Reacquired(guard)),
        None => Ok(MacosOwnerContention::GuardHeld),
    }
}

#[cfg(target_os = "macos")]
const fn launchd_contender_exits_zero(owner: MacosDaemonOwnerArg) -> bool {
    matches!(
        owner,
        MacosDaemonOwnerArg::DirectLaunchd | MacosDaemonOwnerArg::Homebrew
    )
}

#[cfg(target_os = "macos")]
const fn macos_contender_exit_code(owner: MacosDaemonOwnerArg) -> i32 {
    if launchd_contender_exits_zero(owner) {
        0
    } else {
        MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE
    }
}

#[cfg(target_os = "macos")]
fn publish_macos_owner(
    store: &MacosOwnerStore,
    guard: &MacosDaemonGuard,
    owner: MacosDaemonOwner,
    identity: MacosOwnerIdentity,
) -> Result<MacosOwnerRecord> {
    let record = store
        .publish_guard_winner(guard, owner, identity)
        .context("failed to publish the macOS daemon owner")?;
    Ok(record)
}

#[cfg(target_os = "macos")]
fn record_macos_owner_conflict(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    identity: MacosOwnerIdentity,
) -> Result<()> {
    let observed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock predates the Unix epoch")?
        .as_millis()
        .try_into()
        .context("macOS owner conflict timestamp exceeds u64")?;
    let update = store
        .record_conflict(owner, identity, observed_at_ms)
        .context("failed to publish the macOS daemon owner conflict")?;
    let snapshot = update.snapshot();
    eprintln!(
        "macos_daemon_owner_conflict: active={:?} epoch={} contender={owner:?}",
        snapshot.active_owner, snapshot.owner_epoch
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn current_macos_owner_identity(
    executable_path: &std::path::Path,
    requirement: &str,
) -> Result<MacosOwnerIdentity> {
    let digest = Sha256::digest(requirement.as_bytes());
    let mut designated_requirement_hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut designated_requirement_hash, "{byte:02x}")
            .expect("writing into a String cannot fail");
    }
    MacosOwnerIdentity::new(
        current_process_audit_token_identity()?,
        executable_path,
        designated_requirement_hash,
        std::process::id(),
    )
    .map_err(anyhow::Error::from)
}

#[cfg(target_os = "macos")]
fn designated_requirement(executable_path: &std::path::Path) -> Result<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(executable_path)
        .output()
        .context("failed to inspect the daemon code signature")?;
    if !output.status.success() {
        anyhow::bail!("codesign could not read the daemon designated requirement");
    }
    parse_designated_requirement(&output.stdout)
}

#[cfg(target_os = "macos")]
fn parse_designated_requirement(stdout: &[u8]) -> Result<String> {
    const MAX_CODESIGN_STDOUT_BYTES: usize = 16 * 1024;
    const MAX_DESIGNATED_REQUIREMENT_BYTES: usize = 8 * 1024;

    if stdout.len() > MAX_CODESIGN_STDOUT_BYTES {
        anyhow::bail!("codesign designated-requirement output exceeds 16 KiB");
    }
    let stdout = std::str::from_utf8(stdout)
        .context("codesign returned a non-UTF-8 designated requirement")?;
    let requirement = stdout.lines().find_map(|line| {
        line.strip_prefix("designated => ")
            .or_else(|| line.strip_prefix("# designated => "))
    });
    let requirement = requirement.context("codesign omitted the daemon designated requirement")?;
    if requirement.is_empty() || requirement.len() > MAX_DESIGNATED_REQUIREMENT_BYTES {
        anyhow::bail!("codesign designated requirement is empty or exceeds 8 KiB");
    }
    Ok(requirement.to_owned())
}

#[cfg(not(target_os = "macos"))]
fn run_daemon(
    options: DaemonRunOptions,
    extension_installers: &'static [&'static dyn DaemonExtensionInstaller],
) -> Result<()> {
    let runtime = daemon::build_main_runtime()?;
    runtime.block_on(async move {
        // Linux pdeathsig and the Windows job object are armed by the
        // supervisor; the kernel delivers supervisor death to this process.
        let shutdown_rx = install_signal_handlers(ParentLifetime::Kernel);
        daemon::run_with_extensions(options, shutdown_rx, extension_installers).await
    })
}

#[cfg(target_os = "macos")]
fn run_prepared_macos_daemon(
    runtime: tokio::runtime::Runtime,
    prepared: daemon::PreparedDaemon,
    extension_installers: &'static [&'static dyn DaemonExtensionInstaller],
) -> Result<()> {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let runtime_thread = std::thread::Builder::new()
        .name("hypercolor-daemon-runtime".to_owned())
        .spawn(move || {
            let _run_loop_stop = MainRunLoopStop;
            let result = runtime.block_on(async move {
                let shutdown_rx =
                    install_signal_handlers(ParentLifetime::Watch(Box::new(|parent_pid| {
                        if let Err(error) = crate::macos_owner::wait_for_process_exit(parent_pid) {
                            tracing::warn!(
                                %error,
                                parent_pid,
                                "kqueue parent watch failed; falling back to no parent-death watch"
                            );
                            // Never flip shutdown on a watch failure: park the
                            // thread so the daemon keeps serving under launchd
                            // or the terminal user.
                            std::thread::park();
                        }
                    })));
                Box::pin(prepared.run_with_extensions(shutdown_rx, extension_installers)).await
            });
            let _ = result_tx.send(result);
        })
        .context("failed to spawn the macOS daemon runtime thread")?;

    objc2_core_foundation::CFRunLoop::run();
    let result = result_rx.recv();
    runtime_thread
        .join()
        .map_err(|_| anyhow::anyhow!("macOS daemon runtime thread panicked"))?;
    result.context("macOS daemon runtime exited without a result")?
}

#[cfg(target_os = "macos")]
struct MainRunLoopStop;

#[cfg(target_os = "macos")]
impl Drop for MainRunLoopStop {
    fn drop(&mut self) {
        dispatch2::run_on_main(|_mtm| {
            if let Some(run_loop) = objc2_core_foundation::CFRunLoop::main() {
                run_loop.stop();
            }
        });
    }
}

fn daemon_instance_name() -> String {
    #[cfg(target_os = "macos")]
    {
        std::env::temp_dir()
            .join("hypercolor-daemon.lock")
            .display()
            .to_string()
    }

    #[cfg(not(target_os = "macos"))]
    {
        "hypercolor-daemon".to_owned()
    }
}

/// Raise the render thread above normal priority where the host scheduler
/// benefits from the hint.
#[cfg(target_os = "windows")]
pub(crate) fn configure_render_thread_priority() {
    use thread_priority::{ThreadPriority, WinAPIThreadPriority, set_current_thread_priority};

    let priority = ThreadPriority::Os(WinAPIThreadPriority::AboveNormal.into());
    match set_current_thread_priority(priority) {
        Ok(()) => tracing::debug!("configured Windows render thread priority"),
        Err(error) => tracing::warn!(
            error = %error,
            "failed to configure Windows render thread priority"
        ),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn configure_render_thread_priority() {}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        DaemonArgs, RenderAccelerationModeArg, ServoGpuImportModeArg, daemon_instance_name,
    };
    #[cfg(target_os = "macos")]
    use super::{
        MacosDaemonOwnerArg, MacosDaemonRuntimeAuthority, MacosOwnerContention,
        arbitrate_macos_owner_contention_with, combine_macos_daemon_result,
        launchd_contender_exits_zero, macos_contender_exit_code, parse_designated_requirement,
        prepare_then_publish,
    };
    #[cfg(target_os = "macos")]
    use crate::macos_owner::{
        MacosDaemonOwner, MacosOwnerIdentity, MacosOwnerStore, MacosOwnerStoreError,
        try_acquire_macos_daemon_guard,
    };
    use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode};
    #[cfg(target_os = "macos")]
    use hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE;

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
    fn servo_gpu_import_mode_cli_override_updates_config() {
        let args =
            DaemonArgs::try_parse_from(["hypercolor-daemon", "--servo-gpu-import-mode", "auto"])
                .expect("Servo GPU import CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.servo_gpu_import_mode {
            config.rendering.servo_gpu_import.mode = mode.into();
        }

        assert_eq!(
            config.rendering.servo_gpu_import.mode,
            ServoGpuImportMode::Auto
        );
    }

    #[test]
    fn daemon_network_flags_accept_only_canonical_spellings() {
        let listen = DaemonArgs::try_parse_from(["hypercolor-daemon", "--listen", "192.168.1.10"])
            .expect("canonical listen flag should parse");
        assert_eq!(listen.listen.as_deref(), Some("192.168.1.10"));

        let listen_all = DaemonArgs::try_parse_from(["hypercolor-daemon", "--listen-all"])
            .expect("canonical listen-all flag should parse");
        assert!(listen_all.listen_all);

        for retired in ["--listen-host", "--host"] {
            let error = DaemonArgs::try_parse_from(["hypercolor-daemon", retired, "192.168.1.10"])
                .expect_err("retired listen flag must be rejected");
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
        for retired in ["--lan", "--all-interfaces"] {
            let error = DaemonArgs::try_parse_from(["hypercolor-daemon", retired])
                .expect_err("retired listen-all flag must be rejected");
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn render_acceleration_arg_maps_all_modes() {
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Cpu),
            RenderAccelerationMode::Cpu
        );
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Auto),
            RenderAccelerationMode::Auto
        );
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Gpu),
            RenderAccelerationMode::Gpu
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_managed_contenders_exit_zero_without_respawn() {
        assert!(launchd_contender_exits_zero(
            MacosDaemonOwnerArg::DirectLaunchd
        ));
        assert!(launchd_contender_exits_zero(MacosDaemonOwnerArg::Homebrew));
        assert!(!launchd_contender_exits_zero(
            MacosDaemonOwnerArg::AppSidecar
        ));
        assert!(!launchd_contender_exits_zero(
            MacosDaemonOwnerArg::Standalone
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn held_guard_applies_topology_policy_without_an_owner_record() {
        for (owner, owner_arg, exits_zero) in [
            (
                MacosDaemonOwner::DirectLaunchd,
                MacosDaemonOwnerArg::DirectLaunchd,
                true,
            ),
            (
                MacosDaemonOwner::Homebrew,
                MacosDaemonOwnerArg::Homebrew,
                true,
            ),
            (
                MacosDaemonOwner::AppSidecar,
                MacosDaemonOwnerArg::AppSidecar,
                false,
            ),
            (
                MacosDaemonOwner::Standalone,
                MacosDaemonOwnerArg::Standalone,
                false,
            ),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory should build");
            let store = MacosOwnerStore::new(directory.path());
            let guard_path = directory.path().join(format!("{owner:?}.lock"));
            let guard_name = guard_path.to_string_lossy().into_owned();
            let _winner = try_acquire_macos_daemon_guard(&guard_name)
                .expect("guard inspection should succeed")
                .expect("fixture winner should acquire the guard");
            let outcome = arbitrate_macos_owner_contention_with(
                &store,
                owner,
                &owner_identity(owner, 200),
                &guard_name,
                std::time::Duration::ZERO,
            )
            .expect("held guard should produce a terminal contention outcome");
            assert!(matches!(outcome, MacosOwnerContention::GuardHeld));
            assert_eq!(launchd_contender_exits_zero(owner_arg), exits_zero);
            assert_eq!(
                macos_contender_exit_code(owner_arg),
                if exits_zero {
                    0
                } else {
                    MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE
                }
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_diagnostics_never_override_held_guard_policy() {
        for (owner, owner_arg, bytes, exits_zero) in [
            (
                MacosDaemonOwner::DirectLaunchd,
                MacosDaemonOwnerArg::DirectLaunchd,
                b"{ malformed".to_vec(),
                true,
            ),
            (
                MacosDaemonOwner::Homebrew,
                MacosDaemonOwnerArg::Homebrew,
                future_owner_record(),
                true,
            ),
            (
                MacosDaemonOwner::AppSidecar,
                MacosDaemonOwnerArg::AppSidecar,
                b"{ malformed".to_vec(),
                false,
            ),
            (
                MacosDaemonOwner::Standalone,
                MacosDaemonOwnerArg::Standalone,
                future_owner_record(),
                false,
            ),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory should build");
            let store = MacosOwnerStore::new(directory.path());
            std::fs::write(store.owner_record_path(), bytes)
                .expect("diagnostic fixture should write");
            let guard_path = directory.path().join(format!("{owner:?}.lock"));
            let guard_name = guard_path.to_string_lossy().into_owned();
            let _winner = try_acquire_macos_daemon_guard(&guard_name)
                .expect("guard inspection should succeed")
                .expect("fixture winner should acquire the guard");
            let outcome = arbitrate_macos_owner_contention_with(
                &store,
                owner,
                &owner_identity(owner, 201),
                &guard_name,
                std::time::Duration::ZERO,
            )
            .expect("invalid diagnostics should not override the held guard");
            assert!(matches!(outcome, MacosOwnerContention::GuardHeld));
            assert_eq!(launchd_contender_exits_zero(owner_arg), exits_zero);
            assert_eq!(
                macos_contender_exit_code(owner_arg),
                if exits_zero {
                    0
                } else {
                    MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE
                }
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn owner_record_alone_never_authorizes_a_contender_loss() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                owner_identity(MacosDaemonOwner::DirectLaunchd, 101),
            )
            .expect("diagnostic owner should publish");
        let guard_name = directory
            .path()
            .join("unheld.lock")
            .to_string_lossy()
            .into_owned();
        let outcome = arbitrate_macos_owner_contention_with(
            &store,
            MacosDaemonOwner::AppSidecar,
            &owner_identity(MacosDaemonOwner::AppSidecar, 202),
            &guard_name,
            std::time::Duration::ZERO,
        )
        .expect("free guard should be acquired despite a durable owner record");
        assert!(matches!(outcome, MacosOwnerContention::Reacquired(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn authoritative_guard_acquisition_failures_remain_fatal() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path().join("owner-state"));
        store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                owner_identity(MacosDaemonOwner::DirectLaunchd, 101),
            )
            .expect("diagnostic owner should publish");
        let error = arbitrate_macos_owner_contention_with(
            &store,
            MacosDaemonOwner::AppSidecar,
            &owner_identity(MacosDaemonOwner::AppSidecar, 202),
            &directory.path().to_string_lossy(),
            std::time::Duration::ZERO,
        )
        .expect_err("opening a directory as the guard file must remain fatal");
        assert!(
            error
                .to_string()
                .contains("failed to inspect the authoritative daemon guard")
        );
    }

    #[cfg(target_os = "macos")]
    fn owner_identity(owner: MacosDaemonOwner, pid: u32) -> MacosOwnerIdentity {
        MacosOwnerIdentity::new(
            format!("audit-{owner:?}-{pid}"),
            format!("/Applications/{owner:?}/hypercolor-daemon"),
            format!("requirement-{owner:?}"),
            pid,
        )
        .expect("fixture identity should build")
    }

    #[cfg(target_os = "macos")]
    fn future_owner_record() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 99,
            "owner_epoch": 1,
            "active_owner": "app_sidecar",
            "active_identity": {
                "audit_token_identity": "audit-winner",
                "executable_path": "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
                "designated_requirement_hash": "requirement-winner",
                "pid": 100
            },
            "conflict": null,
            "selected_external_owner": null
        }))
        .expect("future owner fixture should serialize")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn designated_requirement_parser_accepts_signed_and_ad_hoc_stdout() {
        assert_eq!(
            parse_designated_requirement(
                b"designated => identifier \"tech.hyperbliss.hypercolor.daemon\" and anchor apple generic\n"
            )
            .expect("signed requirement should parse"),
            "identifier \"tech.hyperbliss.hypercolor.daemon\" and anchor apple generic"
        );
        assert_eq!(
            parse_designated_requirement(b"# designated => cdhash H\"0123456789abcdef\"\n")
                .expect("ad-hoc requirement should parse"),
            "cdhash H\"0123456789abcdef\""
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn designated_requirement_parser_rejects_near_matches_and_oversized_output() {
        assert!(parse_designated_requirement(b"Executable=/tmp/hypercolor-daemon\n").is_err());
        assert!(parse_designated_requirement(b" designated => identifier \"wrong\"\n").is_err());
        assert!(parse_designated_requirement(b"designated => \n").is_err());
        assert!(parse_designated_requirement(&[0xff]).is_err());
        assert!(parse_designated_requirement(&vec![b'x'; 16 * 1024 + 1]).is_err());
        let oversized_requirement = format!("designated => {}\n", "x".repeat(8 * 1024 + 1));
        assert!(parse_designated_requirement(oversized_requirement.as_bytes()).is_err());
    }

    #[test]
    fn servo_gpu_import_arg_maps_all_modes() {
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::Off),
            ServoGpuImportMode::Off
        );
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::Auto),
            ServoGpuImportMode::Auto
        );
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::On),
            ServoGpuImportMode::On
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_the_app_sidecar_uses_background_activation_policy() {
        assert!(MacosDaemonOwnerArg::AppSidecar.is_app_sidecar());
        assert!(!MacosDaemonOwnerArg::DirectLaunchd.is_app_sidecar());
        assert!(!MacosDaemonOwnerArg::Homebrew.is_app_sidecar());
        assert!(!MacosDaemonOwnerArg::Standalone.is_app_sidecar());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn daemon_error_remains_primary_when_session_cleanup_also_fails() {
        let daemon_error = combine_macos_daemon_result(
            Err(anyhow::anyhow!("daemon failed")),
            Err(MacosOwnerStoreError::MissingOwnerRecord),
        )
        .expect_err("daemon and cleanup failure should remain an error");
        assert_eq!(daemon_error.to_string(), "daemon failed");

        let cleanup_error =
            combine_macos_daemon_result(Ok(()), Err(MacosOwnerStoreError::MissingOwnerRecord))
                .expect_err("cleanup failure after success should be returned");
        assert!(
            cleanup_error
                .to_string()
                .contains("failed to clear the private macOS daemon session")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_listener_attestation_occupied_port_prevents_publication() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("fixture should pre-bind a loopback port");
        let occupied_address = occupied
            .local_addr()
            .expect("occupied address should resolve");
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let guard_path = directory.path().join("daemon.lock");
        let guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("guard acquisition should succeed")
            .expect("fixture should win the guard");
        let store = MacosOwnerStore::new(directory.path().join("store"));
        let record = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                owner_identity(MacosDaemonOwner::AppSidecar, std::process::id()),
            )
            .expect("owner record should publish");
        let publication_count = std::cell::Cell::new(0_u32);

        let error = prepare_then_publish(
            || super::daemon::bind_api_listener(occupied_address),
            || {
                publication_count.set(publication_count.get() + 1);
                Ok(store.publish_daemon_session_attestation(&guard, &record.incarnation())?)
            },
        )
        .expect_err("occupied final API port must fail preparation");

        assert_eq!(publication_count.get(), 0);
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::AddrInUse)
        );
        assert!(
            store
                .load_daemon_session_attestation()
                .expect("session state should load")
                .is_none()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_runtime_authority_unwind_clears_visible_session_before_release() {
        let runtime = super::daemon::build_main_runtime().expect("runtime should build");
        let mut prepared = runtime
            .block_on(super::daemon::prepare(super::DaemonRunOptions {
                bind: Some("127.0.0.1:0".to_owned()),
                ..super::DaemonRunOptions::default()
            }))
            .expect("daemon should prepare its final listener");
        let address = prepared.advertised_bind();
        let listener_lease = prepared
            .take_api_listener_lease()
            .expect("listener lease should transfer once");
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let guard_path = directory.path().join("daemon.lock");
        let guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("guard acquisition should succeed")
            .expect("fixture should win the guard");
        let store = MacosOwnerStore::new(directory.path().join("store"));
        let record = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                owner_identity(MacosDaemonOwner::AppSidecar, std::process::id()),
            )
            .expect("owner record should publish");
        store
            .publish_daemon_session_attestation(&guard, &record.incarnation())
            .expect("visible session fixture should publish before authority recovery");
        drop(prepared);
        let _runtime_context = runtime.enter();

        super::daemon::bind_api_listener(address)
            .expect_err("listener lease must block takeover before authority release");
        let authority =
            MacosDaemonRuntimeAuthority::new(store.clone(), record, guard, listener_lease);
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _authority = authority;
            panic!("exercise runtime authority unwind");
        }));
        assert!(unwind.is_err());
        assert!(
            store
                .load_daemon_session_attestation()
                .expect("session state should load")
                .is_none()
        );

        let rebound = super::daemon::bind_api_listener(address)
            .expect("port should rebind only after session, guard, and lease release");
        drop(rebound);
        let reacquired = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("guard inspection should succeed")
            .expect("canonical guard should release before port takeover");
        drop(reacquired);
    }

    #[test]
    fn daemon_instance_name_is_stable() {
        let name = daemon_instance_name();

        assert!(name.contains("hypercolor-daemon"));
    }
}
