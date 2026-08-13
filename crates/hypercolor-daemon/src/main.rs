// Intentionally console-subsystem: `just daemon`, `hypercolor-daemon --help`,
// and other direct CLI invocations need stdout/stderr attached to the user's
// terminal. The supervisor inside hypercolor-app hides its child via
// CREATE_NO_WINDOW (see `configure_platform_command` in supervisor::mod), so
// the GUI shell path also stays clean without forcing GUI subsystem here.

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
#[cfg(target_os = "macos")]
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::daemon::{self, DaemonRunOptions};
#[cfg(target_os = "macos")]
use hypercolor_daemon::macos_owner::{
    MacosDaemonGuard, MacosDaemonOwner, MacosOwnerCoordinatorOutcome, MacosOwnerIdentity,
    MacosOwnerRecoveryRequired, MacosOwnerSnapshot, MacosOwnerStore, acquire_macos_daemon_guard,
    recover_incoming_daemon_owner, try_acquire_macos_daemon_guard,
};
use hypercolor_daemon::startup::install_signal_handlers;
#[cfg(target_os = "macos")]
use hypercolor_macos_input::current_process_audit_token_identity;
use hypercolor_types::config::{RenderAccelerationMode, ServoGpuImportMode};
#[cfg(target_os = "macos")]
use hypercolor_types::event::MACOS_DAEMON_OWNER_CONFLICT_EXIT_CODE;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "macos"))]
use single_instance::SingleInstance;
#[cfg(target_os = "macos")]
use std::fmt::Write as _;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
const MACOS_OWNER_ARBITRATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(target_os = "windows")]
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
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Address and port to bind the API server to.
    #[arg(long)]
    bind: Option<String>,

    /// Host/interface to bind using the configured daemon port.
    #[arg(long, alias = "listen-host", alias = "host", conflicts_with = "bind")]
    listen: Option<String>,

    /// Listen on every IPv4 and IPv6 network interface.
    #[arg(long, alias = "lan", alias = "all-interfaces", conflicts_with_all = ["bind", "listen"])]
    listen_all: bool,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long)]
    log_level: Option<String>,

    /// Override the configured compositor acceleration mode.
    #[arg(long, alias = "render-acceleration-mode", value_enum)]
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

fn main() -> Result<()> {
    let args = DaemonArgs::parse();
    #[cfg(target_os = "macos")]
    let macos_owner = args.macos_owner.into();
    #[cfg(target_os = "macos")]
    let macos_owner_store = MacosOwnerStore::new(ConfigManager::data_dir());
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(request_path) = args.macos_tcc_canary_arm.as_deref() {
        let path = hypercolor_daemon::macos_tcc_canary::arm_macos_tcc_canary(
            &ConfigManager::data_dir(),
            request_path,
        )?;
        println!("macos_tcc_canary_armed={}", path.display());
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(receipt_dir) = args.macos_tcc_canary_validate.as_deref() {
        let validation =
            hypercolor_daemon::macos_tcc_canary::validate_macos_tcc_canary_receipts(receipt_dir)?;
        println!("{}", serde_json::to_string_pretty(&validation)?);
        if !validation.preferred_topology_eligible {
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(request_path) = args.macos_tcc_canary_check_request.as_deref() {
        hypercolor_daemon::macos_tcc_canary::validate_macos_tcc_canary_request(request_path)?;
        println!("macos_tcc_canary_request_valid={}", request_path.display());
        return Ok(());
    }
    #[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
    if let Some(paths) = args.macos_tcc_canary_publish.as_deref() {
        let [canary_root, source, destination] = paths else {
            anyhow::bail!("macOS TCC canary artifact publication requires exactly three paths");
        };
        hypercolor_daemon::macos_tcc_canary::publish_macos_tcc_canary_artifact(
            canary_root,
            source,
            destination,
        )?;
        println!("macos_tcc_canary_artifact={}", destination.display());
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let macos_owner_identity = current_macos_owner_identity()?;
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
    let mut owner_snapshot = publish_macos_owner(
        &macos_owner_store,
        &macos_instance_guard,
        macos_owner,
        macos_owner_identity,
    )?;
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
    if hypercolor_daemon::macos_tcc_canary::run_armed_macos_tcc_canary(
        &ConfigManager::data_dir(),
        macos_owner,
    )? {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    if args.windows_service {
        return windows_service::run(args.into_run_options());
    }

    let mut options = args.into_run_options();
    #[cfg(target_os = "macos")]
    {
        options.macos_owner_snapshot = Some(owner_snapshot);
    }
    run_daemon(options)
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
) -> Result<MacosOwnerSnapshot> {
    let record = store
        .publish_guard_winner(guard, owner, identity)
        .context("failed to publish the macOS daemon owner")?;
    Ok(record.snapshot())
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
fn current_macos_owner_identity() -> Result<MacosOwnerIdentity> {
    let executable_path =
        std::env::current_exe().context("failed to resolve the current daemon executable")?;
    let requirement = designated_requirement(&executable_path)?;
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
fn run_daemon(options: DaemonRunOptions) -> Result<()> {
    let runtime = daemon::build_main_runtime()?;
    runtime.block_on(async move {
        let shutdown_rx = install_signal_handlers();
        daemon::run(options, shutdown_rx).await
    })
}

#[cfg(target_os = "macos")]
fn run_daemon(options: DaemonRunOptions) -> Result<()> {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let runtime_thread = std::thread::Builder::new()
        .name("hypercolor-daemon-runtime".to_owned())
        .spawn(move || {
            let _run_loop_stop = MainRunLoopStop;
            let result = daemon::build_main_runtime().and_then(|runtime| {
                runtime.block_on(async move {
                    let shutdown_rx = install_signal_handlers();
                    daemon::run(options, shutdown_rx).await
                })
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        DaemonArgs, RenderAccelerationModeArg, ServoGpuImportModeArg, daemon_instance_name,
    };
    #[cfg(target_os = "macos")]
    use super::{
        MacosDaemonOwnerArg, MacosOwnerContention, arbitrate_macos_owner_contention_with,
        launchd_contender_exits_zero, macos_contender_exit_code, parse_designated_requirement,
    };
    #[cfg(target_os = "macos")]
    use hypercolor_daemon::macos_owner::{
        MacosDaemonOwner, MacosOwnerIdentity, MacosOwnerStore, try_acquire_macos_daemon_guard,
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

    #[test]
    fn daemon_instance_name_is_stable() {
        let name = daemon_instance_name();

        assert!(name.contains("hypercolor-daemon"));
    }
}
