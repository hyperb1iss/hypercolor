//! Supervision of the OpenRGB fallback SDK server (Spec 81 §3.2).
//!
//! The app detects an OpenRGB installation, adopts a server that already
//! answers on loopback, or spawns a headless one from the detected binary,
//! and stops only a child it spawned. Nothing here runs at app launch: the
//! UI or the setup wizard calls the Tauri commands, and an OpenRGB exit is
//! user intent until the user asks again (no watchdog restarts).
//!
//! Before spawning, the supervisor writes the detector partition into the
//! Hypercolor-managed OpenRGB config directory so natively driven hardware
//! stays invisible to OpenRGB. Per Spec 81 §3.1 a family is disabled only
//! when its native driver is enabled and owns at least one enabled device,
//! which takes both `GET /api/v1/drivers` and `GET /api/v1/devices`.

use std::{
    ffi::OsStr,
    fs::File,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;
use hypercolor_openrgb_host::{
    DEFAULT_SERVER_PORT, DetectorPartition, InstallHint, ManagedConfigDir, OpenRgbBinary,
    PermissionCheck, ProcessSpec, ServerProbe, detect_binary, detector_families,
    detector_prefixes_for_drivers, install_hints, managed_config_dir, permission_checks,
    probe_server, server_command, write_detector_partition,
};
use hypercolor_types::api::devices::DeviceSummary;
use hypercolor_types::api::drivers::DriverSummary;
use hypercolor_types::device::DriverModuleKind;
use serde::Serialize;

use super::child::{self, PlatformGuard};
use super::plan::{OpenRgbHoldReason, OpenRgbPlan, OpenRgbPlanInputs, openrgb_plan};

/// Log file (under `<data>/logs`) receiving the managed server's stdio.
pub const OPENRGB_LOG_FILE_NAME: &str = "openrgb.log";

/// Driver id of the daemon's OpenRGB bridge driver.
pub const OPENRGB_BRIDGE_DRIVER_ID: &str = "openrgb";

/// Timeout for one SDK probe step and one TCP reachability check.
pub const OPENRGB_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Maximum time to wait for a spawned server to answer the SDK handshake.
pub const OPENRGB_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Delay between startup probes.
pub const OPENRGB_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Time a managed server gets to exit after a graceful termination request
/// before it is killed.
pub const OPENRGB_STOP_GRACE: Duration = Duration::from_secs(3);

/// Qt platform plugin selector honored by the OpenRGB binary.
pub const QT_PLATFORM_ENV: &str = "QT_QPA_PLATFORM";

/// Qt platform plugin that needs no display server.
pub const QT_PLATFORM_OFFSCREEN: &str = "offscreen";

/// Timeout for the daemon driver listing.
const DAEMON_HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// The loopback SDK endpoint the supervisor targets.
#[must_use]
pub const fn default_server_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SERVER_PORT)
}

/// Compact plan label for the tray and the UI; the details live on
/// [`OpenRgbStatus`] beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenRgbPlanSummary {
    Adopt,
    Spawn,
    HoldNotInstalled,
    HoldPermissionsMissing,
    HoldBridgeDisabled,
    HoldPortOwnedByUnknown,
    HoldStarting,
}

/// Collapse a plan to its summary label.
#[must_use]
pub const fn plan_summary(plan: &OpenRgbPlan) -> OpenRgbPlanSummary {
    match plan {
        OpenRgbPlan::Adopt { .. } => OpenRgbPlanSummary::Adopt,
        OpenRgbPlan::Spawn { .. } => OpenRgbPlanSummary::Spawn,
        OpenRgbPlan::Hold { reason } => match reason {
            OpenRgbHoldReason::NotInstalled { .. } => OpenRgbPlanSummary::HoldNotInstalled,
            OpenRgbHoldReason::PermissionsMissing { .. } => {
                OpenRgbPlanSummary::HoldPermissionsMissing
            }
            OpenRgbHoldReason::BridgeDisabled => OpenRgbPlanSummary::HoldBridgeDisabled,
            OpenRgbHoldReason::PortOwnedByUnknown { .. } => {
                OpenRgbPlanSummary::HoldPortOwnedByUnknown
            }
            OpenRgbHoldReason::Starting { .. } => OpenRgbPlanSummary::HoldStarting,
        },
    }
}

/// What the app knows about the OpenRGB fallback server right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OpenRgbStatus {
    /// The last plan decided, or `None` before the first detection.
    pub plan_summary: Option<OpenRgbPlanSummary>,
    /// Pid of the server this app spawned and still holds.
    pub managed_pid: Option<u32>,
    /// Whether the bridge rides on a server somebody else started.
    pub adopted: bool,
    /// The loopback SDK endpoint.
    pub addr: SocketAddr,
    /// The most recent SDK probe of `addr`.
    pub probe: ServerProbe,
    /// The detected OpenRGB installation.
    pub binary: Option<OpenRgbBinary>,
    /// Install hints for this host.
    pub hints: Vec<InstallHint>,
    /// Host permission checks (Linux only; empty elsewhere).
    pub checks: Vec<PermissionCheck>,
    /// Whether the daemon's OpenRGB bridge driver is enabled.
    pub bridge_enabled: bool,
    /// The detector partition written before the last spawn.
    pub partition: Option<DetectorPartition>,
    /// The last failure the supervisor hit, kept until the next action.
    pub last_error: Option<String>,
}

impl OpenRgbStatus {
    /// Fold a fresh inspection and its plan into the status.
    ///
    /// `adopted` is true only for a reachable server that is not our own
    /// child. `last_error` is cleared by every fresh look except while our
    /// own child is still starting, so the report from the spawn that
    /// launched it survives a second `start` or `detect`.
    pub fn apply_inspection(
        &mut self,
        inspection: &OpenRgbInspection,
        plan: &OpenRgbPlan,
        managed_pid: Option<u32>,
    ) {
        self.plan_summary = Some(plan_summary(plan));
        self.addr = inspection.addr;
        self.probe = inspection.probe.clone();
        self.binary = inspection.binary.clone();
        self.hints = inspection.hints.clone();
        self.checks = inspection.checks.clone();
        self.bridge_enabled = inspection.bridge_enabled;
        self.managed_pid = managed_pid;
        self.adopted = inspection.probe.reachable && managed_pid.is_none();
        let starting = matches!(
            plan,
            OpenRgbPlan::Hold {
                reason: OpenRgbHoldReason::Starting { .. }
            }
        );
        if !starting {
            self.last_error = None;
        }
    }
}

impl Default for OpenRgbStatus {
    fn default() -> Self {
        Self {
            plan_summary: None,
            managed_pid: None,
            adopted: false,
            addr: default_server_addr(),
            probe: ServerProbe::default(),
            binary: None,
            hints: Vec::new(),
            checks: Vec::new(),
            bridge_enabled: false,
            partition: None,
            last_error: None,
        }
    }
}

/// The slice of a daemon driver summary the partition decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverFacts {
    /// Stable driver id (`razer`, `openrgb`, ...).
    pub id: String,
    /// Registry category of the module.
    pub module_kind: DriverModuleKind,
    /// Whether the user has the driver enabled.
    pub enabled: bool,
}

impl From<&DriverSummary> for DriverFacts {
    fn from(summary: &DriverSummary) -> Self {
        Self {
            id: summary.descriptor.id.clone(),
            module_kind: summary.descriptor.module_kind,
            enabled: summary.enabled,
        }
    }
}

/// The slice of a daemon device summary the partition decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFacts {
    /// Driver module that owns the device.
    pub driver_id: String,
    /// Whether the user has disabled the device (`status == "disabled"`).
    pub disabled: bool,
}

impl From<&DeviceSummary> for DeviceFacts {
    fn from(summary: &DeviceSummary) -> Self {
        Self {
            driver_id: summary.origin.driver_id.clone(),
            disabled: summary.status.eq_ignore_ascii_case("disabled"),
        }
    }
}

/// Which native driver families the detector partition disables and which
/// it hands back to OpenRGB, as driver ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectorPartitionPlan {
    /// Enabled hardware drivers that own at least one enabled device: their
    /// OpenRGB detectors are written `false`.
    pub disabled_driver_ids: Vec<String>,
    /// Every other driver family the detector table knows: their detectors
    /// are written `true` so OpenRGB may drive the hardware again.
    pub re_enable_driver_ids: Vec<String>,
}

/// Driver ids the embedded detector table has families for.
#[must_use]
pub fn known_detector_driver_ids() -> Vec<String> {
    detector_families()
        .iter()
        .map(|family| family.driver_id.clone())
        .collect()
}

/// Decide the detector partition from the daemon's drivers and devices.
///
/// Spec 81 §3.1: a family is disabled for OpenRGB only when its native
/// driver is enabled and at least one of its devices is not user-disabled
/// (a device in state `known` counts as present). Every other id in
/// `known_driver_ids` is re-enabled, so a driver the user turned off, a
/// driver whose only devices are disabled, and a family with no device at
/// all are all handed back to OpenRGB. Both lists are confined to the ids
/// the detector table knows; bridge modules never partition.
#[must_use]
pub fn partition_driver_ids<S: AsRef<str>>(
    drivers: &[DriverFacts],
    devices: &[DeviceFacts],
    known_driver_ids: &[S],
) -> DetectorPartitionPlan {
    let known = |id: &str| {
        known_driver_ids
            .iter()
            .any(|candidate| candidate.as_ref().eq_ignore_ascii_case(id))
    };
    let mut disabled: Vec<String> = drivers
        .iter()
        .filter(|driver| driver.enabled && driver.module_kind != DriverModuleKind::Bridge)
        .filter(|driver| known(&driver.id))
        .filter(|driver| {
            devices
                .iter()
                .any(|device| !device.disabled && device.driver_id.eq_ignore_ascii_case(&driver.id))
        })
        .map(|driver| driver.id.clone())
        .collect();
    disabled.sort();
    disabled.dedup();
    let mut re_enable: Vec<String> = known_driver_ids
        .iter()
        .map(|id| id.as_ref().to_owned())
        .filter(|id| !disabled.iter().any(|kept| kept.eq_ignore_ascii_case(id)))
        .collect();
    re_enable.sort();
    re_enable.dedup();
    DetectorPartitionPlan {
        disabled_driver_ids: disabled,
        re_enable_driver_ids: re_enable,
    }
}

/// Whether the daemon has the OpenRGB bridge driver registered and enabled.
#[must_use]
pub fn bridge_enabled(drivers: &[DriverFacts]) -> bool {
    drivers
        .iter()
        .any(|driver| driver.id == OPENRGB_BRIDGE_DRIVER_ID && driver.enabled)
}

/// Whether the server needs Qt's offscreen platform: Linux with neither an
/// X11 nor a Wayland display in the environment.
#[must_use]
pub fn needs_offscreen_qt(
    linux: bool,
    display: Option<&OsStr>,
    wayland_display: Option<&OsStr>,
) -> bool {
    linux && display.is_none_or(OsStr::is_empty) && wayland_display.is_none_or(OsStr::is_empty)
}

/// Layer the headless Qt selection onto a launch spec without overriding
/// an explicit `QT_QPA_PLATFORM`, whether it sits in the spec's own env or
/// in the environment the child will inherit from this process.
pub fn apply_headless_env(spec: &mut ProcessSpec, headless: bool, inherited: Option<&OsStr>) {
    if !headless || inherited.is_some_and(|value| !value.is_empty()) {
        return;
    }
    spec.env
        .entry(QT_PLATFORM_ENV.to_owned())
        .or_insert_with(|| QT_PLATFORM_OFFSCREEN.to_owned());
}

/// Build the launch spec for this host from a detected binary.
///
/// # Errors
///
/// Returns the host crate's error when the managed config path cannot be
/// passed to OpenRGB (non-UTF-8, or a `:` inside a Flatpak `--filesystem`).
pub fn launch_spec(
    binary: &OpenRgbBinary,
    config_dir: &ManagedConfigDir,
    port: u16,
) -> hypercolor_openrgb_host::Result<ProcessSpec> {
    let mut spec = server_command(binary, config_dir, port)?;
    let headless = needs_offscreen_qt(
        cfg!(target_os = "linux"),
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    );
    apply_headless_env(
        &mut spec,
        headless,
        std::env::var_os(QT_PLATFORM_ENV).as_deref(),
    );
    Ok(spec)
}

/// Everything gathered from the host and the daemon for one decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbInspection {
    pub binary: Option<OpenRgbBinary>,
    pub addr: SocketAddr,
    pub probe: ServerProbe,
    pub port_open: bool,
    pub checks: Vec<PermissionCheck>,
    pub hints: Vec<InstallHint>,
    pub bridge_enabled: bool,
    /// Which detector families to disable and which to hand back.
    pub partition_plan: DetectorPartitionPlan,
    pub config_dir: ManagedConfigDir,
    /// The launch spec built from `binary`, when one was detected.
    pub spawn: Option<ProcessSpec>,
    /// Pid of the server this app already spawned, when it is still alive.
    pub managed_pid: Option<u32>,
}

impl OpenRgbInspection {
    /// Decide the plan for this inspection.
    #[must_use]
    pub fn plan(&self) -> OpenRgbPlan {
        openrgb_plan(OpenRgbPlanInputs {
            bridge_enabled: self.bridge_enabled,
            binary: self.binary.clone(),
            addr: self.addr,
            probe: self.probe.clone(),
            port_open: self.port_open,
            checks: self.checks.clone(),
            hints: self.hints.clone(),
            spawn: self.spawn.clone(),
            managed_pid: self.managed_pid,
        })
    }
}

/// Whether anything accepts a TCP connection on `addr`.
pub async fn tcp_port_open(addr: SocketAddr, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

/// Gather the host facts and the daemon's driver and device view for `addr`.
///
/// `managed_pid` is the child this app already holds, if any, so the plan
/// can tell a server that is still starting from a foreign listener.
///
/// # Errors
///
/// Returns an error when the daemon driver or device listing fails (without
/// them the bridge state and the native device set are unknown, and guessing
/// either would risk spawning a server nothing consumes or one that fights
/// native drivers for hardware), or when the managed config path cannot be
/// passed to the detected binary.
pub async fn inspect(
    daemon_base_url: &str,
    addr: SocketAddr,
    managed_pid: Option<u32>,
) -> Result<OpenRgbInspection> {
    let http = reqwest::Client::builder()
        .timeout(DAEMON_HTTP_TIMEOUT)
        .build()
        .context("failed to build the daemon HTTP client")?;
    let drivers: Vec<DriverFacts> =
        crate::daemon_client::fetch_driver_summaries(&http, daemon_base_url)
            .await
            .context("failed to read the daemon driver list")?
            .iter()
            .map(DriverFacts::from)
            .collect();
    let devices: Vec<DeviceFacts> =
        crate::daemon_client::fetch_device_summaries(&http, daemon_base_url)
            .await
            .context("failed to read the daemon device list")?
            .iter()
            .map(DeviceFacts::from)
            .collect();

    let binary = detect_binary().await;
    let probe = probe_server(addr, OPENRGB_PROBE_TIMEOUT).await;
    let port_open = probe.reachable || tcp_port_open(addr, OPENRGB_PROBE_TIMEOUT).await;
    let checks = permission_checks();
    let hints = install_hints();
    let config_dir = managed_config_dir(&data_dir());
    let spawn = binary
        .as_ref()
        .map(|binary| launch_spec(binary, &config_dir, addr.port()))
        .transpose()
        .context("cannot launch OpenRGB from the managed config directory")?;

    Ok(OpenRgbInspection {
        partition_plan: partition_driver_ids(&drivers, &devices, &known_detector_driver_ids()),
        bridge_enabled: bridge_enabled(&drivers),
        binary,
        addr,
        probe,
        port_open,
        checks,
        hints,
        config_dir,
        spawn,
        managed_pid,
    })
}

/// Wait until the server at `addr` completes the SDK handshake, or give up.
pub async fn wait_until_answering(addr: SocketAddr, timeout: Duration) -> Option<ServerProbe> {
    let started = Instant::now();
    loop {
        let probe = probe_server(addr, OPENRGB_PROBE_TIMEOUT).await;
        if probe.reachable {
            return Some(probe);
        }
        let remaining = timeout.checked_sub(started.elapsed())?;
        let delay = super::startup_retry_delay(remaining, OPENRGB_STARTUP_POLL_INTERVAL)?;
        tokio::time::sleep(delay).await;
    }
}

/// An OpenRGB server this app spawned and owns until it exits or is stopped.
pub struct ManagedOpenRgb {
    child: Option<Child>,
    #[allow(dead_code)]
    platform_guard: PlatformGuard,
    pid: u32,
}

impl ManagedOpenRgb {
    /// Spawn `spec` as a supervised child with stdio appended to `log`.
    ///
    /// # Errors
    ///
    /// Returns an error when the log handle cannot be duplicated, the
    /// process cannot be spawned, or the platform guard cannot be attached.
    pub fn spawn(spec: &ProcessSpec, log: File) -> Result<Self> {
        let mut process = Command::new(&spec.program);
        process
            .args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                log.try_clone().context("failed to clone the log handle")?,
            ))
            .stderr(Stdio::from(log));
        if let Some(cwd) = &spec.cwd {
            process.current_dir(cwd);
        }
        let (child, platform_guard) = child::spawn_supervised(&mut process, &spec.program)?;
        let pid = child.id();
        Ok(Self {
            child: Some(child),
            platform_guard,
            pid,
        })
    }

    /// The pid this child was spawned with.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the child has exited on its own; a confirmed exit reaps it.
    pub fn has_exited(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::info!(pid = self.pid, ?status, "managed OpenRGB server exited");
                self.child = None;
                true
            }
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(pid = self.pid, %error, "managed OpenRGB wait failed");
                false
            }
        }
    }

    /// Stop the child: graceful request, `grace` to comply, then kill.
    pub fn stop(&mut self, grace: Duration) -> Option<ExitStatus> {
        let mut child = self.child.take()?;
        let status = child::stop_child(&mut child, grace);
        tracing::info!(pid = self.pid, ?status, "managed OpenRGB server stopped");
        status
    }
}

impl Drop for ManagedOpenRgb {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child::kill_tree_unless_exited(&mut child);
        }
    }
}

/// App-wide OpenRGB supervision state, managed by Tauri and read by the
/// tray and the UI.
#[derive(Clone, Default)]
pub struct OpenRgbSupervisor {
    managed: Arc<Mutex<Option<ManagedOpenRgb>>>,
    status: Arc<Mutex<OpenRgbStatus>>,
    /// Set while a `start` is between its await points, so a second call
    /// (a double click) cannot spawn a sibling server.
    start_in_flight: Arc<AtomicBool>,
}

/// Releases the start-in-flight flag on every exit path of `start`.
struct StartGuard(Arc<AtomicBool>);

impl StartGuard {
    fn acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl OpenRgbSupervisor {
    /// A snapshot of the current status, with a reaped child reflected.
    #[must_use]
    pub fn status(&self) -> OpenRgbStatus {
        self.reap_if_exited();
        self.status_guard().clone()
    }

    /// Pid of the server this app spawned, when it is still running.
    #[must_use]
    pub fn managed_pid(&self) -> Option<u32> {
        self.reap_if_exited();
        self.managed_guard().as_ref().map(ManagedOpenRgb::pid)
    }

    /// Detect without acting: refresh the status from a fresh inspection
    /// and record the plan it would execute.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon driver list cannot be read.
    pub async fn detect(&self, daemon_base_url: &str) -> Result<OpenRgbStatus> {
        let addr = self.status_guard().addr;
        let inspection = inspect(daemon_base_url, addr, self.managed_pid()).await?;
        let plan = inspection.plan();
        self.record_inspection(&inspection, &plan);
        Ok(self.status())
    }

    /// Execute the plan for a fresh inspection: adopt, spawn, or hold.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon driver or device list cannot be
    /// read, the detector partition cannot be written, or the server cannot
    /// be spawned. A spawned server that has not answered within the startup
    /// budget is kept running and reported through `last_error`. A call that
    /// overlaps another `start` returns the current status without acting.
    pub async fn start(&self, daemon_base_url: &str) -> Result<OpenRgbStatus> {
        let Some(_in_flight) = StartGuard::acquire(&self.start_in_flight) else {
            tracing::info!("OpenRGB start already in progress; ignoring the overlapping call");
            return Ok(self.status());
        };
        let addr = self.status_guard().addr;
        let inspection = inspect(daemon_base_url, addr, self.managed_pid()).await?;
        let plan = inspection.plan();
        self.record_inspection(&inspection, &plan);

        match plan {
            // Adopt is already fully reflected by record_inspection; a
            // reachable server that is our own child stays managed, not
            // adopted.
            OpenRgbPlan::Adopt { .. } | OpenRgbPlan::Hold { .. } => {}
            OpenRgbPlan::Spawn { spec } => {
                if let Some(pid) = self.managed_pid() {
                    tracing::info!(
                        pid,
                        "managed OpenRGB server already running; not spawning again"
                    );
                    return Ok(self.status());
                }
                // The partition must land before the launch: the Flatpak
                // `--filesystem=` grant needs the directory to exist.
                let disabled =
                    detector_prefixes_for_drivers(&inspection.partition_plan.disabled_driver_ids);
                let re_enable =
                    detector_prefixes_for_drivers(&inspection.partition_plan.re_enable_driver_ids);
                let config_dir = inspection.config_dir.clone();
                let partition = tokio::task::spawn_blocking(move || {
                    write_detector_partition(&config_dir, &disabled, &re_enable, None)
                })
                .await
                .context("detector partition task failed")?
                .context("failed to write the OpenRGB detector partition")?;
                tracing::info!(
                    disabled = partition.disabled.len(),
                    enabled = partition.enabled.len(),
                    "wrote the managed OpenRGB detector partition"
                );
                self.update_status(|status| status.partition = Some(partition));

                let managed = tokio::task::spawn_blocking(move || {
                    let log = child::supervised_log_file(OPENRGB_LOG_FILE_NAME)
                        .context("failed to open the OpenRGB log file")?;
                    ManagedOpenRgb::spawn(&spec, log)
                })
                .await
                .context("OpenRGB spawn task failed")??;
                let pid = managed.pid();
                tracing::info!(pid, %addr, "spawned the managed OpenRGB server");
                *self.managed_guard() = Some(managed);
                self.update_status(|status| {
                    status.managed_pid = Some(pid);
                    status.adopted = false;
                });

                match wait_until_answering(addr, OPENRGB_STARTUP_TIMEOUT).await {
                    Some(probe) => self.update_status(|status| status.probe = probe),
                    None => {
                        let exited = self.reap_if_exited();
                        let message = if exited {
                            "OpenRGB exited before answering the SDK handshake; see logs/openrgb.log"
                        } else {
                            "OpenRGB has not answered the SDK handshake yet (device detection can take a while); see logs/openrgb.log"
                        };
                        tracing::warn!(pid, %addr, message);
                        self.update_status(|status| status.last_error = Some(message.to_owned()));
                    }
                }
            }
        }

        Ok(self.status())
    }

    /// Stop the server this app spawned. A no-op for adopted servers and
    /// when nothing is running.
    pub fn stop_managed(&self) -> Option<ExitStatus> {
        let mut managed = self.managed_guard().take()?;
        let status = managed.stop(OPENRGB_STOP_GRACE);
        self.update_status(|status| {
            status.managed_pid = None;
            status.probe = ServerProbe::default();
        });
        status
    }

    /// Reap the managed server on app exit.
    ///
    /// `app.exit()` terminates without unwinding, so `ManagedOpenRgb::Drop`
    /// never fires on its own; the `RunEvent::Exit` handler calls this while
    /// the process still lives.
    pub fn terminate_managed_for_exit(&self) {
        if let Some(mut managed) = self.managed_guard().take() {
            let pid = managed.pid();
            let status = managed.stop(OPENRGB_STOP_GRACE);
            tracing::info!(pid, ?status, "managed OpenRGB server reaped on app exit");
        }
    }

    fn record_inspection(&self, inspection: &OpenRgbInspection, plan: &OpenRgbPlan) {
        let managed_pid = self.managed_pid();
        self.update_status(|status| status.apply_inspection(inspection, plan, managed_pid));
    }

    /// Drop a child that exited on its own and clear its pid. Returns
    /// whether a child was found exited.
    fn reap_if_exited(&self) -> bool {
        let mut managed = self.managed_guard();
        let exited = managed.as_mut().is_some_and(ManagedOpenRgb::has_exited);
        if exited {
            *managed = None;
            drop(managed);
            self.update_status(|status| status.managed_pid = None);
        }
        exited
    }

    fn update_status(&self, update: impl FnOnce(&mut OpenRgbStatus)) {
        update(&mut self.status_guard());
    }

    fn status_guard(&self) -> MutexGuard<'_, OpenRgbStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn managed_guard(&self) -> MutexGuard<'_, Option<ManagedOpenRgb>> {
        self.managed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Detect the OpenRGB binary, probe the SDK port, run the permission checks,
/// and report the plan the supervisor would execute, without acting.
///
/// Detection runs `openrgb --version`, which may create the user's own
/// OpenRGB config directory as a side effect of the binary starting up;
/// that directory is OpenRGB's, not the managed one, and is left alone.
///
/// # Errors
///
/// Returns a message when the daemon driver or device list cannot be read.
#[tauri::command]
pub async fn detect_openrgb(
    state: tauri::State<'_, OpenRgbSupervisor>,
) -> Result<OpenRgbStatus, String> {
    let supervisor = state.inner().clone();
    supervisor
        .detect(&crate::daemon_base_url())
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Execute the OpenRGB plan: adopt a live server, spawn a headless one, or
/// hold with the reason carried in the returned status. Runs the same
/// detection as [`detect_openrgb`] first (including `openrgb --version`).
///
/// # Errors
///
/// Returns a message when the daemon driver or device list cannot be read,
/// the detector partition cannot be written, or the spawn fails.
#[tauri::command]
pub async fn start_openrgb(
    state: tauri::State<'_, OpenRgbSupervisor>,
) -> Result<OpenRgbStatus, String> {
    let supervisor = state.inner().clone();
    supervisor
        .start(&crate::daemon_base_url())
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Stop the server this app spawned. Adopted servers are left alone.
///
/// # Errors
///
/// Returns a message when the blocking stop task fails to run.
#[tauri::command]
pub async fn stop_openrgb(
    state: tauri::State<'_, OpenRgbSupervisor>,
) -> Result<OpenRgbStatus, String> {
    let supervisor = state.inner().clone();
    let stopper = supervisor.clone();
    tokio::task::spawn_blocking(move || stopper.stop_managed())
        .await
        .map_err(|error| format!("OpenRGB stop task failed: {error}"))?;
    Ok(supervisor.status())
}

/// Install hints for this host, in preference order.
#[tauri::command]
#[must_use]
pub fn openrgb_install_hints() -> Vec<InstallHint> {
    install_hints()
}
